//! Bruker-konfig i `settings`-tabellen.
//!
//! Nøkler som `openrouter_api_key` og `ai_model` lever her, ikke i `.env` — slik at
//! GUI kan skrive dem og pakka forblir flyttbar. Hemmeligheter maskeres ved *visning*
//! (`mask_value`), men lagres i klartekst i den lokale DB-en (`prepare-handoff`
//! tømmer tabellen før deling).

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::Result;

/// Defaults som gjelder når nøkkelen ikke er satt i DB-en.
pub const DEFAULTS: &[(&str, &str)] = &[
    ("ai_model", "google/gemini-2.5-flash"),
    ("face_match_threshold", "0.5"),
    ("face_min_cluster_size", "3"),
];

/// Nøkler hvis verdi aldri skal skrives ut i klartekst.
pub const SECRET_KEYS: &[&str] = &["openrouter_api_key"];

pub fn default_for(key: &str) -> Option<&'static str> {
    DEFAULTS.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// Verdien for `key`, eller default, eller `None` hvis ukjent.
pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    let value: Option<Option<String>> = conn
        .query_row("SELECT value FROM settings WHERE key = ?", [key], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(value
        .flatten()
        .or_else(|| default_for(key).map(str::to_string)))
}

/// Skriv (upsert) en setting.
pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?, ?) \
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// Alle satte settings slått sammen med defaults (rå verdier, ingen maskering), sortert på nøkkel.
pub fn all_settings(conn: &Connection) -> Result<BTreeMap<String, String>> {
    let mut merged: BTreeMap<String, String> = DEFAULTS
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let mut stmt = conn.prepare("SELECT key, value FROM settings ORDER BY key")?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
    })?;
    for row in rows {
        if let (key, Some(value)) = row? {
            merged.insert(key, value);
        }
    }
    Ok(merged)
}

/// Maskér hemmelige verdier for visning; vis resten som de er.
pub fn mask_value(key: &str, value: Option<&str>) -> String {
    let Some(value) = value else {
        return "(ikke satt)".into();
    };
    if SECRET_KEYS.contains(&key) {
        let chars: Vec<char> = value.chars().collect();
        if chars.len() <= 8 {
            return "••••".into();
        }
        let head: String = chars[..4].iter().collect();
        let tail: String = chars[chars.len() - 4..].iter().collect();
        return format!("{head}…{tail}");
    }
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        db::init_db(&c).unwrap();
        c
    }

    #[test]
    fn default_overstyres_av_satt_verdi() {
        let c = conn();
        assert_eq!(
            get_setting(&c, "ai_model").unwrap().as_deref(),
            Some("google/gemini-2.5-flash")
        );
        assert_eq!(get_setting(&c, "ukjent").unwrap(), None);
        set_setting(&c, "ai_model", "x/y").unwrap();
        set_setting(&c, "ai_model", "x/z").unwrap();
        assert_eq!(get_setting(&c, "ai_model").unwrap().as_deref(), Some("x/z"));
        let all = all_settings(&c).unwrap();
        assert_eq!(all["ai_model"], "x/z");
        assert_eq!(all["face_match_threshold"], "0.5");
    }

    #[test]
    fn hemmeligheter_maskeres() {
        assert_eq!(
            mask_value("openrouter_api_key", Some("sk-or-v1-abcdef123456")),
            "sk-o…3456"
        );
        assert_eq!(mask_value("openrouter_api_key", Some("kort")), "••••");
        assert_eq!(mask_value("ai_model", Some("a/b")), "a/b");
        assert_eq!(mask_value("ai_model", None), "(ikke satt)");
    }
}
