//! Tagging-profiler: list/opprett/rediger/slett.
//!
//! Motparten til `api/routers/profiles.py`; kaller `fotomaxxer_core::profiles` uendret.
//! En profil bærer fokus-direktiv + kategorier; kilder peker på den via `profile_id`,
//! globalt standard ligger i `settings['default_profile_id']`. Sletting nullstiller
//! pekende kilder (FK).

use axum::http::StatusCode;
use axum::response::IntoResponse;
use fotomaxxer_core::{Error, profiles};

use super::Routes;
use crate::error::{ApiError, ApiResult, DictResponse, JsonResponse};
use crate::schemas::{
    Deleted, ProfileCreate, ProfileIdPath, ProfileItem, ProfileList, ProfileUpdate,
};
use crate::state::blocking;
use crate::validate::DbReq;

pub fn routes() -> Routes {
    Routes::new()
        .get("/profiles", list_profiles)
        .post("/profiles", create_profile)
        .put("/profiles/{profile_id}", update_profile)
        .delete("/profiles/{profile_id}", delete_profile)
}

fn to_item(p: profiles::Profile) -> ProfileItem {
    ProfileItem {
        id: p.id,
        name: p.name,
        focus: p.focus,
        categories: p.categories,
    }
}

fn profile_error(status: u16, error: &'static str) -> impl Fn(Error) -> ApiError {
    move |e| match e {
        Error::Profile(message) => ApiError::http(status, error, message),
        other => ApiError::from(other),
    }
}

async fn list_profiles(req: DbReq) -> ApiResult<JsonResponse<ProfileList>> {
    let conn = req.conn;
    blocking(move || {
        let items = profiles::list_profiles(&conn)?
            .into_iter()
            .map(to_item)
            .collect();
        Ok(JsonResponse(ProfileList { items }))
    })
    .await
}

async fn create_profile(req: DbReq<(), (), ProfileCreate>) -> ApiResult<impl IntoResponse> {
    let DbReq { conn, body, .. } = req;
    blocking(move || {
        let p =
            profiles::create_profile(&conn, &body.name, body.focus.as_deref(), &body.categories)
                .map_err(profile_error(400, "bad_profile"))?;
        Ok((StatusCode::CREATED, JsonResponse(to_item(p))))
    })
    .await
}

async fn update_profile(
    req: DbReq<ProfileIdPath, (), ProfileUpdate>,
) -> ApiResult<JsonResponse<ProfileItem>> {
    let DbReq {
        conn, path, body, ..
    } = req;
    blocking(move || {
        let p = profiles::update_profile(
            &conn,
            path.profile_id,
            &body.name,
            body.focus.as_deref(),
            &body.categories,
        )
        .map_err(profile_error(400, "bad_profile"))?;
        Ok(JsonResponse(to_item(p)))
    })
    .await
}

async fn delete_profile(req: DbReq<ProfileIdPath>) -> ApiResult<DictResponse<Deleted>> {
    let DbReq { conn, path, .. } = req;
    blocking(move || {
        profiles::delete_profile(&conn, path.profile_id)
            .map_err(profile_error(404, "not_found"))?;
        Ok(DictResponse(Deleted {
            deleted: path.profile_id,
        }))
    })
    .await
}
