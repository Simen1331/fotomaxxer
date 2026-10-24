"""Eksport til mappe: kopier fulloppløselige originaler, nummerert.

Den eneste fil-skrivende operasjonen for album. Kopierer originalen (``files.path``,
én per bilde) nummerert etter album-``position`` → ``001_<originalnavn>.jpg`` i en
undermappe oppkalt etter albumet. Manglende original hoppes over og rapporteres.
Kjøres som bakgrunnsjobb via den delte jobb-kjøreren (én jobb om gangen).
**PDF er ikke her** — det blir en egen TB.
"""

from __future__ import annotations

import re
import shutil
import sqlite3
from pathlib import Path

from fastapi import APIRouter, Depends, HTTPException

from api.deps import get_conn, make_storage
from api.jobs import JobBusy, JobRunner, runner
from api.schemas import ExportRequest, JobAccepted
from fotomaxxer import db, settings as settings_mod

router = APIRouter(tags=["export"])

_SAFE = re.compile(r"[^\w\-. æøåÆØÅ]+")


def _safe_dirname(name: str) -> str:
    cleaned = _SAFE.sub("_", name).strip().strip(".")
    return cleaned or "album"


def _resolve_targets(
    conn: sqlite3.Connection, req: ExportRequest
) -> tuple[list[int], str | None]:
    """Returner (ordnede image_ids, undermappe-navn|None)."""
    if req.album_id is not None:
        album = conn.execute("SELECT name FROM albums WHERE id = ?", (req.album_id,)).fetchone()
        if album is None:
            raise HTTPException(status_code=404, detail={"error": "not_found", "message": "album finnes ikke"})
        rows = conn.execute(
            "SELECT image_id FROM album_items WHERE album_id = ? ORDER BY position, image_id",
            (req.album_id,),
        ).fetchall()
        return [r["image_id"] for r in rows], _safe_dirname(album["name"])
    return list(req.image_ids or []), None


@router.post("/export", response_model=JobAccepted, status_code=202)
def export(req: ExportRequest, conn: sqlite3.Connection = Depends(get_conn)) -> JobAccepted:
    if req.album_id is None and not req.image_ids:
        raise HTTPException(
            status_code=400,
            detail={"error": "bad_request", "message": "oppgi album_id eller image_ids"},
        )

    ordered_ids, subdir = _resolve_targets(conn, req)
    if not ordered_ids:
        raise HTTPException(
            status_code=400, detail={"error": "bad_request", "message": "ingen bilder å eksportere"}
        )

    dest_root = req.dest_path or settings_mod.get_setting(conn, "default_export_dir")
    if not dest_root:
        raise HTTPException(
            status_code=400,
            detail={
                "error": "no_dest",
                "message": "ingen mål-mappe — sett en eller velg standard eksport-mappe i Innstillinger",
            },
        )

    keep_names = req.keep_original_names
    dest_base = Path(dest_root).expanduser()

    def job(run: JobRunner) -> None:
        run.update(step="kopierer")
        jconn = db.connect(make_storage().db_path)
        try:
            target_dir = dest_base / subdir if subdir else dest_base
            target_dir.mkdir(parents=True, exist_ok=True)
            total = len(ordered_ids)
            copied = 0
            skipped = 0
            run.update(progress={"copied": 0, "skipped": 0, "total": total})
            for i, image_id in enumerate(ordered_ids, start=1):
                frow = jconn.execute(
                    "SELECT path FROM files WHERE image_id = ? ORDER BY id LIMIT 1", (image_id,)
                ).fetchone()
                src = Path(frow["path"]) if frow else None
                if src is None or not src.exists():
                    skipped += 1
                    run.update(progress={"copied": copied, "skipped": skipped, "total": total})
                    continue
                name = src.name if keep_names else f"{i:03d}_{src.name}"
                shutil.copy2(src, target_dir / name)
                copied += 1
                run.update(progress={"copied": copied, "skipped": skipped, "total": total})
            run.update(message=f"Eksporterte {copied} bilder til {target_dir}" + (f", hoppet over {skipped}" if skipped else ""))
        finally:
            jconn.close()

    try:
        job_id = runner.start("export", job)
    except JobBusy:
        raise HTTPException(
            status_code=409,
            detail={"error": "busy", "message": "en jobb kjører allerede — vent til den er ferdig"},
        )
    return JobAccepted(job_id=job_id, kind="export")
