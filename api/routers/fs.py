"""Filsystem-browse: list undermapper så GUI-et kan velge en kilde-mappe.

Read-only og bevisst enkelt: FotoMaxxer er en single-user lokal-app bundet til
127.0.0.1, så å bla i brukerens egne mapper er greit. Vi lister **kun mapper**
(man velger en mappe, ikke en fil), hopper skjulte (``.``-prefiks) og lar
``parent`` peke ett nivå opp (``None`` på rota). Ingen skriving, ingen sletting.
"""

from __future__ import annotations

from pathlib import Path

from fastapi import APIRouter, HTTPException

from api.schemas import FsEntry, FsList

router = APIRouter(tags=["fs"])


@router.get("/fs/list", response_model=FsList)
def fs_list(path: str | None = None) -> FsList:
    """List undermapper i ``path`` (standard: brukerens hjemmemappe)."""
    base = (Path(path).expanduser() if path else Path.home()).resolve()
    if not base.is_dir():
        raise HTTPException(
            status_code=400,
            detail={"error": "not_a_dir", "message": f"Ikke en mappe: {base}"},
        )
    try:
        dirs = sorted(
            (p for p in base.iterdir() if p.is_dir() and not p.name.startswith(".")),
            key=lambda p: p.name.lower(),
        )
    except PermissionError:
        raise HTTPException(
            status_code=403,
            detail={"error": "denied", "message": f"Ingen tilgang: {base}"},
        )
    parent = None if base.parent == base else str(base.parent)
    return FsList(
        path=str(base),
        parent=parent,
        entries=[FsEntry(name=p.name, path=str(p)) for p in dirs],
    )
