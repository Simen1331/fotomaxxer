//! `/api`-ruterne, én modul per fil i `api/routers/`.
//!
//! **Slik legges en ruter til** (trinn B: `images`, `export`, `persons`, `albums` finnes
//! allerede som tomme moduler og er registrert under — fyll inn `routes()` i din fil):
//!
//! ```ignore
//! pub fn routes() -> Routes {
//!     Routes::new()
//!         .get("/albums", list_albums)
//!         .post("/albums", create_album)
//!         .get("/albums/{album_id}", get_album)
//! }
//!
//! // `JsonResponse` for endepunkter med `response_model` (pydantic-flyttall),
//! // `DictResponse` for de som returnerer en `dict` (`{"deleted": id}`).
//! async fn get_album(req: DbReq<AlbumIdPath>) -> ApiResult<JsonResponse<AlbumDetail>> {
//!     let DbReq { conn, path, .. } = req;
//!     blocking(move || { … Ok(JsonResponse(detail)) }).await
//! }
//! ```
//!
//! Hjelpere trinn B kan bruke: [`image_query`] (`_build_filters`, `_thumb_url` og
//! bildelista som både `images.py` og `persons.py` trenger), [`jobs::accept`] (start en
//! bakgrunnsjobb eller svar 409 «busy», som eksport trenger) og [`jobs::job_conn`].
//!
//! Stiene skrives uten `/api` (prefikset legges på i [`api_router`]) og med axums
//! `{param}`-syntaks, som i FastAPI. Rekkefølgen i [`routes`] er FastAPIs
//! `include_router`-rekkefølge; den avgjør `allow`-headeren i 405-svar (metoden til den
//! første ruten med samme sti, som i Starlette). `HEAD` på en `GET`-rute gir 405, som i
//! FastAPI. Med SPA gir `GET` på en sti som bare har andre metoder 404 «ukjent endepunkt»
//! (SPA-ruten `GET /{full_path:path}` matcher fullt og vinner over 405), f.eks.
//! `/api/export`, `/api/jobs/update`, `/api/albums/{id}/order`; uten SPA 405 som før.

use std::collections::HashMap;

use axum::Router;
use axum::extract::{Request, State};
use axum::handler::Handler;
use axum::http::Method;
use axum::response::IntoResponse;
use axum::routing::{MethodFilter, MethodRouter, on};

use crate::error::ApiError;
use crate::state::AppState;

pub mod albums;
pub mod export;
pub mod fs;
pub mod image_query;
pub mod images;
pub mod jobs;
pub mod persons;
pub mod profiles;
pub mod settings;
pub mod sources;

/// Alle API-rutene i FastAPIs rekkefølge (`api/app.py`), med `/api/health` til slutt.
pub fn routes() -> Routes {
    Routes::new()
        .merge(images::routes())
        .merge(persons::routes())
        .merge(albums::routes())
        .merge(export::routes())
        .merge(sources::routes())
        .merge(profiles::routes())
        .merge(fs::routes())
        .merge(jobs::routes())
        .merge(settings::routes())
        .get("/health", health)
}

/// `/api`-ruteren.
pub fn api_router() -> Router<AppState> {
    routes().build("/api")
}

async fn health() -> crate::error::DictResponse<crate::schemas::Health> {
    crate::error::DictResponse(crate::schemas::Health {
        status: "ok".into(),
    })
}

/// Rutetabell med FastAPI-semantikk for metoder (se moduldokumentasjonen).
#[derive(Default)]
pub struct Routes {
    entries: Vec<(String, Method, MethodRouter<AppState>)>,
}

impl Routes {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registrer `handler` for `method` på `path` (uten `/api`).
    pub fn route<H, T>(mut self, method: Method, path: &str, handler: H) -> Self
    where
        H: Handler<T, AppState>,
        T: 'static,
    {
        let filter = MethodFilter::try_from(method.clone()).expect("støttet HTTP-metode");
        self.entries
            .push((path.to_string(), method, on(filter, handler)));
        self
    }

    pub fn get<H: Handler<T, AppState>, T: 'static>(self, path: &str, handler: H) -> Self {
        self.route(Method::GET, path, handler)
    }

    pub fn post<H: Handler<T, AppState>, T: 'static>(self, path: &str, handler: H) -> Self {
        self.route(Method::POST, path, handler)
    }

    pub fn put<H: Handler<T, AppState>, T: 'static>(self, path: &str, handler: H) -> Self {
        self.route(Method::PUT, path, handler)
    }

    pub fn patch<H: Handler<T, AppState>, T: 'static>(self, path: &str, handler: H) -> Self {
        self.route(Method::PATCH, path, handler)
    }

    pub fn delete<H: Handler<T, AppState>, T: 'static>(self, path: &str, handler: H) -> Self {
        self.route(Method::DELETE, path, handler)
    }

    pub fn merge(mut self, other: Routes) -> Self {
        self.entries.extend(other.entries);
        self
    }

    /// Bygg en axum-`Router` med `prefix` foran hver sti.
    pub fn build(self, prefix: &str) -> Router<AppState> {
        let mut order: Vec<String> = Vec::new();
        let mut by_path: HashMap<String, (Vec<Method>, MethodRouter<AppState>)> = HashMap::new();
        for (path, method, mr) in self.entries {
            match by_path.remove(&path) {
                Some((mut methods, existing)) => {
                    methods.push(method);
                    by_path.insert(path, (methods, existing.merge(mr)));
                }
                None => {
                    order.push(path.clone());
                    by_path.insert(path, (vec![method], mr));
                }
            }
        }
        let mut router = Router::new();
        for path in order {
            let (methods, mut mr) = by_path.remove(&path).expect("stien ble nettopp lagt inn");
            let allow = methods[0].as_str().to_string();
            if !methods.contains(&Method::HEAD) {
                let allow = allow.clone();
                mr = mr.on(MethodFilter::HEAD, move || async move {
                    ApiError::method_not_allowed(Some(&allow)).into_response()
                });
            }
            if !methods.contains(&Method::GET) {
                // Med SPA matcher FastAPIs `GET /{full_path:path}` fullt og vinner over den
                // delvise treffen her: `GET` på en sti som bare har andre metoder gir da 404
                // «ukjent endepunkt», ikke 405 (f.eks. `GET /api/albums/{id}/order`). En egen
                // `GET`-håndterer (ikke `fallback`), så axum ikke legger på `allow`.
                let allow = allow.clone();
                mr = mr.on(
                    MethodFilter::GET,
                    move |State(state): State<AppState>, req: Request| async move {
                        if state.spa().is_some() {
                            return crate::spa::fallback(State(state), req).await;
                        }
                        ApiError::method_not_allowed(Some(&allow)).into_response()
                    },
                );
            }
            mr = mr.fallback(move || async move {
                ApiError::method_not_allowed(Some(&allow)).into_response()
            });
            router = router.route(&format!("{prefix}{path}"), mr);
        }
        router
    }
}
