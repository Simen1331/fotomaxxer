//! SQLite-tilkobling og skjema for FotoMaxxer.
//!
//! Skjemaet er autoritativt definert i Technical Brief (`tb-fotomaxxer-data-model-v1`).
//! `schema.sql` er kopiert ordrett fra Python-versjonen (`fotomaxxer/db.py`), så en
//! fersk database får identisk `sqlite_master`. Versjonering via `PRAGMA user_version`;
//! eldre databaser migreres trinnvis med de samme `ALTER`-setningene som i Python.

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{Error, Result};

pub const SCHEMA_VERSION: i64 = 5;

/// Full DDL (siste versjon). Alt med `IF NOT EXISTS` → idempotent.
pub const SCHEMA_SQL: &str = include_str!("schema.sql");

/// Åpne en tilkobling med fremmednøkler på og samme ventetid som Pythons `sqlite3` (5 s).
pub fn connect(db_path: &Path) -> Result<Connection> {
    let conn = Connection::open(db_path)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(Duration::from_secs(5))?;
    Ok(conn)
}

/// True hvis `table` har `column` (for idempotente ALTER-migrasjoner).
fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = stmt.query_map([], |r| r.get::<_, String>("name"))?;
    for name in names {
        if name? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// v1 → v2 (GUI-grill): `faces.locked` + `images.excluded` + `idx_images_excluded`.
fn migrate_v2(conn: &Connection) -> Result<()> {
    if !column_exists(conn, "faces", "locked")? {
        conn.execute(
            "ALTER TABLE faces ADD COLUMN locked INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    if !column_exists(conn, "images", "excluded")? {
        conn.execute(
            "ALTER TABLE images ADD COLUMN excluded INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_images_excluded ON images(excluded)",
        [],
    )?;
    Ok(())
}

/// v2 → v3: `ai_tags.cost` (faktisk USD-kost per AI-merking fra OpenRouter).
fn migrate_v3(conn: &Connection) -> Result<()> {
    if !column_exists(conn, "ai_tags", "cost")? {
        conn.execute("ALTER TABLE ai_tags ADD COLUMN cost REAL", [])?;
    }
    Ok(())
}

/// v3 → v4: `sources.focus` (per-kilde tagging-fokus; NULL = arv globalt).
fn migrate_v4(conn: &Connection) -> Result<()> {
    if !column_exists(conn, "sources", "focus")? {
        conn.execute("ALTER TABLE sources ADD COLUMN focus TEXT", [])?;
    }
    Ok(())
}

/// Hent (eller opprett) en tagging-profil for en fritekst-fokus. Dedup på focus.
fn profile_for_focus(conn: &Connection, focus: &str) -> Result<i64> {
    let focus = focus.trim();
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM tagging_profiles WHERE focus = ?",
            [focus],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        return Ok(id);
    }
    // Navn: avled fra teksten, fall tilbake til nummerert hvis kollisjon/tomt.
    let head: String = focus.chars().take(40).collect();
    let base = head
        .rsplit_once(' ')
        .map_or(head.as_str(), |(before, _)| before);
    let base = if base.is_empty() {
        "Importert fokus"
    } else {
        base
    }
    .trim()
    .to_string();
    let mut name = base.clone();
    let mut n = 1;
    while conn
        .query_row(
            "SELECT 1 FROM tagging_profiles WHERE name = ?",
            [&name],
            |_| Ok(()),
        )
        .optional()?
        .is_some()
    {
        n += 1;
        name = format!("{base} ({n})");
    }
    conn.execute(
        "INSERT INTO tagging_profiles (name, focus, categories) VALUES (?, ?, '[]')",
        params![name, focus],
    )?;
    Ok(conn.last_insert_rowid())
}

/// v4 → v5: `tagging_profiles` + `sources.profile_id` + `tags.category`.
///
/// Data-bevarende: hver distinkt ikke-tom `sources.focus` importeres som en profil,
/// og kilden re-pekes via `profile_id`. `settings['tagging_focus']` blir en profil
/// pekt av `settings['default_profile_id']`. `sources.focus` beholdes fysisk
/// (vestigial) men leses ikke lenger. Idempotent.
fn migrate_v5(conn: &Connection) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS tagging_profiles (
             id INTEGER PRIMARY KEY,
             name TEXT NOT NULL UNIQUE,
             focus TEXT,
             categories TEXT NOT NULL DEFAULT '[]',
             created_at TEXT NOT NULL DEFAULT (datetime('now'))
           )",
        [],
    )?;
    if !column_exists(conn, "sources", "profile_id")? {
        conn.execute(
            "ALTER TABLE sources ADD COLUMN profile_id INTEGER \
             REFERENCES tagging_profiles(id) ON DELETE SET NULL",
            [],
        )?;
    }
    if !column_exists(conn, "tags", "category")? {
        conn.execute("ALTER TABLE tags ADD COLUMN category TEXT", [])?;
    }

    // Importer per-kilde fokus → profil (kun rader som ennå ikke er re-pekt).
    if column_exists(conn, "sources", "focus")? {
        let rows: Vec<(i64, String)> = conn
            .prepare(
                "SELECT id, focus FROM sources \
                 WHERE focus IS NOT NULL AND TRIM(focus) <> '' AND profile_id IS NULL",
            )?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (sid, focus) in rows {
            let pid = profile_for_focus(conn, &focus)?;
            conn.execute(
                "UPDATE sources SET profile_id = ? WHERE id = ?",
                params![pid, sid],
            )?;
        }
    }

    // Globalt fokus → default-profil.
    let global: Option<Option<String>> = conn
        .query_row(
            "SELECT value FROM settings WHERE key = 'tagging_focus'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(Some(focus)) = global
        && !focus.trim().is_empty()
    {
        let pid = profile_for_focus(conn, &focus)?;
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('default_profile_id', ?) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [pid.to_string()],
        )?;
    }
    Ok(())
}

/// Trinnvis migrasjon som tar skjemaet fra `version - 1` til `version`.
fn migration(version: i64) -> Option<fn(&Connection) -> Result<()>> {
    match version {
        2 => Some(migrate_v2),
        3 => Some(migrate_v3),
        4 => Some(migrate_v4),
        5 => Some(migrate_v5),
        _ => None,
    }
}

/// Opprett/oppgradér skjemaet idempotent.
///
/// Tom DB (versjon 0) bygges fra `SCHEMA_SQL`; en eldre DB kjører de trinnvise
/// migrasjonene opp til `SCHEMA_VERSION`. En nyere DB enn appen kjenner avvises.
pub fn init_db(conn: &Connection) -> Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if current > SCHEMA_VERSION {
        return Err(Error::Schema(format!(
            "Databasen har skjema-versjon {current}, men denne appen kjenner \
             kun opptil {SCHEMA_VERSION}. Oppdater FotoMaxxer."
        )));
    }
    if current == SCHEMA_VERSION {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    if current == 0 {
        tx.execute_batch(SCHEMA_SQL)?;
    } else {
        for version in current + 1..=SCHEMA_VERSION {
            if let Some(migrate) = migration(version) {
                migrate(&tx)?;
            }
        }
    }
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(())
}

/// Alle bruker-tabeller i DB-en (for verifisering/diagnostikk).
pub fn table_names(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type='table' \
         AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )?;
    let names = stmt
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// v1-skjemaet: dagens DDL uten kolonnene/tabellene v2–v5 la til.
    const SCHEMA_V1: &str = "
        CREATE TABLE sources (id INTEGER PRIMARY KEY, root_path TEXT NOT NULL UNIQUE,
          source_type TEXT NOT NULL DEFAULT 'local', added_at TEXT NOT NULL DEFAULT (datetime('now')),
          last_scanned_at TEXT, file_count INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
        CREATE TABLE images (id INTEGER PRIMARY KEY, content_hash TEXT NOT NULL UNIQUE);
        CREATE TABLE ai_tags (id INTEGER PRIMARY KEY, image_id INTEGER NOT NULL, provider TEXT NOT NULL);
        CREATE TABLE tags (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
        CREATE TABLE persons (cluster_id INTEGER PRIMARY KEY, name TEXT);
        CREATE TABLE faces (id INTEGER PRIMARY KEY, image_id INTEGER NOT NULL, bbox TEXT NOT NULL,
          embedding BLOB NOT NULL, cluster_id INTEGER);
        PRAGMA user_version = 1;
    ";

    fn version(conn: &Connection) -> i64 {
        conn.query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn fersk_db_far_alle_tabeller_og_siste_versjon() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        assert_eq!(version(&conn), SCHEMA_VERSION);
        assert_eq!(
            table_names(&conn).unwrap(),
            [
                "ai_tags",
                "album_items",
                "albums",
                "derived",
                "faces",
                "files",
                "image_tags",
                "images",
                "persons",
                "processing_state",
                "scan_index",
                "settings",
                "sources",
                "tagging_profiles",
                "tags"
            ]
        );
        init_db(&conn).unwrap(); // idempotent
    }

    #[test]
    fn v1_migreres_til_v5() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        init_db(&conn).unwrap();
        assert_eq!(version(&conn), SCHEMA_VERSION);
        for (table, column) in [
            ("faces", "locked"),
            ("images", "excluded"),
            ("ai_tags", "cost"),
            ("sources", "focus"),
            ("sources", "profile_id"),
            ("tags", "category"),
        ] {
            assert!(
                column_exists(&conn, table, column).unwrap(),
                "{table}.{column}"
            );
        }
    }

    #[test]
    fn v4_fokus_blir_profiler() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        for m in [migrate_v2, migrate_v3, migrate_v4] {
            m(&conn).unwrap();
        }
        conn.execute_batch(
            "INSERT INTO sources (root_path, focus) VALUES ('/a', 'Design og håndverk i tre'),
                                                          ('/b', 'Design og håndverk i tre'),
                                                          ('/c', '  '),
                                                          ('/d', 'Fugler');
             INSERT INTO settings (key, value) VALUES ('tagging_focus', 'Fugler');
             PRAGMA user_version = 4;",
        )
        .unwrap();
        init_db(&conn).unwrap();

        let profiles: Vec<(i64, String, String)> = conn
            .prepare("SELECT id, name, focus FROM tagging_profiles ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            profiles,
            [
                (
                    1,
                    "Design og håndverk i".to_string(),
                    "Design og håndverk i tre".to_string()
                ),
                (2, "Fugler".to_string(), "Fugler".to_string()),
            ]
        );
        let pointers: Vec<Option<i64>> = conn
            .prepare("SELECT profile_id FROM sources ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(pointers, [Some(1), Some(1), None, Some(2)]);
        let default: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'default_profile_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(default, "2");
    }

    #[test]
    fn nyere_db_avvises() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
            .unwrap();
        assert!(matches!(init_db(&conn), Err(Error::Schema(_))));
    }
}
