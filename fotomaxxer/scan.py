"""Scan-steg: walk → whitelist → stat-fast-path → SHA-256 → dedup → images/files.

Per `source`, aldri implisitt over alt. Idempotent: re-scan gir kun arbeid på
nye/endrede filer (stat-fast-path mot ``scan_index`` hopper over uendrede). Nye
hasher gir en ny ``images``-rad med EXIF + geocode og ``processing_state`` pending
for derive/ai/faces. Kjente hasher dedupliseres. EXIF/geocode-feil stopper aldri
ingest — feltene blir bare NULL.
"""

from __future__ import annotations

import hashlib
import os
import sqlite3
from collections.abc import Callable
from dataclasses import dataclass, field

from .exif import ExifReader
from .geocode import Geocoder

WHITELIST = {".jpg", ".jpeg", ".png", ".heic", ".heif"}
_HASH_CHUNK = 1 << 20  # 1 MiB
_COMMIT_EVERY = 500  # flush scan_index jevnlig så re-scan er billig etter avbrudd
_PROGRESS_EVERY = 50  # rapportér fremdrift til kalleren såpass ofte (live GUI-teller)


@dataclass
class ScanStats:
    seen: int = 0          # whitelist-filer behandlet
    skipped_ext: int = 0   # filer utenfor whitelist
    hashed: int = 0        # faktisk lest + hashet
    fast_path: int = 0     # gjenbrukt hash via scan_index (ingen ny I/O)
    new_images: int = 0    # nye images-rader
    duplicates: int = 0    # kjent hash (dedup)
    files: int = 0         # files-rader upsertet
    errors: int = 0        # filer hoppet over pga. feil
    skipped_ext_types: dict[str, int] = field(default_factory=dict)

    def summary(self) -> str:
        return (
            f"sett={self.seen} hashet={self.hashed} fast-path={self.fast_path} "
            f"nye={self.new_images} duplikater={self.duplicates} "
            f"files={self.files} ikke-bilde={self.skipped_ext} feil={self.errors}"
        )


def scan_source(
    conn: sqlite3.Connection,
    source: sqlite3.Row,
    exif: ExifReader,
    geocoder: Geocoder,
    on_progress: Callable[[ScanStats], None] | None = None,
) -> ScanStats:
    """``on_progress`` kalles med løpende ``stats`` ca. hver ``_PROGRESS_EVERY``
    fil, så GUI-et kan vise en levende scan-teller (scan har ingen
    ``processing_state`` å lese fra under selve walken)."""
    stats = ScanStats()
    source_id = source["id"]
    root = source["root_path"]
    pending = 0

    for dirpath, _dirs, filenames in os.walk(root):
        for name in filenames:
            ext = os.path.splitext(name)[1].lower()
            if ext not in WHITELIST:
                stats.skipped_ext += 1
                stats.skipped_ext_types[ext] = stats.skipped_ext_types.get(ext, 0) + 1
                continue
            path = os.path.join(dirpath, name)
            try:
                st = os.stat(path)
            except OSError:
                stats.errors += 1
                continue
            stats.seen += 1

            content_hash = _fast_path_hash(conn, path, st)
            if content_hash is None:
                try:
                    content_hash = _hash_file(path)
                except OSError:
                    stats.errors += 1
                    continue
                _upsert_scan_index(conn, path, st, content_hash)
                stats.hashed += 1
            else:
                stats.fast_path += 1

            image_id = _image_id_for_hash(conn, content_hash)
            if image_id is None:
                image_id = _insert_image(conn, content_hash, path, exif, geocoder)
                stats.new_images += 1
            else:
                stats.duplicates += 1

            _upsert_file(conn, image_id, source_id, path)
            stats.files += 1

            pending += 1
            if pending >= _COMMIT_EVERY:
                conn.commit()
                pending = 0
            if on_progress is not None and stats.seen % _PROGRESS_EVERY == 0:
                on_progress(stats)

    if on_progress is not None:
        on_progress(stats)  # siste flush så telleren lander på sluttsummen

    # file_count lagres ikke — det utledes live ved lesing (sources._SELECT), så
    # tallet ikke kan gå stale når en overlappende kilde reassigner en fils source_id.
    conn.execute(
        "UPDATE sources SET last_scanned_at = datetime('now') WHERE id = ?",
        (source_id,),
    )
    conn.commit()
    return stats


def _fast_path_hash(conn: sqlite3.Connection, path: str, st: os.stat_result) -> str | None:
    row = conn.execute(
        "SELECT size, mtime, content_hash FROM scan_index WHERE path = ?", (path,)
    ).fetchone()
    if row and row["size"] == st.st_size and row["mtime"] == st.st_mtime:
        return row["content_hash"]
    return None


def _hash_file(path: str) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(_HASH_CHUNK), b""):
            h.update(chunk)
    return h.hexdigest()


def _upsert_scan_index(conn: sqlite3.Connection, path: str, st: os.stat_result, content_hash: str) -> None:
    conn.execute(
        "INSERT INTO scan_index (path, size, mtime, content_hash) VALUES (?, ?, ?, ?) "
        "ON CONFLICT(path) DO UPDATE SET size=excluded.size, mtime=excluded.mtime, "
        "content_hash=excluded.content_hash",
        (path, st.st_size, st.st_mtime, content_hash),
    )


def _image_id_for_hash(conn: sqlite3.Connection, content_hash: str) -> int | None:
    row = conn.execute(
        "SELECT id FROM images WHERE content_hash = ?", (content_hash,)
    ).fetchone()
    return row["id"] if row else None


def _insert_image(
    conn: sqlite3.Connection,
    content_hash: str,
    path: str,
    exif: ExifReader,
    geocoder: Geocoder,
) -> int:
    meta = exif.extract(path)
    lat, lon = meta.get("gps_lat"), meta.get("gps_lon")
    place_name = geocoder.lookup(lat, lon)
    cur = conn.execute(
        "INSERT INTO images (content_hash, width, height, taken_at, year, season, "
        "camera_make, camera_model, gps_lat, gps_lon, place_name) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        (
            content_hash,
            meta.get("width"),
            meta.get("height"),
            meta.get("taken_at"),
            meta.get("year"),
            meta.get("season"),
            meta.get("camera_make"),
            meta.get("camera_model"),
            lat,
            lon,
            place_name,
        ),
    )
    image_id = cur.lastrowid
    conn.executemany(
        "INSERT INTO processing_state (image_id, stage, status) VALUES (?, ?, 'pending')",
        [(image_id, stage) for stage in ("derive", "ai", "faces")],
    )
    return image_id


def _upsert_file(conn: sqlite3.Connection, image_id: int, source_id: int, path: str) -> None:
    conn.execute(
        "INSERT INTO files (image_id, source_id, path) VALUES (?, ?, ?) "
        "ON CONFLICT(path) DO UPDATE SET last_seen_at = datetime('now'), "
        "image_id = excluded.image_id, source_id = excluded.source_id",
        (image_id, source_id, path),
    )
