"""Tagging-profiler: navngitt, gjenbrukbar AI-merkings-konfig.

En profil bærer et fokus-direktiv + en ordnet liste kategorier. Kategoriene gjør
dobbel jobb: de former prompten (be modellen bøtte stikkord per kategori) og GUI-
gruppering (sidebar grupperer tags på kategori). En kilde peker på en profil via
``sources.profile_id``; tom = arv globalt ``settings['default_profile_id']``.

Rene ``(conn, …)``-funksjoner som resten av pipelinen — GUI/API kaller uendret.
``categories`` lagres som JSON-streng i DB-en; her tas/returneres den som ``list[str]``.
"""

from __future__ import annotations

import json
import sqlite3
from dataclasses import dataclass


class ProfileError(Exception):
    pass


@dataclass
class Profile:
    id: int
    name: str
    focus: str | None
    categories: list[str]


def _row_to_profile(row: sqlite3.Row) -> Profile:
    try:
        cats = json.loads(row["categories"]) if row["categories"] else []
    except (json.JSONDecodeError, TypeError):
        cats = []
    cats = [str(c).strip() for c in cats if str(c).strip()]
    return Profile(id=row["id"], name=row["name"], focus=row["focus"], categories=cats)


def _norm(name: str, focus: str | None, categories: list[str] | None) -> tuple[str, str | None, str]:
    name = (name or "").strip()
    if not name:
        raise ProfileError("Profilen må ha et navn.")
    focus = focus.strip() if focus and focus.strip() else None
    cats = [str(c).strip() for c in (categories or []) if str(c).strip()]
    # Dedup kategorier, bevar rekkefølge.
    cats = list(dict.fromkeys(cats))
    return name, focus, json.dumps(cats, ensure_ascii=False)


def list_profiles(conn: sqlite3.Connection) -> list[Profile]:
    rows = conn.execute(
        "SELECT id, name, focus, categories FROM tagging_profiles ORDER BY name"
    ).fetchall()
    return [_row_to_profile(r) for r in rows]


def get_profile(conn: sqlite3.Connection, profile_id: int) -> Profile | None:
    row = conn.execute(
        "SELECT id, name, focus, categories FROM tagging_profiles WHERE id = ?", (profile_id,)
    ).fetchone()
    return _row_to_profile(row) if row is not None else None


def create_profile(
    conn: sqlite3.Connection, name: str, focus: str | None, categories: list[str] | None
) -> Profile:
    name, focus, cats_json = _norm(name, focus, categories)
    try:
        with conn:
            cur = conn.execute(
                "INSERT INTO tagging_profiles (name, focus, categories) VALUES (?, ?, ?)",
                (name, focus, cats_json),
            )
    except sqlite3.IntegrityError:
        raise ProfileError(f"Det finnes allerede en profil med navnet «{name}».")
    profile = get_profile(conn, cur.lastrowid)
    assert profile is not None
    return profile


def update_profile(
    conn: sqlite3.Connection, profile_id: int, name: str, focus: str | None, categories: list[str] | None
) -> Profile:
    if get_profile(conn, profile_id) is None:
        raise ProfileError(f"Ukjent profil: {profile_id}")
    name, focus, cats_json = _norm(name, focus, categories)
    try:
        with conn:
            conn.execute(
                "UPDATE tagging_profiles SET name = ?, focus = ?, categories = ? WHERE id = ?",
                (name, focus, cats_json, profile_id),
            )
    except sqlite3.IntegrityError:
        raise ProfileError(f"Det finnes allerede en profil med navnet «{name}».")
    profile = get_profile(conn, profile_id)
    assert profile is not None
    return profile


def delete_profile(conn: sqlite3.Connection, profile_id: int) -> None:
    """Slett en profil. Kilder som peker på den nullstilles (FK ON DELETE SET NULL),
    og er den globalt standard fjernes den pekeren også."""
    if get_profile(conn, profile_id) is None:
        raise ProfileError(f"Ukjent profil: {profile_id}")
    with conn:
        conn.execute("DELETE FROM tagging_profiles WHERE id = ?", (profile_id,))
        conn.execute(
            "DELETE FROM settings WHERE key = 'default_profile_id' AND value = ?",
            (str(profile_id),),
        )
