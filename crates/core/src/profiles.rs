//! Tagging-profiler: navngitt, gjenbrukbar AI-merkings-konfig.
//!
//! En profil bærer et fokus-direktiv + en ordnet liste kategorier. Kategoriene former
//! prompten og GUI-grupperingen. En kilde peker på en profil via `sources.profile_id`;
//! tom = arv globalt `settings['default_profile_id']`. `categories` lagres som
//! JSON-tekst (samme form som Pythons `json.dumps`).

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;

use crate::error::{Error, Result};
use crate::pyjson;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Profile {
    pub id: i64,
    pub name: String,
    pub focus: Option<String>,
    pub categories: Vec<String>,
}

/// Profilens `categories`-JSON → liste. Tom/ugyldig → `[]`; tomme navn droppes.
pub fn parse_categories(raw: Option<&str>) -> Vec<String> {
    let Some(raw) = raw.filter(|s| !s.is_empty()) else {
        return Vec::new();
    };
    match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(serde_json::Value::Array(items)) => items
            .iter()
            .map(|v| match v {
                serde_json::Value::String(s) => s.trim().to_string(),
                other => other.to_string().trim().to_string(),
            })
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

fn from_row(row: &Row) -> rusqlite::Result<Profile> {
    Ok(Profile {
        id: row.get("id")?,
        name: row.get("name")?,
        focus: row.get("focus")?,
        categories: parse_categories(row.get::<_, Option<String>>("categories")?.as_deref()),
    })
}

/// Normaliser input: navn påkrevd, tom fokus → `None`, kategorier trimmet og deduplisert.
fn normalize(
    name: &str,
    focus: Option<&str>,
    categories: &[String],
) -> Result<(String, Option<String>, String)> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::Profile("Profilen må ha et navn.".into()));
    }
    let focus = focus
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .map(str::to_string);
    let mut cats: Vec<String> = Vec::new();
    for c in categories
        .iter()
        .map(|c| c.trim())
        .filter(|c| !c.is_empty())
    {
        if !cats.iter().any(|seen| seen == c) {
            cats.push(c.to_string());
        }
    }
    Ok((name.to_string(), focus, pyjson::dumps(&cats)))
}

fn duplicate_name(name: &str) -> Error {
    Error::Profile(format!(
        "Det finnes allerede en profil med navnet «{name}»."
    ))
}

fn is_unique_violation(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(err, _) if err.code == rusqlite::ErrorCode::ConstraintViolation)
}

pub fn list_profiles(conn: &Connection) -> Result<Vec<Profile>> {
    let mut stmt =
        conn.prepare("SELECT id, name, focus, categories FROM tagging_profiles ORDER BY name")?;
    let profiles = stmt
        .query_map([], from_row)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(profiles)
}

pub fn get_profile(conn: &Connection, profile_id: i64) -> Result<Option<Profile>> {
    Ok(conn
        .query_row(
            "SELECT id, name, focus, categories FROM tagging_profiles WHERE id = ?",
            [profile_id],
            from_row,
        )
        .optional()?)
}

pub fn create_profile(
    conn: &Connection,
    name: &str,
    focus: Option<&str>,
    categories: &[String],
) -> Result<Profile> {
    let (name, focus, cats_json) = normalize(name, focus, categories)?;
    conn.execute(
        "INSERT INTO tagging_profiles (name, focus, categories) VALUES (?, ?, ?)",
        params![name, focus, cats_json],
    )
    .map_err(|e| {
        if is_unique_violation(&e) {
            duplicate_name(&name)
        } else {
            e.into()
        }
    })?;
    let id = conn.last_insert_rowid();
    get_profile(conn, id)?.ok_or_else(|| Error::Profile(format!("Ukjent profil: {id}")))
}

pub fn update_profile(
    conn: &Connection,
    profile_id: i64,
    name: &str,
    focus: Option<&str>,
    categories: &[String],
) -> Result<Profile> {
    if get_profile(conn, profile_id)?.is_none() {
        return Err(Error::Profile(format!("Ukjent profil: {profile_id}")));
    }
    let (name, focus, cats_json) = normalize(name, focus, categories)?;
    conn.execute(
        "UPDATE tagging_profiles SET name = ?, focus = ?, categories = ? WHERE id = ?",
        params![name, focus, cats_json, profile_id],
    )
    .map_err(|e| {
        if is_unique_violation(&e) {
            duplicate_name(&name)
        } else {
            e.into()
        }
    })?;
    get_profile(conn, profile_id)?
        .ok_or_else(|| Error::Profile(format!("Ukjent profil: {profile_id}")))
}

/// Slett en profil. Kilder som peker på den nullstilles (FK ON DELETE SET NULL),
/// og er den globalt standard fjernes den pekeren også.
pub fn delete_profile(conn: &Connection, profile_id: i64) -> Result<()> {
    if get_profile(conn, profile_id)?.is_none() {
        return Err(Error::Profile(format!("Ukjent profil: {profile_id}")));
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM tagging_profiles WHERE id = ?", [profile_id])?;
    tx.execute(
        "DELETE FROM settings WHERE key = 'default_profile_id' AND value = ?",
        [profile_id.to_string()],
    )?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn conn() -> Connection {
        let c = db::connect(std::path::Path::new(":memory:")).unwrap();
        db::init_db(&c).unwrap();
        c
    }

    fn cats(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn opprett_normaliserer_og_lagrer_python_json() {
        let c = conn();
        let p = create_profile(
            &c,
            "  Design  ",
            Some("  "),
            &cats(&["materiale", " teknikk ", "materiale", ""]),
        )
        .unwrap();
        assert_eq!(p.name, "Design");
        assert_eq!(p.focus, None);
        assert_eq!(p.categories, ["materiale", "teknikk"]);
        let raw: String = c
            .query_row("SELECT categories FROM tagging_profiles", [], |r| r.get(0))
            .unwrap();
        assert_eq!(raw, r#"["materiale", "teknikk"]"#);
    }

    #[test]
    fn duplikat_navn_og_tomt_navn_gir_brukerfeil() {
        let c = conn();
        create_profile(&c, "A", None, &[]).unwrap();
        let e = create_profile(&c, "A", None, &[]).unwrap_err();
        assert_eq!(
            e.to_string(),
            "Det finnes allerede en profil med navnet «A»."
        );
        assert_eq!(
            create_profile(&c, " ", None, &[]).unwrap_err().to_string(),
            "Profilen må ha et navn."
        );
    }

    #[test]
    fn sletting_nullstiller_kilder_og_default() {
        let c = conn();
        let p = create_profile(&c, "A", Some("fokus"), &[]).unwrap();
        c.execute(
            "INSERT INTO sources (root_path, profile_id) VALUES ('/x', ?)",
            [p.id],
        )
        .unwrap();
        c.execute(
            "INSERT INTO settings (key, value) VALUES ('default_profile_id', ?)",
            [p.id.to_string()],
        )
        .unwrap();
        delete_profile(&c, p.id).unwrap();
        let pid: Option<i64> = c
            .query_row("SELECT profile_id FROM sources", [], |r| r.get(0))
            .unwrap();
        assert_eq!(pid, None);
        let n: i64 = c
            .query_row("SELECT COUNT(*) FROM settings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
        assert!(delete_profile(&c, p.id).is_err());
    }
}
