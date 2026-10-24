//! FotoMaxxers HTTP-API (axum) — drop-in-erstatning for FastAPI-appen i `api/`.
//!
//! | Python (`api/`)          | Rust (`crates/api/src/`)                       |
//! | ------------------------ | ---------------------------------------------- |
//! | `app.py` (`create_app`)  | [`create_app`], [`serve`], [`spa`]             |
//! | `deps.py`                | [`state`] (`make_storage`, tilkobling per forespørsel) |
//! | `desktop_access.py`      | [`desktop_access`]                             |
//! | `jobs.py`                | [`jobs`] (`JobRunner`)                         |
//! | `schemas.py`             | [`schemas`] (alle modeller, også trinn B sine) |
//! | `routers/*.py`           | [`routers`] (én fil per ruter)                 |
//! | FastAPI/pydantic-feil    | [`error`], [`validate`]                        |
//!
//! Kontrakten er den samme som FastAPI-versjonen: stier, statuskoder, JSON-bytes
//! (feltrekkefølge, `null`, flyttall som pydantic/`json.dumps`) og feilformatene. Avvik er
//! dokumentert i modulene der de oppstår.

pub mod desktop_access;
pub mod error;
pub mod jobs;
pub mod routers;
pub mod schemas;
pub mod spa;
pub mod state;
pub mod validate;

use std::future::Future;

use axum::Router;
use axum::routing::any;

pub use error::{ApiError, ApiResult, DictResponse, JsonResponse};
pub use state::{AppState, DesktopConfig};

/// Bygg appen (`create_app` i `api/app.py`): `/api`-ruterne, `/derived`, `/assets` og
/// SPA-fallbacken, med desktop-tilgangsporten ytterst når `state` har en [`DesktopConfig`].
pub fn create_app(state: AppState) -> Router {
    let mut app = routers::api_router()
        .route("/derived/", any(spa::derived))
        .route("/derived/{*path}", any(spa::derived));
    if state.spa().is_some_and(|s| s.has_assets()) {
        app = app
            .route("/assets/", any(spa::assets))
            .route("/assets/{*path}", any(spa::assets));
    }
    let app = app.fallback(spa::fallback).with_state(state.clone());
    if state.desktop().is_some() {
        app.layer(axum::middleware::from_fn_with_state(
            state,
            desktop_access::desktop_access,
        ))
    } else {
        app
    }
}

/// Kjør tjeneren på `listener` til `shutdown` fullføres, og avslutt som uvicorns
/// lifespan: steng jobbporten og vent til en pågående jobb er ferdig før vi returnerer.
pub async fn serve(
    listener: tokio::net::TcpListener,
    state: AppState,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let app = create_app(state.clone());
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await;
    state.runner().begin_shutdown();
    let runner = state.runner().clone();
    let _ = tokio::task::spawn_blocking(move || runner.wait(None)).await;
    result
}

/// Ctrl-C (og SIGTERM på Unix) — signalene uvicorn avslutter på.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
}
