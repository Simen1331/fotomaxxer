"""Faces-steg: deteksjon + ArcFace-embedding, og klynging til personer.

To operasjoner mot samme stage (``faces``):

* ``detect`` — per bilde: insightface ``buffalo_l`` på 1024-derivatet → én
  ``faces``-rad per ansikt (bbox i **originalkoordinater**, embedding som
  L2-normalisert float32-BLOB). 0 ansikter er gyldig → ``done`` likevel.
  CPU-bundet → prosess-pool, som ``derive``. Myk per-bilde-feil.
* ``cluster`` — jobber på lagrede vektorer, ikke bilder. Inkrementell
  tilordning av NULL-ansikter til nærmeste eksisterende klynge er standard
  (bevarer navn brukeren har satt); HDBSCAN føder nye klynger fra resten.
  ``--full`` re-klynger alt og re-mapper navn via sentroide-matching.

insightface/cv2 importeres **lazy inne i pool-arbeideren** — hovedprosessen
laster aldri onnxruntime før fork, så prosess-poolen er trygg.
"""

from __future__ import annotations

import json
import sqlite3
from concurrent.futures import ProcessPoolExecutor
from dataclasses import dataclass

import numpy as np

from . import settings as settings_mod
from .storage import Storage
from .resources import face_model_root

_DET_SIZE = (640, 640)
_EMBED_DIM = 512

# Per-prosess modell-singleton (settes lazy i hver pool-arbeider).
_APP = None


def _get_app():
    """Lat-init av FaceAnalysis i den kallende prosessen. Lastes én gang per arbeider."""
    global _APP
    if _APP is None:
        from insightface.app import FaceAnalysis

        root = face_model_root()
        kwargs = {"root": str(root)} if root is not None else {}
        app = FaceAnalysis(name="buffalo_l", providers=["CPUExecutionProvider"], **kwargs)
        app.prepare(ctx_id=-1, det_size=_DET_SIZE)
        _APP = app
    return _APP


@dataclass
class FaceStats:
    pending: int = 0
    done: int = 0
    errors: int = 0
    faces_found: int = 0

    def summary(self) -> str:
        return f"pending={self.pending} done={self.done} ansikter={self.faces_found} feil={self.errors}"


@dataclass
class ClusterStats:
    faces_total: int = 0
    assigned_incremental: int = 0
    new_clusters: int = 0
    clusters_total: int = 0
    unassigned: int = 0
    names_remapped: int = 0
    full: bool = False

    def summary(self) -> str:
        head = "full re-klynging" if self.full else "inkrementell"
        return (
            f"{head}: ansikter={self.faces_total} klynger={self.clusters_total} "
            f"tilordnet={self.assigned_incremental} nye={self.new_clusters} "
            f"utilordnet={self.unassigned} navn-remappet={self.names_remapped}"
        )


# --- Deteksjon -------------------------------------------------------------


def _detect_one(
    job: tuple[int, str, int | None, int | None],
) -> tuple[int, list[tuple[str, bytes]] | None, str | None]:
    """Pool-arbeider: les 1024-derivat, detektér ansikter, skalér bbox til original.

    Returnerer (image_id, [(bbox_json, embedding_bytes), …], error). Ren av DB.
    """
    import cv2

    image_id, derived_path, orig_w, orig_h = job
    try:
        # cv2.imread kan ikke åpne stier med æøå på Windows; dekod fra bytes.
        img = cv2.imdecode(np.fromfile(derived_path, dtype=np.uint8), cv2.IMREAD_COLOR)
        if img is None:
            return image_id, None, "kunne ikke lese derivat (cv2.imdecode → None)"
        thumb_h, thumb_w = img.shape[:2]
        scale_x = (orig_w / thumb_w) if orig_w else 1.0
        scale_y = (orig_h / thumb_h) if orig_h else 1.0

        app = _get_app()
        detections = app.get(img)
        out: list[tuple[str, bytes]] = []
        for face in detections:
            x1, y1, x2, y2 = face.bbox  # piksler i thumb-koordinater
            bbox = [
                round(x1 * scale_x),
                round(y1 * scale_y),
                round((x2 - x1) * scale_x),
                round((y2 - y1) * scale_y),
            ]
            emb = np.asarray(face.normed_embedding, dtype=np.float32)
            out.append((json.dumps(bbox), emb.tobytes()))
        return image_id, out, None
    except Exception as e:  # korrupt derivat o.l. — feiler mykt
        return image_id, None, f"{type(e).__name__}: {e}"


def detect(
    conn: sqlite3.Connection,
    storage: Storage,
    workers: int | None = None,
    retry_errors: bool = False,
    source_id: int | None = None,
) -> FaceStats:
    """Detektér ansikter på pending (ev. error) bilder → faces-rader. Idempotent.

    ``source_id`` begrenser til bilder med en fil i den kilden (per-mappe-kjøring);
    ``None`` = alle pending."""
    statuses = ("pending", "error") if retry_errors else ("pending",)
    placeholders = ",".join("?" for _ in statuses)
    source_clause = ""
    params: list[object] = list(statuses)
    if source_id is not None:
        source_clause = " AND EXISTS (SELECT 1 FROM files f WHERE f.image_id = i.id AND f.source_id = ?)"
        params.append(source_id)
    rows = conn.execute(
        f"""
        SELECT i.id AS image_id, i.content_hash AS content_hash,
               i.width AS width, i.height AS height
        FROM images i
        JOIN processing_state pf ON pf.image_id = i.id AND pf.stage = 'faces'
                                AND pf.status IN ({placeholders})
        JOIN processing_state pd ON pd.image_id = i.id AND pd.stage = 'derive' AND pd.status = 'done'
        JOIN derived d           ON d.image_id  = i.id AND d.size = '1024'
        WHERE 1=1{source_clause}
        ORDER BY i.id
        """,
        params,
    ).fetchall()

    stats = FaceStats(pending=len(rows))
    if not rows:
        return stats

    jobs = [
        (
            r["image_id"],
            str(storage.derived_path(r["content_hash"], "1024")),
            r["width"],
            r["height"],
        )
        for r in rows
    ]

    with ProcessPoolExecutor(max_workers=workers) as pool:
        # Strøm resultater og commit per bilde, så faces_total/done beveger seg
        # live under inferensen i stedet for som ett hopp når poolen er ferdig.
        for image_id, faces_data, error in pool.map(_detect_one, jobs):
            if error is not None:
                _set_state(conn, image_id, "error", error)
                stats.errors += 1
                conn.commit()
                continue
            _persist_faces(conn, image_id, faces_data or [])
            stats.faces_found += len(faces_data or [])
            stats.done += 1
            conn.commit()

    return stats


def _persist_faces(conn: sqlite3.Connection, image_id: int, faces_data: list[tuple[str, bytes]]) -> None:
    """Erstatt faces for bildet og sett faces-state done (også for 0 ansikter)."""
    conn.execute("DELETE FROM faces WHERE image_id = ?", (image_id,))
    for bbox_json, emb_bytes in faces_data:
        # locked=0: et nydetektert ansikt er ferskt — re-deteksjon nullstiller
        # en eventuell tidligere frys (raden er uansett slettet over).
        conn.execute(
            "INSERT INTO faces (image_id, bbox, embedding, cluster_id, locked) "
            "VALUES (?, ?, ?, NULL, 0)",
            (image_id, bbox_json, emb_bytes),
        )
    _set_state(conn, image_id, "done", None)


# --- Klynging ---------------------------------------------------------------


def _normalize(v: np.ndarray) -> np.ndarray:
    n = float(np.linalg.norm(v))
    return v / n if n > 0 else v


def _centroid(embs: np.ndarray) -> np.ndarray:
    """Re-normalisert gjennomsnitt av (allerede normaliserte) embeddings."""
    return _normalize(embs.mean(axis=0))


def _hdbscan(embs: np.ndarray, min_cluster_size: int) -> np.ndarray:
    """HDBSCAN over normaliserte vektorer (euklidsk ≈ cosinus). -1 = støy."""
    from sklearn.cluster import HDBSCAN

    model = HDBSCAN(min_cluster_size=min_cluster_size, metric="euclidean", copy=True)
    return model.fit_predict(embs)


def _load_faces(
    conn: sqlite3.Connection,
) -> tuple[list[int], np.ndarray, list[int | None], list[bool]]:
    rows = conn.execute(
        "SELECT id, embedding, cluster_id, locked FROM faces ORDER BY id"
    ).fetchall()
    ids = [r["id"] for r in rows]
    cluster_ids = [r["cluster_id"] for r in rows]
    locked = [bool(r["locked"]) for r in rows]
    if not rows:
        return ids, np.empty((0, _EMBED_DIM), dtype=np.float32), cluster_ids, locked
    embs = np.stack([np.frombuffer(r["embedding"], dtype=np.float32) for r in rows])
    return ids, embs, cluster_ids, locked


def cluster(
    conn: sqlite3.Connection,
    full: bool = False,
    threshold: float | None = None,
) -> ClusterStats:
    """Klyng ansikter til personer. Inkrementell (standard) eller full re-klynging."""
    if threshold is None:
        threshold = float(settings_mod.get_setting(conn, "face_match_threshold") or 0.5)
    min_cluster_size = int(settings_mod.get_setting(conn, "face_min_cluster_size") or 3)

    ids, embs, cluster_ids, locked = _load_faces(conn)
    stats = ClusterStats(faces_total=len(ids), full=full)
    if len(ids) == 0:
        return stats

    if full:
        _cluster_full(conn, ids, embs, cluster_ids, locked, min_cluster_size, threshold, stats)
    else:
        _cluster_incremental(conn, ids, embs, cluster_ids, locked, min_cluster_size, threshold, stats)

    conn.commit()
    stats.clusters_total = conn.execute("SELECT COUNT(*) FROM persons").fetchone()[0]
    stats.unassigned = conn.execute("SELECT COUNT(*) FROM faces WHERE cluster_id IS NULL").fetchone()[0]
    return stats


def _existing_centroids(
    conn: sqlite3.Connection, embs: np.ndarray, cluster_ids: list[int | None]
) -> dict[int, np.ndarray]:
    """Sentroide per eksisterende persons-klynge fra dens medlemmer."""
    centroids: dict[int, np.ndarray] = {}
    for p in conn.execute("SELECT cluster_id FROM persons").fetchall():
        cid = p["cluster_id"]
        members = [i for i, c in enumerate(cluster_ids) if c == cid]
        if members:
            centroids[cid] = _centroid(embs[members])
    return centroids


def _cluster_incremental(conn, ids, embs, cluster_ids, locked, min_cluster_size, threshold, stats):
    # Sentroider regnes fra klynge-medlemmer (også låste); kun NULL-poolen
    # tilordnes, og låste ansikter holdes utenfor den (et ignorert søppel-
    # ansikt med locked=1, cluster_id=NULL blir aldri re-tilordnet).
    centroids = _existing_centroids(conn, embs, cluster_ids)
    null_idx = [i for i, c in enumerate(cluster_ids) if c is None and not locked[i]]

    remaining: list[int] = []
    for i in null_idx:
        if centroids:
            cid, sim = max(
                ((cid, float(embs[i] @ c)) for cid, c in centroids.items()),
                key=lambda kv: kv[1],
            )
            if sim >= threshold:
                conn.execute("UPDATE faces SET cluster_id = ? WHERE id = ?", (cid, ids[i]))
                stats.assigned_incremental += 1
                continue
        remaining.append(i)

    # Birth: HDBSCAN på gjenstående NULL-pool alene.
    if len(remaining) >= min_cluster_size:
        labels = _hdbscan(embs[remaining], min_cluster_size)
        for label in sorted(set(labels)):
            if label == -1:
                continue
            members = [remaining[j] for j, l in enumerate(labels) if l == label]
            new_cid = conn.execute("INSERT INTO persons (name) VALUES (NULL)").lastrowid
            for i in members:
                conn.execute("UPDATE faces SET cluster_id = ? WHERE id = ?", (new_cid, ids[i]))
            stats.new_clusters += 1


def _cluster_full(conn, ids, embs, cluster_ids, locked, min_cluster_size, threshold, stats):
    # Snapshot navngitte klyngers sentroider FØR vi river dem (membership
    # inkluderer låste medlemmer, jf. inkrementell).
    old_named: list[tuple[str, np.ndarray]] = []
    for p in conn.execute("SELECT cluster_id, name FROM persons WHERE name IS NOT NULL").fetchall():
        members = [i for i, c in enumerate(cluster_ids) if c == p["cluster_id"]]
        if members:
            old_named.append((p["name"], _centroid(embs[members])))

    # Full re-klynging rører kun ikke-låste ansikter. Låste beholder cluster_id
    # (manuelt tilordnet) eller forblir NULL (ignorert søppel) — uberørt.
    movable = [i for i in range(len(ids)) if not locked[i]]

    # Nullstill kun de flyttbare; så slett personer ingen (låst) ansikt lenger
    # peker på. Personer med låste medlemmer overlever.
    for i in movable:
        if cluster_ids[i] is not None:
            conn.execute("UPDATE faces SET cluster_id = NULL WHERE id = ?", (ids[i],))
    conn.execute(
        "DELETE FROM persons WHERE cluster_id NOT IN "
        "(SELECT cluster_id FROM faces WHERE cluster_id IS NOT NULL)"
    )

    new_centroids: dict[int, np.ndarray] = {}
    if len(movable) >= min_cluster_size:
        labels = _hdbscan(embs[movable], min_cluster_size)
        for label in sorted(set(labels)):
            if label == -1:
                continue
            members = [movable[j] for j, l in enumerate(labels) if l == label]
            new_cid = conn.execute("INSERT INTO persons (name) VALUES (NULL)").lastrowid
            for i in members:
                conn.execute("UPDATE faces SET cluster_id = ? WHERE id = ?", (new_cid, ids[i]))
            new_centroids[new_cid] = _centroid(embs[members])
            stats.new_clusters += 1
            stats.assigned_incremental += len(members)

    # Re-map navn: hver gammel navngitt sentroide → nærmeste nye klynge.
    for name, old_c in old_named:
        if not new_centroids:
            break
        cid, sim = max(
            ((cid, float(old_c @ c)) for cid, c in new_centroids.items()),
            key=lambda kv: kv[1],
        )
        if sim >= threshold:
            conn.execute("UPDATE persons SET name = ? WHERE cluster_id = ?", (name, cid))
            stats.names_remapped += 1


# --- Korrigering / kuratering (GUI-mutasjoner) ------------------------------
#
# Rene funksjoner API-laget wrapper (PUT /api/faces/{id}, POST/PUT/DELETE
# /api/persons…). Hver tar en connection, gjør én logisk endring og committer.
# Klynging respekterer resultatet: et frosset ansikt (locked=1) rører cluster()
# aldri; et tilordnet ansikt (cluster_id != NULL) hoppes over av inkrementell.


def _face_exists(conn: sqlite3.Connection, face_id: int) -> bool:
    return conn.execute("SELECT 1 FROM faces WHERE id = ?", (face_id,)).fetchone() is not None


def _person_exists(conn: sqlite3.Connection, cluster_id: int) -> bool:
    return (
        conn.execute("SELECT 1 FROM persons WHERE cluster_id = ?", (cluster_id,)).fetchone()
        is not None
    )


def reassign_face(conn: sqlite3.Connection, face_id: int, cluster_id: int | None) -> None:
    """Flytt ett ansikt til en annen person, eller løsne det (``cluster_id=None``)."""
    if not _face_exists(conn, face_id):
        raise ValueError(f"ansikt {face_id} finnes ikke")
    if cluster_id is not None and not _person_exists(conn, cluster_id):
        raise ValueError(f"person {cluster_id} finnes ikke")
    conn.execute("UPDATE faces SET cluster_id = ? WHERE id = ?", (cluster_id, face_id))
    conn.commit()


def set_face_locked(conn: sqlite3.Connection, face_id: int, locked: bool) -> None:
    """Frys/ignorér eller tin opp ett ansikt.

    ``locked=True`` + ``cluster_id=None`` = ignorer søppel-ansikt (statue/maleri):
    cluster() vil aldri re-tilordne det. ``locked=True`` på et tilordnet ansikt
    fryser den manuelle tilordningen mot full re-klynging.
    """
    if not _face_exists(conn, face_id):
        raise ValueError(f"ansikt {face_id} finnes ikke")
    conn.execute("UPDATE faces SET locked = ? WHERE id = ?", (1 if locked else 0, face_id))
    conn.commit()


def create_person_from_faces(
    conn: sqlite3.Connection, face_ids: list[int], name: str | None = None
) -> int:
    """Opprett en ny person av et utvalg ansikter (splitt). Returnerer cluster_id."""
    if not face_ids:
        raise ValueError("face_ids er tom")
    for fid in face_ids:
        if not _face_exists(conn, fid):
            raise ValueError(f"ansikt {fid} finnes ikke")
    cluster_id = conn.execute("INSERT INTO persons (name) VALUES (?)", (name,)).lastrowid
    conn.executemany(
        "UPDATE faces SET cluster_id = ? WHERE id = ?",
        [(cluster_id, fid) for fid in face_ids],
    )
    conn.commit()
    return cluster_id


def merge_persons(conn: sqlite3.Connection, source_cluster_id: int, into_cluster_id: int) -> None:
    """Slå sammen to personer: flytt alle ansikter til ``into`` og slett kilden."""
    if source_cluster_id == into_cluster_id:
        raise ValueError("kan ikke slå sammen en person med seg selv")
    if not _person_exists(conn, source_cluster_id):
        raise ValueError(f"person {source_cluster_id} finnes ikke")
    if not _person_exists(conn, into_cluster_id):
        raise ValueError(f"person {into_cluster_id} finnes ikke")
    # Flytt ansiktene først, så peker ingen på kilden når den slettes.
    conn.execute(
        "UPDATE faces SET cluster_id = ? WHERE cluster_id = ?",
        (into_cluster_id, source_cluster_id),
    )
    conn.execute("DELETE FROM persons WHERE cluster_id = ?", (source_cluster_id,))
    conn.commit()


def delete_person(conn: sqlite3.Connection, cluster_id: int) -> None:
    """Slett en person. Ansiktene løsnes (``cluster_id → NULL`` via ON DELETE SET NULL)."""
    if not _person_exists(conn, cluster_id):
        raise ValueError(f"person {cluster_id} finnes ikke")
    conn.execute("DELETE FROM persons WHERE cluster_id = ?", (cluster_id,))
    conn.commit()


def ignore_person(conn: sqlite3.Connection, cluster_id: int) -> int:
    """Kryss bort en søppel-klynge permanent. Returnerer antall ignorerte ansikter.

    Til forskjell fra ``delete_person`` (som bare løsner ansiktene, så de blir
    re-klynget til samme person ved neste ``cluster()``): her **låses** alle
    ansiktene og løsnes (``locked=1, cluster_id=NULL``). Et låst, uklynget ansikt
    rører ``cluster()`` aldri — så personen forsvinner fra lista og gjenoppstår
    aldri. Bruk for tilfeldig søppel (statuer, malerier, halve ansikter)."""
    if not _person_exists(conn, cluster_id):
        raise ValueError(f"person {cluster_id} finnes ikke")
    n = conn.execute(
        "UPDATE faces SET locked = 1, cluster_id = NULL WHERE cluster_id = ?", (cluster_id,)
    ).rowcount
    conn.execute("DELETE FROM persons WHERE cluster_id = ?", (cluster_id,))
    conn.commit()
    return n


def set_person_name(conn: sqlite3.Connection, cluster_id: int, name: str | None) -> None:
    """Navngi/omdøp en person (``name=None`` fjerner navnet). Wrapper PUT /api/persons/{id}."""
    if not _person_exists(conn, cluster_id):
        raise ValueError(f"person {cluster_id} finnes ikke")
    conn.execute("UPDATE persons SET name = ? WHERE cluster_id = ?", (name, cluster_id))
    conn.commit()


def _set_state(conn: sqlite3.Connection, image_id: int, status: str, error: str | None) -> None:
    conn.execute(
        "UPDATE processing_state SET status = ?, error = ?, updated_at = datetime('now') "
        "WHERE image_id = ? AND stage = 'faces'",
        (status, error, image_id),
    )
