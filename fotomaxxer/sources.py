"""Kilde-mapper: legg til, list, fjern + klargjør pakke for handoff.

En ``source`` er en mappe brukeren vil indeksere. **Jernregel: kilde-filer på disk
røres aldri herfra.** ``remove`` sletter kun appens egne data — DB-rader og de
avledede ``derived/<hash>_*.jpg``-filene — med orphan-opprydding for bilder som
mister alle sine fysiske kopier.
"""

from __future__ import annotations

import os
import sqlite3
from dataclasses import dataclass
from pathlib import Path

from .storage import Storage


class SourceError(Exception):
    pass


# Alle kilde-lesninger går via denne. ``file_count`` beregnes **live** som en
# korrelert COUNT av ``files`` — ikke et lagret øyeblikksbilde. Grunnen: legger
# man til en kilde som overlapper en eksisterende (forelder/barn-mappe),
# reassignes de delte filenes ``source_id`` til den sist-scannede kilden (path er
# UNIQUE), og et lagret snapshot på taperen ville da lyve. Live-telling stemmer
# alltid. (Kolonnen ``sources.file_count`` er nå vestigial — kan droppes i en
# senere data-model-revisjon; se Blueprint-flagg.)
_SELECT = (
    "SELECT id, root_path, source_type, added_at, last_scanned_at, profile_id, "
    "(SELECT COUNT(*) FROM files WHERE files.source_id = sources.id) AS file_count "
    "FROM sources"
)


@dataclass
class RemoveStats:
    sources_removed: int = 0
    files_removed: int = 0
    scan_index_removed: int = 0
    images_removed: int = 0      # orphans ryddet helt
    derived_files_removed: int = 0  # fysiske thumbnails slettet fra disk

    def __iadd__(self, other: "RemoveStats") -> "RemoveStats":
        self.sources_removed += other.sources_removed
        self.files_removed += other.files_removed
        self.scan_index_removed += other.scan_index_removed
        self.images_removed += other.images_removed
        self.derived_files_removed += other.derived_files_removed
        return self

    def summary(self) -> str:
        return (
            f"kilder={self.sources_removed} files={self.files_removed} "
            f"scan_index={self.scan_index_removed} orphan-bilder={self.images_removed} "
            f"thumbnails={self.derived_files_removed}"
        )


def add(
    conn: sqlite3.Connection,
    root: str | Path,
    source_type: str = "local",
    profile_id: int | None = None,
) -> sqlite3.Row:
    """Registrer en kilde-mappe. ``root`` lagres som absolutt sti.

    ``profile_id`` (valgfritt) peker på en tagging-profil; NULL = arv globalt
    ``settings['default_profile_id']``."""
    path = Path(root).expanduser().resolve()
    if not path.exists():
        raise SourceError(f"Mappa finnes ikke: {path}")
    if not path.is_dir():
        raise SourceError(f"Ikke en mappe: {path}")
    root_str = str(path)
    try:
        with conn:
            conn.execute(
                "INSERT INTO sources (root_path, source_type, profile_id) VALUES (?, ?, ?)",
                (root_str, source_type, profile_id),
            )
    except sqlite3.IntegrityError:
        raise SourceError(f"Kilden er allerede lagt til: {root_str}")
    return get_by_path(conn, root_str)


def set_profile(
    conn: sqlite3.Connection, target: str | int, profile_id: int | None
) -> sqlite3.Row:
    """Sett (eller fjern) per-kilde tagging-profil. NULL = arv globalt.

    Endrer kun ``sources.profile_id`` — bilder merkes ikke på nytt automatisk
    (penge-disiplin); re-merking går via den gated ``tag(--retag, source_id)``-flyten."""
    src = _resolve_target(conn, target)
    if src is None:
        raise SourceError(f"Ukjent kilde: {target}")
    with conn:
        conn.execute(
            "UPDATE sources SET profile_id = ? WHERE id = ?",
            (profile_id, src["id"]),
        )
    return _resolve_target(conn, src["id"])


def list_sources(conn: sqlite3.Connection) -> list[sqlite3.Row]:
    return conn.execute(_SELECT + " ORDER BY id").fetchall()


def get_by_path(conn: sqlite3.Connection, root: str | Path) -> sqlite3.Row | None:
    path = str(Path(root).expanduser().resolve())
    return conn.execute(_SELECT + " WHERE root_path = ?", (path,)).fetchone()


def _resolve_target(conn: sqlite3.Connection, target: str | int) -> sqlite3.Row | None:
    s = str(target)
    if s.isdigit():
        return conn.execute(_SELECT + " WHERE id = ?", (int(s),)).fetchone()
    return get_by_path(conn, s)


def _like_escape(s: str) -> str:
    return s.replace("\\", "\\\\").replace("%", "\\%").replace("_", "\\_")


def remove(conn: sqlite3.Connection, storage: Storage, target: str | int) -> RemoveStats:
    """Fjern en kilde + dens app-data. Kilde-filer på disk røres ALDRI.

    1. Slett ``sources``-raden (``files`` cascade-slettes via FK) + ``scan_index``
       under roten.
    2. Orphan-opprydding: bilder som mister *alle* ``files`` slettes helt
       (``images`` cascade → ``derived``/``ai_tags``/``image_tags``/``faces``/
       ``processing_state``/``album_items``) **og** de fysiske
       ``derived/<hash>_*.jpg`` på disk. Bilder med kopier i andre kilder blir
       liggende.
    """
    src = _resolve_target(conn, target)
    if src is None:
        raise SourceError(f"Ukjent kilde: {target}")
    source_id = src["id"]
    root = src["root_path"]
    stats = RemoveStats(sources_removed=1)

    candidate_image_ids = [
        r["image_id"]
        for r in conn.execute(
            "SELECT DISTINCT image_id FROM files WHERE source_id = ?", (source_id,)
        )
    ]
    stats.files_removed = conn.execute(
        "SELECT COUNT(*) FROM files WHERE source_id = ?", (source_id,)
    ).fetchone()[0]

    prefix = root.rstrip(os.sep) + os.sep

    with conn:
        # files cascade-slettes når sources-raden går
        conn.execute("DELETE FROM sources WHERE id = ?", (source_id,))
        cur = conn.execute(
            "DELETE FROM scan_index WHERE path LIKE ? ESCAPE '\\'",
            (_like_escape(prefix) + "%",),
        )
        stats.scan_index_removed = cur.rowcount

        for image_id in candidate_image_ids:
            still_referenced = conn.execute(
                "SELECT 1 FROM files WHERE image_id = ? LIMIT 1", (image_id,)
            ).fetchone()
            if still_referenced is not None:
                continue  # kopi finnes i annen kilde — la bildet ligge
            # orphan: slett fysiske thumbnails før raden (basename → resolv via storage)
            for drow in conn.execute(
                "SELECT path FROM derived WHERE image_id = ?", (image_id,)
            ):
                fp = storage.derived_dir / drow["path"]
                if fp.exists():
                    fp.unlink()
                    stats.derived_files_removed += 1
            conn.execute("DELETE FROM images WHERE id = ?", (image_id,))
            stats.images_removed += 1

    return stats


def prepare_for_handoff(conn: sqlite3.Connection, storage: Storage) -> RemoveStats:
    """Tøm pakka for deling: fjern alle kilder (med opprydding) + tøm settings.

    Etterpå inneholder pakka ingen av brukerens mapper, thumbnails eller nøkler —
    klar til å gis videre. Kilde-originalene på disk er urørt.

    Til slutt ``VACUUM`` så slettede rader ikke overlever som lesbare bytes i
    frilistesider: SQLite nuller ikke frigjort innhold med mindre ``secure_delete``
    er på (og det er en platform-default vi ikke kan stole på). VACUUM bygger
    db-fila på nytt og kaster frilisten helt — da kan ikke en avslørt API-nøkkel
    graves ut av ``fotomaxxer.db`` med ``strings`` etter handoff.
    """
    total = RemoveStats()
    for src in list_sources(conn):
        total += remove(conn, storage, src["id"])
    with conn:
        conn.execute("DELETE FROM settings")
    # VACUUM kan ikke kjøre i en åpen transaksjon — commit først (with-blokka over
    # har alt commitet, men vær eksplisitt), så kompaktér utenfor transaksjon.
    conn.commit()
    conn.execute("VACUUM")
    return total
