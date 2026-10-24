"""Personer / ansikter: oversikt, person-detalj (ansikts-rutenett) + korrigering.

Lese-laget over ``persons``/``faces``/``images``. Mutasjonene er rene funksjoner
i ``fotomaxxer.faces`` (``reassign_face``, ``set_face_locked``,
``create_person_from_faces``, ``merge_persons``, ``delete_person``,
``set_person_name``) — API-laget wrapper dem uendret; hver committer selv og
reiser ``ValueError`` på ukjent ansikt/person (→ 404 her).

Ansikter på ekskluderte bilder skjules (samme jernregel som bilde-listene):
cover og rutenett joiner ``images`` med ``excluded = 0``.
"""

from __future__ import annotations

import json
import sqlite3

from fastapi import APIRouter, Depends, HTTPException, Query

from api.deps import get_conn
from api.routers.images import _build_filters, _thumb_url
from api.schemas import (
    FacePatch,
    FacePatchResult,
    ImageList,
    ImageListItem,
    PersonCover,
    PersonCreate,
    PersonCreateResult,
    PersonFaceItem,
    PersonFaceList,
    PersonList,
    PersonMerge,
    PersonRename,
    PersonRenameResult,
    PersonSummary,
)
from fotomaxxer import faces

router = APIRouter(tags=["persons"])


def _require_person(conn: sqlite3.Connection, cluster_id: int) -> None:
    row = conn.execute("SELECT 1 FROM persons WHERE cluster_id = ?", (cluster_id,)).fetchone()
    if row is None:
        raise HTTPException(
            status_code=404, detail={"error": "not_found", "message": "person finnes ikke"}
        )


def _cover(conn: sqlite3.Connection, cluster_id: int) -> PersonCover | None:
    row = conn.execute(
        """
        SELECT f.image_id, i.content_hash AS hash, f.bbox, i.width, i.height
        FROM faces f JOIN images i ON i.id = f.image_id
        WHERE f.cluster_id = ? AND i.excluded = 0
        ORDER BY f.locked DESC, f.id
        LIMIT 1
        """,
        (cluster_id,),
    ).fetchone()
    if row is None:
        return None
    return PersonCover(
        image_id=row["image_id"],
        hash=row["hash"],
        bbox=json.loads(row["bbox"]),
        width=row["width"],
        height=row["height"],
    )


@router.get("/persons", response_model=PersonList)
def list_persons(conn: sqlite3.Connection = Depends(get_conn)) -> PersonList:
    """Klynge-kort: alle personer med minst ett ikke-ekskludert ansikt."""
    rows = conn.execute(
        """
        SELECT p.cluster_id AS cluster_id, p.name AS name, COUNT(f.id) AS face_count
        FROM persons p
        JOIN faces f ON f.cluster_id = p.cluster_id
        JOIN images i ON i.id = f.image_id
        WHERE i.excluded = 0
        GROUP BY p.cluster_id
        ORDER BY face_count DESC, p.name IS NULL, p.name
        """
    ).fetchall()
    items = [
        PersonSummary(
            cluster_id=r["cluster_id"],
            name=r["name"],
            face_count=r["face_count"],
            cover=_cover(conn, r["cluster_id"]),
        )
        for r in rows
    ]
    return PersonList(items=items)


@router.get("/persons/{cluster_id}/faces", response_model=PersonFaceList)
def person_faces(cluster_id: int, conn: sqlite3.Connection = Depends(get_conn)) -> PersonFaceList:
    """Ansikts-rutenett for person-detalj (alle ansikter i klyngen)."""
    _require_person(conn, cluster_id)
    rows = conn.execute(
        """
        SELECT f.id AS face_id, f.image_id, i.content_hash AS hash, f.bbox, f.locked,
               i.width, i.height
        FROM faces f JOIN images i ON i.id = f.image_id
        WHERE f.cluster_id = ? AND i.excluded = 0
        ORDER BY f.id
        """,
        (cluster_id,),
    ).fetchall()
    items = [
        PersonFaceItem(
            face_id=r["face_id"],
            image_id=r["image_id"],
            hash=r["hash"],
            bbox=json.loads(r["bbox"]),
            locked=bool(r["locked"]),
            width=r["width"],
            height=r["height"],
        )
        for r in rows
    ]
    return PersonFaceList(items=items)


@router.get("/persons/{cluster_id}/images", response_model=ImageList)
def person_images(
    cluster_id: int,
    conn: sqlite3.Connection = Depends(get_conn),
    limit: int = Query(200, ge=1, le=1000),
    offset: int = Query(0, ge=0),
) -> ImageList:
    """Bilder personen finnes i (samme form som ``GET /api/images``)."""
    _require_person(conn, cluster_id)
    where, params = _build_filters(
        None, None, None, None, None, None, cluster_id, None, include_excluded=False
    )
    where_sql = ("WHERE " + " AND ".join(where)) if where else ""
    total = conn.execute(f"SELECT COUNT(*) FROM images i {where_sql}", params).fetchone()[0]
    rows = conn.execute(
        f"""
        SELECT i.id, i.content_hash, i.taken_at, i.year, i.season, i.place_name, i.excluded,
               (SELECT COUNT(*) FROM faces f WHERE f.image_id = i.id)        AS person_count,
               (SELECT COUNT(*) FROM image_tags it WHERE it.image_id = i.id) AS tag_count
        FROM images i
        {where_sql}
        ORDER BY i.taken_at IS NULL, i.taken_at DESC, i.id DESC
        LIMIT ? OFFSET ?
        """,
        [*params, limit, offset],
    ).fetchall()
    items = [
        ImageListItem(
            id=r["id"],
            hash=r["content_hash"],
            thumb_url=_thumb_url(r["content_hash"], "300"),
            taken_at=r["taken_at"],
            year=r["year"],
            season=r["season"],
            place_name=r["place_name"],
            person_count=r["person_count"],
            tag_count=r["tag_count"],
            excluded=bool(r["excluded"]),
        )
        for r in rows
    ]
    return ImageList(items=items, total=total)


@router.put("/persons/{cluster_id}", response_model=PersonRenameResult)
def rename_person(
    cluster_id: int, body: PersonRename, conn: sqlite3.Connection = Depends(get_conn)
) -> PersonRenameResult:
    """Navngi/omdøp en person. ``name=null`` fjerner navnet (klyngen består)."""
    name = body.name.strip() if body.name and body.name.strip() else None
    try:
        faces.set_person_name(conn, cluster_id, name)
    except ValueError as e:
        raise HTTPException(status_code=404, detail={"error": "not_found", "message": str(e)})
    return PersonRenameResult(cluster_id=cluster_id, name=name)


@router.post("/persons", response_model=PersonCreateResult)
def create_person(
    body: PersonCreate, conn: sqlite3.Connection = Depends(get_conn)
) -> PersonCreateResult:
    """Ny person fra et utvalg ansikter (splitt)."""
    name = body.name.strip() if body.name and body.name.strip() else None
    try:
        cluster_id = faces.create_person_from_faces(conn, body.face_ids, name)
    except ValueError as e:
        raise HTTPException(status_code=400, detail={"error": "bad_request", "message": str(e)})
    return PersonCreateResult(cluster_id=cluster_id)


@router.post("/persons/{cluster_id}/merge", response_model=PersonRenameResult)
def merge_person(
    cluster_id: int, body: PersonMerge, conn: sqlite3.Connection = Depends(get_conn)
) -> PersonRenameResult:
    """Slå sammen: flytt alle ansikter til ``into_cluster_id`` og slett denne klyngen."""
    try:
        faces.merge_persons(conn, cluster_id, body.into_cluster_id)
    except ValueError as e:
        raise HTTPException(status_code=400, detail={"error": "bad_request", "message": str(e)})
    row = conn.execute(
        "SELECT name FROM persons WHERE cluster_id = ?", (body.into_cluster_id,)
    ).fetchone()
    return PersonRenameResult(
        cluster_id=body.into_cluster_id, name=row["name"] if row else None
    )


@router.delete("/persons/{cluster_id}")
def remove_person(cluster_id: int, conn: sqlite3.Connection = Depends(get_conn)) -> dict[str, int]:
    """Slett klyngen; ansiktene løsnes (``cluster_id → NULL``)."""
    try:
        faces.delete_person(conn, cluster_id)
    except ValueError as e:
        raise HTTPException(status_code=404, detail={"error": "not_found", "message": str(e)})
    return {"deleted": cluster_id}


@router.post("/persons/{cluster_id}/ignore")
def ignore_person(cluster_id: int, conn: sqlite3.Connection = Depends(get_conn)) -> dict[str, int]:
    """Kryss bort en søppel-person permanent: lås + løsne ansiktene + slett klyngen.

    Ansiktene blir aldri re-klynget (``locked=1``), så personen gjenoppstår ikke
    ved neste klynging — i motsetning til vanlig sletting."""
    try:
        ignored = faces.ignore_person(conn, cluster_id)
    except ValueError as e:
        raise HTTPException(status_code=404, detail={"error": "not_found", "message": str(e)})
    return {"ignored": cluster_id, "faces": ignored}


@router.put("/faces/{face_id}", response_model=FacePatchResult)
def patch_face(
    face_id: int, body: FacePatch, conn: sqlite3.Connection = Depends(get_conn)
) -> FacePatchResult:
    """Flytt ansikt (``cluster_id``) og/eller frys/ignorer (``locked``).

    ``locked=true`` + ``cluster_id=null`` = ignorer søppel-ansikt: ``cluster()``
    re-tilordner det aldri. Bare felt som sendes i body anvendes.
    """
    fields = body.model_fields_set
    if not fields:
        raise HTTPException(
            status_code=400,
            detail={"error": "bad_request", "message": "ingen felt å oppdatere"},
        )
    try:
        if "cluster_id" in fields:
            faces.reassign_face(conn, face_id, body.cluster_id)
        if "locked" in fields:
            faces.set_face_locked(conn, face_id, bool(body.locked))
    except ValueError as e:
        raise HTTPException(status_code=404, detail={"error": "not_found", "message": str(e)})

    row = conn.execute(
        "SELECT cluster_id, locked FROM faces WHERE id = ?", (face_id,)
    ).fetchone()
    return FacePatchResult(
        id=face_id, cluster_id=row["cluster_id"], locked=bool(row["locked"])
    )
