"""Delte avhengigheter: pakke-rot, Storage og en connection-per-request.

Skilt ut fra ``app.py`` så routerne kan importere ``get_conn`` uten en sirkulær
import (app importerer routerne, routerne importerer denne).
"""

from __future__ import annotations

import sqlite3
from collections.abc import Iterator

from fotomaxxer import db, storage


def make_storage() -> storage.Storage:
    """Storage rotet i den gjeldende datamappa (bootstrap-peker > env > cwd)."""
    return storage.Storage(storage.resolve_root())


def get_conn() -> Iterator[sqlite3.Connection]:
    """FastAPI-dependency: én connection per request, skjemaet sikret (idempotent).

    ``init_db`` kjører her, så en v1-DB migreres til v2 ved første kall.
    Connection lukkes alltid når requesten er ferdig. Bakgrunnsjobber bruker
    IKKE denne — de åpner sin egen connection (jf. én-jobb-kjøreren).
    """
    store = make_storage()
    conn = db.connect(store.db_path)
    db.init_db(conn)
    try:
        yield conn
    finally:
        conn.close()
