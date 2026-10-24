"""Bruker-konfig i ``settings``-tabellen.

Nøkler som ``openrouter_api_key`` og ``ai_model`` lever her, ikke i ``.env`` —
slik at GUI senere kan skrive dem og pakka forblir flyttbar. Hemmeligheter
maskeres ved *visning* (``mask_value``), men lagres i klartekst i den lokale
DB-en (``prepare-handoff`` tømmer tabellen før deling).
"""

from __future__ import annotations

import sqlite3

# Defaults som gjelder når nøkkelen ikke er satt i DB-en.
DEFAULTS: dict[str, str] = {
    "ai_model": "google/gemini-2.5-flash",
    "face_match_threshold": "0.5",
    "face_min_cluster_size": "3",
}

# Nøkler hvis verdi aldri skal skrives ut i klartekst.
SECRET_KEYS: frozenset[str] = frozenset({"openrouter_api_key"})


def get_setting(conn: sqlite3.Connection, key: str) -> str | None:
    """Verdien for ``key``, eller default, eller ``None`` hvis ukjent."""
    row = conn.execute("SELECT value FROM settings WHERE key = ?", (key,)).fetchone()
    if row is not None and row["value"] is not None:
        return row["value"]
    return DEFAULTS.get(key)


def set_setting(conn: sqlite3.Connection, key: str, value: str) -> None:
    """Skriv (upsert) en setting og commit."""
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?, ?) "
        "ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        (key, value),
    )
    conn.commit()


def all_settings(conn: sqlite3.Connection) -> dict[str, str]:
    """Alle satte settings slått sammen med defaults (rå verdier, ingen maskering)."""
    merged = dict(DEFAULTS)
    for row in conn.execute("SELECT key, value FROM settings ORDER BY key").fetchall():
        if row["value"] is not None:
            merged[row["key"]] = row["value"]
    return merged


def mask_value(key: str, value: str | None) -> str:
    """Maskér hemmelige verdier for visning; vis resten som de er."""
    if value is None:
        return "(ikke satt)"
    if key in SECRET_KEYS:
        if len(value) <= 8:
            return "••••"
        return f"{value[:4]}…{value[-4:]}"
    return value
