"""Innstillinger + vedlikehold: nøkkel/modell/terskler, datamappe, klargjør-for-deling.

Vanlige settings (``ai_model``, terskler, ``default_export_dir``, OpenRouter-nøkkel)
går via ``fotomaxxer.settings`` — nøkkelen vises **alltid** maskert (``mask_value``).

**Datamappa er bootstrap, ikke en setting** (sirkulært — stien trengs for å åpne
DB-en): den leses/skrives via ``maintenance/data-dir`` mot pekerfila
(``~/.config/fotomaxxer/config.json``), aldri ``/api/settings``.
"""

from __future__ import annotations

import sqlite3
from pathlib import Path

from fastapi import APIRouter, Depends, HTTPException

from api.deps import get_conn, make_storage
from api.schemas import DataDir, DataDirMove, SettingPut, Settings
from fotomaxxer import settings as settings_mod, sources as sources_mod, storage

router = APIRouter(tags=["settings"])

_ALLOWED_KEYS = frozenset(
    {
        "ai_model",
        "face_match_threshold",
        "face_min_cluster_size",
        "default_export_dir",
        "default_profile_id",
        "openrouter_api_key",
    }
)


def _opt_int(value: str | None) -> int | None:
    """Parse en settings-streng til int, eller None hvis tom/ugyldig."""
    if value is None or not str(value).strip():
        return None
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


@router.get("/settings", response_model=Settings)
def get_settings(conn: sqlite3.Connection = Depends(get_conn)) -> Settings:
    merged = settings_mod.all_settings(conn)
    raw_key = settings_mod.get_setting(conn, "openrouter_api_key")
    return Settings(
        ai_model=merged.get("ai_model"),
        face_match_threshold=merged.get("face_match_threshold"),
        face_min_cluster_size=merged.get("face_min_cluster_size"),
        default_export_dir=settings_mod.get_setting(conn, "default_export_dir"),
        default_profile_id=_opt_int(settings_mod.get_setting(conn, "default_profile_id")),
        openrouter_api_key=settings_mod.mask_value("openrouter_api_key", raw_key),
    )


@router.put("/settings/{key}", response_model=Settings)
def put_setting(
    key: str, body: SettingPut, conn: sqlite3.Connection = Depends(get_conn)
) -> Settings:
    if key not in _ALLOWED_KEYS:
        raise HTTPException(
            status_code=400, detail={"error": "bad_key", "message": f"ukjent setting: {key}"}
        )
    settings_mod.set_setting(conn, key, body.value)
    return get_settings(conn)


@router.get("/maintenance/data-dir", response_model=DataDir)
def get_data_dir() -> DataDir:
    """Gjeldende datamappe (fra bootstrap-pekeren / env / cwd)."""
    return DataDir(data_dir=str(storage.resolve_root()))


@router.post("/maintenance/data-dir", response_model=DataDir)
def move_data_dir(body: DataDirMove) -> DataDir:
    """Kopiér db + derived til ny mappe og oppdater pekeren.

    Gammel mappe beholdes som standard; sett ``delete_source=true`` for å
    fjerne den etter at kopien er trygt på plass.
    """
    new_dir = Path(body.path).expanduser()
    try:
        storage.relocate_data(new_dir, delete_source=body.delete_source)
    except (OSError, ValueError) as e:
        raise HTTPException(
            status_code=400, detail={"error": "move_failed", "message": str(e)}
        )
    return DataDir(data_dir=str(storage.resolve_root()))


@router.post("/maintenance/prepare-handoff")
def prepare_handoff() -> dict[str, str]:
    """Tøm kilder + settings (inkl. nøkkel) så pakka kan deles. Disk-filer urørt."""
    conn = make_storage()
    db_conn = None
    try:
        from fotomaxxer import db

        db_conn = db.connect(conn.db_path)
        stats = sources_mod.prepare_for_handoff(db_conn, conn)
    finally:
        if db_conn is not None:
            db_conn.close()
    return {"message": f"Pakka er klar for deling. {stats.summary()}"}
