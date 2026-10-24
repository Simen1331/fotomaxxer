"""SQLite-tilkobling og skjema for FotoMaxxer.

Skjemaet er autoritativt definert i Technical Brief
(``tb-fotomaxxer-data-model-v1``). DDL-en her speiler den 1:1, men med
``IF NOT EXISTS`` slik at ``init_db`` er idempotent. Versjonering via
``PRAGMA user_version`` holder rom for senere migrasjoner.
"""

from __future__ import annotations

import os
import sqlite3
from pathlib import Path

SCHEMA_VERSION = 5

# Full DDL fra TB Data Model v1. Alt med IF NOT EXISTS → idempotent.
SCHEMA_SQL = """
CREATE TABLE IF NOT EXISTS tagging_profiles (       -- [v5] navngitt, gjenbrukbar tagging-profil
  id              INTEGER PRIMARY KEY,
  name            TEXT NOT NULL UNIQUE,             -- "Design & håndverk"
  focus           TEXT,                             -- fokus-direktivteksten (kan være tom)
  categories      TEXT NOT NULL DEFAULT '[]',       -- JSON ordnet liste: ["materiale","teknikk",...]
  created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS sources (
  id              INTEGER PRIMARY KEY,
  root_path       TEXT NOT NULL UNIQUE,
  source_type     TEXT NOT NULL DEFAULT 'local',   -- 'local' | 'dropbox'
  added_at        TEXT NOT NULL DEFAULT (datetime('now')),
  last_scanned_at TEXT,                             -- NULL = aldri scannet
  file_count      INTEGER NOT NULL DEFAULT 0,
  focus           TEXT,                             -- [v4] vestigial — avløst av profile_id (v5), beholdt for migrasjon
  profile_id      INTEGER REFERENCES tagging_profiles(id) ON DELETE SET NULL  -- [v5] NULL = arv globalt default_profile_id
);

CREATE TABLE IF NOT EXISTS scan_index (             -- rask forhåndssjekk, unngår re-hashing
  path            TEXT PRIMARY KEY,
  size            INTEGER NOT NULL,
  mtime           REAL NOT NULL,
  content_hash    TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS settings (               -- bruker-konfig, skrives fra GUI
  key             TEXT PRIMARY KEY,                 -- f.eks. 'openrouter_api_key', 'ai_model'
  value           TEXT
);

CREATE TABLE IF NOT EXISTS images (
  id              INTEGER PRIMARY KEY,
  content_hash    TEXT NOT NULL UNIQUE,             -- SHA-256 hex
  owner_id        TEXT NOT NULL DEFAULT 'bjorn',
  width           INTEGER,
  height          INTEGER,
  taken_at        TEXT,                             -- ISO8601 fra EXIF DateTimeOriginal
  year            INTEGER,                          -- avledet fra taken_at
  season          TEXT,                             -- 'vinter'|'vår'|'sommer'|'høst'
  camera_make     TEXT,
  camera_model    TEXT,
  gps_lat         REAL,
  gps_lon         REAL,
  place_name      TEXT,                             -- Kartverket-geocodet
  excluded        INTEGER NOT NULL DEFAULT 0,       -- [v2] 1 = skjult fra vyer (mykt, reversibelt)
  created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS files (                  -- alle fysiske kopier av et bilde
  id              INTEGER PRIMARY KEY,
  image_id        INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
  source_id       INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
  path            TEXT NOT NULL UNIQUE,
  last_seen_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS derived (                -- nedskalerte arbeidskopier
  id              INTEGER PRIMARY KEY,
  image_id        INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
  size            TEXT NOT NULL,                    -- '1024' | '300'
  path            TEXT NOT NULL,
  UNIQUE(image_id, size)
);

CREATE TABLE IF NOT EXISTS processing_state (       -- idempotens per steg
  image_id        INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
  stage           TEXT NOT NULL,                    -- 'derive'|'ai'|'faces'
  status          TEXT NOT NULL DEFAULT 'pending',  -- 'pending'|'done'|'error'
  error           TEXT,
  updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
  PRIMARY KEY (image_id, stage)
);

-- Defineres her, fylles av andre TB-er:
CREATE TABLE IF NOT EXISTS ai_tags (
  id              INTEGER PRIMARY KEY,
  image_id        INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
  provider        TEXT NOT NULL,
  scene           TEXT,
  event_type      TEXT,
  season_ai       TEXT,
  indoor_outdoor  TEXT,
  person_count    INTEGER,
  description     TEXT,
  raw_json        TEXT,
  cost            REAL,                             -- [v3] faktisk USD-kost fra OpenRouter usage.cost (NULL = ukjent)
  tagged_at       TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS tags (
  id              INTEGER PRIMARY KEY,
  name            TEXT NOT NULL UNIQUE,
  category        TEXT                              -- [v5] kategori fra tagging-profil; NULL = ukategorisert/generisk
);
CREATE TABLE IF NOT EXISTS image_tags (
  image_id        INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
  tag_id          INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
  PRIMARY KEY (image_id, tag_id)
);

CREATE TABLE IF NOT EXISTS persons (
  cluster_id      INTEGER PRIMARY KEY,
  name            TEXT                              -- NULL = ikke navngitt
);
CREATE TABLE IF NOT EXISTS faces (
  id              INTEGER PRIMARY KEY,
  image_id        INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
  bbox            TEXT NOT NULL,                    -- JSON [x,y,w,h] i originalkoordinater
  embedding       BLOB NOT NULL,                   -- float32-vektor
  cluster_id      INTEGER REFERENCES persons(cluster_id) ON DELETE SET NULL,
  locked          INTEGER NOT NULL DEFAULT 0        -- [v2] 1 = frosset/ignorert; cluster() rører det aldri
);

CREATE TABLE IF NOT EXISTS albums (
  id              INTEGER PRIMARY KEY,
  name            TEXT NOT NULL,
  created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE TABLE IF NOT EXISTS album_items (
  album_id        INTEGER NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
  image_id        INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
  position        INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (album_id, image_id)
);

-- Indekser
CREATE INDEX IF NOT EXISTS idx_images_taken_at   ON images(taken_at);
CREATE INDEX IF NOT EXISTS idx_images_year       ON images(year);
CREATE INDEX IF NOT EXISTS idx_files_image       ON files(image_id);
CREATE INDEX IF NOT EXISTS idx_procstate_lookup  ON processing_state(stage, status);
CREATE INDEX IF NOT EXISTS idx_faces_cluster     ON faces(cluster_id);
CREATE INDEX IF NOT EXISTS idx_aitags_image      ON ai_tags(image_id);
CREATE INDEX IF NOT EXISTS idx_images_excluded   ON images(excluded);   -- [v2]
"""


def connect(db_path: str | os.PathLike[str]) -> sqlite3.Connection:
    """Åpne en tilkobling med foreign keys på og rad-tilgang via navn.

    ``check_same_thread=False``: FastAPI kjører sync-avhengigheter i en tråd-pool,
    og en generator-dependency (``get_conn``) kan kjøre teardown (``conn.close()``)
    på en *annen* tråd enn den som opprettet connectionen. Hver request og hver
    bakgrunnsjobb har sin **egen** connection som aldri brukes samtidig fra flere
    tråder, så det er trygt å droppe tråd-sjekken (ellers: ProgrammingError ved close).
    """
    conn = sqlite3.connect(str(db_path), check_same_thread=False)
    conn.row_factory = sqlite3.Row
    conn.execute("PRAGMA foreign_keys = ON")
    return conn


def _column_exists(conn: sqlite3.Connection, table: str, column: str) -> bool:
    """True hvis ``table`` har ``column`` (for idempotente ALTER-migrasjoner)."""
    return any(r["name"] == column for r in conn.execute(f"PRAGMA table_info({table})"))


def _migrate_v2(conn: sqlite3.Connection) -> None:
    """v1 → v2 (GUI-grill): faces.locked + images.excluded + idx_images_excluded.

    Idempotent: hopper over ALTER hvis kolonnen alt finnes, slik at en delvis
    kjørt migrasjon kan repeteres trygt.
    """
    if not _column_exists(conn, "faces", "locked"):
        conn.execute("ALTER TABLE faces ADD COLUMN locked INTEGER NOT NULL DEFAULT 0")
    if not _column_exists(conn, "images", "excluded"):
        conn.execute("ALTER TABLE images ADD COLUMN excluded INTEGER NOT NULL DEFAULT 0")
    conn.execute("CREATE INDEX IF NOT EXISTS idx_images_excluded ON images(excluded)")


def _migrate_v3(conn: sqlite3.Connection) -> None:
    """v2 → v3: ai_tags.cost (faktisk USD-kost per AI-merking fra OpenRouter).

    Idempotent: hopper over ALTER hvis kolonnen alt finnes."""
    if not _column_exists(conn, "ai_tags", "cost"):
        conn.execute("ALTER TABLE ai_tags ADD COLUMN cost REAL")


def _migrate_v4(conn: sqlite3.Connection) -> None:
    """v3 → v4: sources.focus (per-kilde tagging-fokus; NULL = arv globalt).

    Idempotent: hopper over ALTER hvis kolonnen alt finnes."""
    if not _column_exists(conn, "sources", "focus"):
        conn.execute("ALTER TABLE sources ADD COLUMN focus TEXT")


def _profile_for_focus(conn: sqlite3.Connection, focus: str) -> int:
    """Hent (eller opprett) en tagging-profil for en fritekst-fokus. Dedup på focus."""
    focus = focus.strip()
    row = conn.execute(
        "SELECT id FROM tagging_profiles WHERE focus = ?", (focus,)
    ).fetchone()
    if row is not None:
        return row[0]
    # Navn: avled fra teksten, fall tilbake til nummerert hvis kollisjon/tomt.
    base = (focus[:40].rsplit(" ", 1)[0] or "Importert fokus").strip()
    name = base
    n = 1
    while conn.execute("SELECT 1 FROM tagging_profiles WHERE name = ?", (name,)).fetchone():
        n += 1
        name = f"{base} ({n})"
    cur = conn.execute(
        "INSERT INTO tagging_profiles (name, focus, categories) VALUES (?, ?, '[]')",
        (name, focus),
    )
    return cur.lastrowid


def _migrate_v5(conn: sqlite3.Connection) -> None:
    """v4 → v5: tagging_profiles + sources.profile_id + tags.category.

    Data-bevarende: hver distinkt ikke-tom ``sources.focus`` importeres som en
    profil, og kilden re-pekes via ``profile_id``. ``settings['tagging_focus']``
    blir en profil pekt av ``settings['default_profile_id']``. ``sources.focus``
    beholdes kolonne-fysisk (vestigial) men leses ikke lenger. Idempotent."""
    conn.execute(
        """CREATE TABLE IF NOT EXISTS tagging_profiles (
             id INTEGER PRIMARY KEY,
             name TEXT NOT NULL UNIQUE,
             focus TEXT,
             categories TEXT NOT NULL DEFAULT '[]',
             created_at TEXT NOT NULL DEFAULT (datetime('now'))
           )"""
    )
    if not _column_exists(conn, "sources", "profile_id"):
        conn.execute(
            "ALTER TABLE sources ADD COLUMN profile_id INTEGER "
            "REFERENCES tagging_profiles(id) ON DELETE SET NULL"
        )
    if not _column_exists(conn, "tags", "category"):
        conn.execute("ALTER TABLE tags ADD COLUMN category TEXT")

    # Importer per-kilde fokus → profil (kun rader som ennå ikke er re-pekt).
    if _column_exists(conn, "sources", "focus"):
        rows = conn.execute(
            "SELECT id, focus FROM sources "
            "WHERE focus IS NOT NULL AND TRIM(focus) <> '' AND profile_id IS NULL"
        ).fetchall()
        for sid, focus in rows:
            pid = _profile_for_focus(conn, focus)
            conn.execute("UPDATE sources SET profile_id = ? WHERE id = ?", (pid, sid))

    # Globalt fokus → default-profil.
    gf = conn.execute("SELECT value FROM settings WHERE key = 'tagging_focus'").fetchone()
    if gf is not None and gf[0] and gf[0].strip():
        pid = _profile_for_focus(conn, gf[0])
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('default_profile_id', ?) "
            "ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            (str(pid),),
        )


# Trinnvise migrasjoner: versjon N → SQL som tar skjemaet fra N-1 til N.
# En fersk DB bygges direkte fra SCHEMA_SQL (alt på siste versjon); en
# eksisterende DB kjører kun trinnene mellom sin versjon og SCHEMA_VERSION.
MIGRATIONS = {
    2: _migrate_v2,
    3: _migrate_v3,
    4: _migrate_v4,
    5: _migrate_v5,
}


def init_db(conn: sqlite3.Connection) -> None:
    """Opprett/oppgradér skjemaet idempotent.

    ``PRAGMA user_version`` er skjema-versjonen. Tom DB (versjon 0) bygges fra
    ``SCHEMA_SQL`` (som alltid speiler siste versjon). En eldre DB kjører de
    trinnvise migrasjonene i ``MIGRATIONS`` opp til ``SCHEMA_VERSION``. Begge
    veier er trygge å kjøre om igjen.
    """
    current = conn.execute("PRAGMA user_version").fetchone()[0]
    if current > SCHEMA_VERSION:
        raise RuntimeError(
            f"Databasen har skjema-versjon {current}, men denne appen kjenner "
            f"kun opptil {SCHEMA_VERSION}. Oppdater FotoMaxxer."
        )
    if current == SCHEMA_VERSION:
        return
    with conn:
        if current == 0:
            conn.executescript(SCHEMA_SQL)
        else:
            for version in range(current + 1, SCHEMA_VERSION + 1):
                migrate = MIGRATIONS.get(version)
                if migrate is not None:
                    migrate(conn)
        conn.execute(f"PRAGMA user_version = {SCHEMA_VERSION}")


def table_names(conn: sqlite3.Connection) -> list[str]:
    """Alle bruker-tabeller i DB-en (for verifisering/diagnostikk)."""
    rows = conn.execute(
        "SELECT name FROM sqlite_master WHERE type='table' "
        "AND name NOT LIKE 'sqlite_%' ORDER BY name"
    ).fetchall()
    return [r[0] for r in rows]
