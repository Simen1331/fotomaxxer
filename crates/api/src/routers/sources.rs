//! Kilder: list/legg til/fjern kilde-mapper og sett per-kilde tagging-profil.
//!
//! Motparten til `api/routers/sources.py`; kaller `fotomaxxer_core::sources` uendret.
//! `add` validerer at mappa finnes; `remove` rydder orphans men rører **aldri**
//! kilde-filer på disk. Selve skanningen skjer via pipeline-jobbene (`/api/jobs`).

use std::path::Path;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use fotomaxxer_core::{Error, sources};

use super::Routes;
use crate::error::{ApiError, ApiResult, DictResponse, JsonResponse};
use crate::schemas::{
    SourceCreate, SourceIdPath, SourceItem, SourceList, SourceProfileUpdate, SourceRemoved,
};
use crate::state::{blocking, make_storage};
use crate::validate::DbReq;

pub fn routes() -> Routes {
    Routes::new()
        .get("/sources", list_sources)
        .post("/sources", add_source)
        .patch("/sources/{source_id}", set_source_profile)
        .delete("/sources/{source_id}", remove_source)
}

pub fn to_item(s: sources::Source) -> SourceItem {
    SourceItem {
        id: s.id,
        root_path: s.root_path,
        source_type: s.source_type,
        last_scanned_at: s.last_scanned_at,
        file_count: s.file_count,
        profile_id: s.profile_id,
    }
}

/// `SourceError` → `HTTPException(status, {"error": error, "message": …})`; andre feil → 500.
fn source_error(status: u16, error: &'static str) -> impl Fn(Error) -> ApiError {
    move |e| match e {
        Error::Source(message) => ApiError::http(status, error, message),
        other => ApiError::from(other),
    }
}

async fn list_sources(req: DbReq) -> ApiResult<JsonResponse<SourceList>> {
    let conn = req.conn;
    blocking(move || {
        let items = sources::list_sources(&conn)?
            .into_iter()
            .map(to_item)
            .collect();
        Ok(JsonResponse(SourceList { items }))
    })
    .await
}

async fn add_source(req: DbReq<(), (), SourceCreate>) -> ApiResult<impl IntoResponse> {
    let DbReq { conn, body, .. } = req;
    blocking(move || {
        let row = sources::add(&conn, Path::new(&body.path), "local", body.profile_id)
            .map_err(source_error(400, "bad_source"))?;
        Ok((StatusCode::CREATED, JsonResponse(to_item(row))))
    })
    .await
}

/// Sett/fjern per-kilde tagging-profil. Re-merker ikke automatisk (gated retag).
async fn set_source_profile(
    req: DbReq<SourceIdPath, (), SourceProfileUpdate>,
) -> ApiResult<JsonResponse<SourceItem>> {
    let DbReq {
        conn, path, body, ..
    } = req;
    blocking(move || {
        let row = sources::set_profile(&conn, &path.source_id.to_string(), body.profile_id)
            .map_err(source_error(404, "not_found"))?;
        Ok(JsonResponse(to_item(row)))
    })
    .await
}

async fn remove_source(req: DbReq<SourceIdPath>) -> ApiResult<DictResponse<SourceRemoved>> {
    let DbReq { conn, path, .. } = req;
    blocking(move || {
        let stats = sources::remove(&conn, &make_storage(), &path.source_id.to_string())
            .map_err(source_error(404, "not_found"))?;
        Ok(DictResponse(SourceRemoved {
            removed: path.source_id,
            summary: stats.summary(),
        }))
    })
    .await
}
