//! Innstillinger + vedlikehold: nøkkel/modell/terskler, datamappe, klargjør-for-deling.
//!
//! Motparten til `api/routers/settings.py`. Vanlige settings går via
//! `fotomaxxer_core::settings` — nøkkelen vises **alltid** maskert (`mask_value`).
//!
//! **Datamappa er bootstrap, ikke en setting** (sirkulært — stien trengs for å åpne
//! databasen): den leses/skrives via `maintenance/data-dir` mot pekerfila, aldri
//! `/api/settings`. Etter en flytting åpner neste forespørsel databasen på det nye
//! stedet, mens `/derived` fortsatt serveres fra mappa tjeneren startet med (som i
//! Python, der `StaticFiles`-mounten er låst ved oppstart).

use std::path::Path;

use fotomaxxer_core::{db, paths, settings as settings_mod, sources, storage};
use rusqlite::Connection;

use super::Routes;
use crate::error::{ApiError, ApiResult, DictResponse, JsonResponse};
use crate::schemas::{DataDir, DataDirMove, Message, SettingKeyPath, SettingPut, Settings};
use crate::state::{blocking, make_storage};
use crate::validate::{DbReq, Req};

pub fn routes() -> Routes {
    Routes::new()
        .get("/settings", get_settings)
        .put("/settings/{key}", put_setting)
        .get("/maintenance/data-dir", get_data_dir)
        .post("/maintenance/data-dir", move_data_dir)
        .post("/maintenance/prepare-handoff", prepare_handoff)
}

const ALLOWED_KEYS: &[&str] = &[
    "ai_model",
    "face_match_threshold",
    "face_min_cluster_size",
    "default_export_dir",
    "default_profile_id",
    "openrouter_api_key",
];

/// Pythons `int(value)`: whitespace trimmes, valgfritt fortegn, sifre med enkle `_`
/// mellom. Alt annet (også `"1.0"`) → `None`, som `_opt_int`.
fn py_int(value: &str) -> Option<i64> {
    let t = value.trim();
    let digits = t.strip_prefix(['+', '-']).unwrap_or(t);
    let bytes = digits.as_bytes();
    if bytes.is_empty()
        || !bytes[0].is_ascii_digit()
        || !bytes[bytes.len() - 1].is_ascii_digit()
        || bytes.windows(2).any(|w| w == b"__")
        || !bytes.iter().all(|b| b.is_ascii_digit() || *b == b'_')
    {
        return None;
    }
    t.replace('_', "").parse().ok()
}

/// Settings-svaret (brukes også etter `PUT`).
fn settings_out(conn: &Connection) -> ApiResult<Settings> {
    let merged = settings_mod::all_settings(conn)?;
    let raw_key = settings_mod::get_setting(conn, "openrouter_api_key")?;
    Ok(Settings {
        ai_model: merged.get("ai_model").cloned(),
        face_match_threshold: merged.get("face_match_threshold").cloned(),
        face_min_cluster_size: merged.get("face_min_cluster_size").cloned(),
        default_export_dir: settings_mod::get_setting(conn, "default_export_dir")?,
        default_profile_id: settings_mod::get_setting(conn, "default_profile_id")?
            .as_deref()
            .and_then(py_int),
        openrouter_api_key: settings_mod::mask_value("openrouter_api_key", raw_key.as_deref()),
    })
}

async fn get_settings(req: DbReq) -> ApiResult<JsonResponse<Settings>> {
    let conn = req.conn;
    blocking(move || Ok(JsonResponse(settings_out(&conn)?))).await
}

async fn put_setting(
    req: DbReq<SettingKeyPath, (), SettingPut>,
) -> ApiResult<JsonResponse<Settings>> {
    let DbReq {
        conn, path, body, ..
    } = req;
    blocking(move || {
        if !ALLOWED_KEYS.contains(&path.key.as_str()) {
            return Err(ApiError::http(
                400,
                "bad_key",
                format!("ukjent setting: {}", path.key),
            ));
        }
        settings_mod::set_setting(&conn, &path.key, &body.value)?;
        Ok(JsonResponse(settings_out(&conn)?))
    })
    .await
}

/// Gjeldende datamappe (fra `--home`/env, pekerfila eller cwd).
async fn get_data_dir() -> JsonResponse<DataDir> {
    JsonResponse(DataDir {
        data_dir: paths::to_string(&storage::resolve_root(None)),
    })
}

/// Kopiér db + derived til ny mappe og oppdater pekeren. Gammel mappe beholdes som
/// standard; `delete_source=true` fjerner den etter at kopien er trygt på plass.
async fn move_data_dir(req: Req<(), (), DataDirMove>) -> ApiResult<JsonResponse<DataDir>> {
    let body = req.body;
    blocking(move || {
        let new_dir = paths::expand_user(Path::new(&body.path));
        storage::relocate_data(&new_dir, None, body.delete_source).map_err(|e| match e {
            fotomaxxer_core::Error::Io(e) => ApiError::http(400, "move_failed", e.to_string()),
            other => ApiError::from(other),
        })?;
        Ok(JsonResponse(DataDir {
            data_dir: paths::to_string(&storage::resolve_root(None)),
        }))
    })
    .await
}

/// Tøm kilder + settings (inkl. nøkkel) så pakka kan deles. Filer på disk er urørt.
async fn prepare_handoff() -> ApiResult<DictResponse<Message>> {
    blocking(|| {
        let store = make_storage();
        let conn = db::connect(&store.db_path())?;
        let stats = sources::prepare_for_handoff(&conn, &store)?;
        Ok(DictResponse(Message {
            message: format!("Pakka er klar for deling. {}", stats.summary()),
        }))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::py_int;

    #[test]
    fn opt_int_som_pythons_int() {
        for (s, want) in [
            ("3", Some(3)),
            (" 3 ", Some(3)),
            ("+3", Some(3)),
            ("-3", Some(-3)),
            ("1_0", Some(10)),
            ("1.0", None),
            ("", None),
            ("  ", None),
            ("x", None),
            ("1__0", None),
            ("_1", None),
        ] {
            assert_eq!(py_int(s), want, "{s:?}");
        }
    }
}
