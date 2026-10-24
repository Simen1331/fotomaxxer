"""Derive-steg: nedskalerte arbeidskopier.

For hvert bilde med ``processing_state(stage='derive', status='pending')`` lages
to JPEG-er — lengste kant 1024 (AI + lightbox) og 300 (GUI-rutenett) — riktig
orientert (EXIF) og i sRGB. HEIC leses via ``pillow-heif``. Tungt I/O+CPU-arbeid
parallelliseres over en prosess-pool. Idempotent: ``done``-bilder hoppes over.
Én råtten fil stopper ikke jobben — den markeres ``error`` og resten fortsetter.

Bærbarhet: ``derived.path`` lagrer kun filnavnet (``<hash>_<size>.jpg``), aldri
en absolutt sti — resolves ved kjøretid via ``storage``.
"""

from __future__ import annotations

import os
import sqlite3
from concurrent.futures import ProcessPoolExecutor
from dataclasses import dataclass

import pillow_heif
from PIL import Image, ImageOps

from .storage import Storage

# Registreres i hver (også forked/spawned) prosess som importerer modulen,
# slik at Image.open leser HEIC både i hovedprosessen og i pool-arbeiderne.
pillow_heif.register_heif_opener()

SIZES: tuple[tuple[str, int], ...] = (("1024", 1024), ("300", 300))
_JPEG_QUALITY = 85
_COMMIT_EVERY = 16  # commit i batcher så /progress beveger seg live under jobben


@dataclass
class DeriveStats:
    pending: int = 0
    done: int = 0
    errors: int = 0

    def summary(self) -> str:
        return f"pending={self.pending} done={self.done} feil={self.errors}"


def _derive_one(job: tuple[int, str, list[tuple[str, str]]]) -> tuple[int, str | None]:
    """Pool-arbeider: les original, orientér, skriv hver størrelse. Ren av DB."""
    image_id, src, outputs = job
    try:
        with Image.open(src) as im:
            im.load()
            base = ImageOps.exif_transpose(im).convert("RGB")
        for out_path, target in outputs:
            copy = base.copy()
            copy.thumbnail((target, target), Image.LANCZOS)  # lengste kant ≤ mål, beholder aspekt
            copy.save(out_path, "JPEG", quality=_JPEG_QUALITY)
        return image_id, None
    except Exception as e:  # korrupt fil o.l. — feiler mykt
        return image_id, f"{type(e).__name__}: {e}"


def derive(
    conn: sqlite3.Connection,
    storage: Storage,
    workers: int | None = None,
    source_id: int | None = None,
) -> DeriveStats:
    """``source_id`` begrenser til bilder med en fil i den kilden (per-mappe-kjøring);
    ``None`` = alle pending."""
    storage.ensure_dirs()
    sql = """
        SELECT i.id AS image_id, i.content_hash AS content_hash,
               (SELECT f.path FROM files f WHERE f.image_id = i.id LIMIT 1) AS src
        FROM images i
        JOIN processing_state ps ON ps.image_id = i.id
        WHERE ps.stage = 'derive' AND ps.status = 'pending'
    """
    params: list[object] = []
    if source_id is not None:
        sql += " AND EXISTS (SELECT 1 FROM files f WHERE f.image_id = i.id AND f.source_id = ?)"
        params.append(source_id)
    sql += " ORDER BY i.id"
    rows = conn.execute(sql, params).fetchall()

    stats = DeriveStats(pending=len(rows))
    jobs: list[tuple[int, str, list[tuple[str, str]]]] = []
    hashes: dict[int, str] = {}

    for r in rows:
        if not r["src"] or not os.path.exists(r["src"]):
            _set_state(conn, r["image_id"], "error", "ingen lesbar kildefil")
            stats.errors += 1
            continue
        h = r["content_hash"]
        hashes[r["image_id"]] = h
        outputs = [(str(storage.derived_path(h, size)), target) for size, target in SIZES]
        jobs.append((r["image_id"], r["src"], outputs))

    if jobs:
        since_commit = 0
        with ProcessPoolExecutor(max_workers=workers) as pool:
            # Strøm resultater og commit i batcher, så processing_state (og dermed
            # /progress) beveger seg live i stedet for å hoppe fra 0 til alt når
            # hele poolen er ferdig.
            for image_id, error in pool.map(_derive_one, jobs):
                if error is not None:
                    _set_state(conn, image_id, "error", error)
                    stats.errors += 1
                else:
                    h = hashes[image_id]
                    for size, _target in SIZES:
                        _upsert_derived(conn, image_id, size, storage.derived_path(h, size).name)
                    _set_state(conn, image_id, "done", None)
                    stats.done += 1
                since_commit += 1
                if since_commit >= _COMMIT_EVERY:
                    conn.commit()
                    since_commit = 0

    conn.commit()
    return stats


def _upsert_derived(conn: sqlite3.Connection, image_id: int, size: str, filename: str) -> None:
    conn.execute(
        "INSERT INTO derived (image_id, size, path) VALUES (?, ?, ?) "
        "ON CONFLICT(image_id, size) DO UPDATE SET path = excluded.path",
        (image_id, size, filename),
    )


def _set_state(conn: sqlite3.Connection, image_id: int, status: str, error: str | None) -> None:
    conn.execute(
        "UPDATE processing_state SET status = ?, error = ?, updated_at = datetime('now') "
        "WHERE image_id = ? AND stage = 'derive'",
        (status, error, image_id),
    )
