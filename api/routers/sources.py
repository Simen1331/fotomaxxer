"""Kilder: list/legg til/fjern kilde-mapper.

Wrapper ``fotomaxxer.sources`` (``list_sources``/``add``/``remove``) uendret.
``add`` validerer at mappa finnes; ``remove`` rydder orphans men rører **aldri**
kilde-filer på disk. Selve scanningen skjer via pipeline-jobbene (``/api/jobs``).
"""

from __future__ import annotations

import sqlite3

from fastapi import APIRouter, Depends, HTTPException

from api.deps import get_conn, make_storage
from api.schemas import SourceCreate, SourceItem, SourceList, SourceProfileUpdate
from fotomaxxer import sources as sources_mod

router = APIRouter(tags=["sources"])


def _to_item(row: sqlite3.Row) -> SourceItem:
    return SourceItem(
        id=row["id"],
        root_path=row["root_path"],
        source_type=row["source_type"],
        last_scanned_at=row["last_scanned_at"],
        file_count=row["file_count"],
        profile_id=row["profile_id"],
    )


@router.get("/sources", response_model=SourceList)
def list_sources(conn: sqlite3.Connection = Depends(get_conn)) -> SourceList:
    return SourceList(items=[_to_item(r) for r in sources_mod.list_sources(conn)])


@router.post("/sources", response_model=SourceItem, status_code=201)
def add_source(body: SourceCreate, conn: sqlite3.Connection = Depends(get_conn)) -> SourceItem:
    try:
        row = sources_mod.add(conn, body.path, profile_id=body.profile_id)
    except sources_mod.SourceError as e:
        raise HTTPException(status_code=400, detail={"error": "bad_source", "message": str(e)})
    return _to_item(row)


@router.patch("/sources/{source_id}", response_model=SourceItem)
def set_source_profile(
    source_id: int, body: SourceProfileUpdate, conn: sqlite3.Connection = Depends(get_conn)
) -> SourceItem:
    """Sett/fjern per-kilde tagging-profil. Re-merker ikke automatisk (gated retag)."""
    try:
        row = sources_mod.set_profile(conn, source_id, body.profile_id)
    except sources_mod.SourceError as e:
        raise HTTPException(status_code=404, detail={"error": "not_found", "message": str(e)})
    return _to_item(row)


@router.delete("/sources/{source_id}")
def remove_source(
    source_id: int, conn: sqlite3.Connection = Depends(get_conn)
) -> dict[str, object]:
    try:
        stats = sources_mod.remove(conn, make_storage(), source_id)
    except sources_mod.SourceError as e:
        raise HTTPException(status_code=404, detail={"error": "not_found", "message": str(e)})
    return {"removed": source_id, "summary": stats.summary()}
