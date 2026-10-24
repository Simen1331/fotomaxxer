//! Delt tilstand og avhengigheter — motparten til `api/deps.py`.
//!
//! - [`make_storage`]: `Storage` rotet i den gjeldende datamappa (`--home` via
//!   `FOTOMAXXER_HOME` > pekerfil > cwd), løst på nytt for hver forespørsel, så en
//!   flyttet datamappe (`POST /api/maintenance/data-dir`) tas i bruk umiddelbart.
//! - [`open_request_db`]: én tilkobling per forespørsel med skjemaet sikret
//!   (`db::connect` + `init_db`), som Pythons `get_conn`. Brukes via [`crate::validate::DbReq`].
//! - [`blocking`]: kjør synkront databasearbeid på tokios blokkeringstråder (FastAPI
//!   kjører `def`-endepunkter i en trådpool på samme måte).
//!
//! Bakgrunnsjobber bruker IKKE disse; de åpner sin egen tilkobling (se [`crate::jobs`]).

use std::path::PathBuf;
use std::sync::Arc;

use fotomaxxer_core::{Storage, db, storage};
use rusqlite::Connection;

use crate::error::{ApiError, ApiResult};
use crate::jobs::JobRunner;
use crate::spa::{EmbeddedSpa, SpaSource};

/// Storage rotet i den gjeldende datamappa (bootstrap-peker > env > cwd).
pub fn make_storage() -> Storage {
    Storage::new(&storage::resolve_root(None))
}

/// Åpne forespørselens tilkobling med skjemaet på plass (en v1-database migreres her).
pub async fn open_request_db() -> ApiResult<Connection> {
    blocking(|| {
        let store = make_storage();
        let conn = db::connect(&store.db_path())?;
        db::init_db(&conn)?;
        Ok(conn)
    })
    .await
}

/// Kjør `f` på en blokkeringstråd. Panikk blir 500 (som et ufanget unntak i Python).
pub async fn blocking<T, F>(f: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> ApiResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::internal(format!("håndtereren krasjet: {e}")))?
}

/// Tilgangsporten for desktop-modus (`create_app(session_token=…, origin=…)`).
#[derive(Debug, Clone)]
pub struct DesktopConfig {
    pub token: String,
    /// `http://127.0.0.1:<port>`.
    pub origin: String,
    /// Engangskoder for [`crate::desktop_access::SESSION_PATH`] (delt mellom klonene).
    launch_codes: Arc<std::sync::Mutex<Vec<String>>>,
}

impl DesktopConfig {
    pub fn new(token: String, origin: String) -> Self {
        Self {
            token,
            origin,
            launch_codes: Arc::default(),
        }
    }

    /// Legg til en engangskode og returner oppstartslenken som setter sesjons-cookien.
    pub fn add_launch_code(&self, code: String) -> String {
        let url = format!(
            "{}{}?code={code}",
            self.origin,
            crate::desktop_access::SESSION_PATH
        );
        self.codes().push(code);
        url
    }

    /// Bruk opp `code`. `true` hvis den var gyldig (og ikke brukt før).
    pub fn take_launch_code(&self, code: &str) -> bool {
        let mut codes = self.codes();
        match codes.iter().position(|c| !code.is_empty() && c == code) {
            Some(i) => {
                codes.swap_remove(i);
                true
            }
            None => false,
        }
    }

    fn codes(&self) -> std::sync::MutexGuard<'_, Vec<String>> {
        self.launch_codes.lock().unwrap_or_else(|e| e.into_inner())
    }
}

struct Inner {
    runner: JobRunner,
    derived_dir: PathBuf,
    spa: Option<Arc<dyn SpaSource>>,
    desktop: Option<DesktopConfig>,
}

/// Tilstanden alle håndtererne deler. Billig å klone.
#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

impl AppState {
    /// Som `create_app()` i Python: `/derived` låses til datamappa slik den er ved
    /// oppstart (Starlettes `StaticFiles`-mount), og SPA-en er den innebygde `web/dist`.
    pub fn new(desktop: Option<DesktopConfig>) -> std::io::Result<Self> {
        let store = make_storage();
        std::fs::create_dir_all(store.derived_dir())?;
        let spa = EmbeddedSpa::detect().map(|s| Arc::new(s) as Arc<dyn SpaSource>);
        Ok(Self::with_parts(store.derived_dir(), spa, desktop))
    }

    /// For tester og innbygging: eksplisitt `derived/`-mappe og SPA-kilde.
    pub fn with_parts(
        derived_dir: PathBuf,
        spa: Option<Arc<dyn SpaSource>>,
        desktop: Option<DesktopConfig>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                runner: JobRunner::default(),
                derived_dir,
                spa,
                desktop,
            }),
        }
    }

    /// Den ene jobbkjøreren for prosessen.
    pub fn runner(&self) -> &JobRunner {
        &self.inner.runner
    }

    pub fn derived_dir(&self) -> &std::path::Path {
        &self.inner.derived_dir
    }

    pub fn spa(&self) -> Option<&Arc<dyn SpaSource>> {
        self.inner.spa.as_ref()
    }

    pub fn desktop(&self) -> Option<&DesktopConfig> {
        self.inner.desktop.as_ref()
    }
}
