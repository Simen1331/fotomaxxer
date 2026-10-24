"""Tagging-profiler: list/opprett/rediger/slett.

Wrapper ``fotomaxxer.profiles`` uendret. En profil bærer fokus-direktiv +
kategorier; kilder peker på den via ``profile_id``, globalt standard ligger i
``settings['default_profile_id']``. Sletting nullstiller pekende kilder (FK).
"""

from __future__ import annotations

import sqlite3

from fastapi import APIRouter, Depends, HTTPException

from api.deps import get_conn
from api.schemas import ProfileCreate, ProfileItem, ProfileList, ProfileUpdate
from fotomaxxer import profiles as profiles_mod

router = APIRouter(tags=["profiles"])


def _to_item(p: profiles_mod.Profile) -> ProfileItem:
    return ProfileItem(id=p.id, name=p.name, focus=p.focus, categories=p.categories)


@router.get("/profiles", response_model=ProfileList)
def list_profiles(conn: sqlite3.Connection = Depends(get_conn)) -> ProfileList:
    return ProfileList(items=[_to_item(p) for p in profiles_mod.list_profiles(conn)])


@router.post("/profiles", response_model=ProfileItem, status_code=201)
def create_profile(
    body: ProfileCreate, conn: sqlite3.Connection = Depends(get_conn)
) -> ProfileItem:
    try:
        p = profiles_mod.create_profile(conn, body.name, body.focus, body.categories)
    except profiles_mod.ProfileError as e:
        raise HTTPException(status_code=400, detail={"error": "bad_profile", "message": str(e)})
    return _to_item(p)


@router.put("/profiles/{profile_id}", response_model=ProfileItem)
def update_profile(
    profile_id: int, body: ProfileUpdate, conn: sqlite3.Connection = Depends(get_conn)
) -> ProfileItem:
    try:
        p = profiles_mod.update_profile(conn, profile_id, body.name, body.focus, body.categories)
    except profiles_mod.ProfileError as e:
        raise HTTPException(status_code=400, detail={"error": "bad_profile", "message": str(e)})
    return _to_item(p)


@router.delete("/profiles/{profile_id}")
def delete_profile(
    profile_id: int, conn: sqlite3.Connection = Depends(get_conn)
) -> dict[str, object]:
    try:
        profiles_mod.delete_profile(conn, profile_id)
    except profiles_mod.ProfileError as e:
        raise HTTPException(status_code=404, detail={"error": "not_found", "message": str(e)})
    return {"deleted": profile_id}
