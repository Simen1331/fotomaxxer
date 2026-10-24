//! Desktop-modus: eget vindu mot den innebygde tjeneren — motparten til
//! `fotomaxxer/desktop.py` (oppstart), `desktop_window.py`, `desktop_server.py` og
//! `desktop_smoke.py`.
//!
//! Oppstart:
//! 1. Datamappa: `--home` > `FOTOMAXXER_HOME` > pekerfil > standard
//!    (`%LOCALAPPDATA%\FotoMaxxer`, ellers `$XDG_DATA_HOME/fotomaxxer` eller
//!    `~/.local/share/fotomaxxer`). Første gang standarden velges, skrives pekerfila, som
//!    `data_root` i Python, så en datamappe fra Python-pakka tas over uendret.
//! 2. Én instans per datamappe ([`instance`]): en ny start henter frem vinduet og avslutter
//!    med kode 0.
//! 3. Logg i `<datamappe>/logs/desktop.log` ([`log`]), `init_db`, så tjeneren ([`server`]).
//! 4. Vinduet ([`window`]). Kan det ikke lages (f.eks. uten WebView2), åpnes
//!    standardnettleseren mot tjeneren i stedet, og prosessen lever til brukeren trykker OK.
//!
//! Feil ved oppstart vises i en dialog med stien til loggen.

pub mod dialog;
pub mod instance;
pub mod log;
pub mod server;
pub mod smoke;
pub mod window;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, OnceLock};

use fotomaxxer_core::{Storage, db, paths, storage};
use tao::event_loop::{EventLoop, EventLoopBuilder};

use self::log::{error, info, warning};
use self::server::Server;
use self::window::UserEvent;

/// Flaggene til `fotomaxxer-gui` og `fotomaxxer desktop`.
#[derive(Debug, Default, Clone)]
pub struct Args {
    pub home: Option<PathBuf>,
    /// Røyktest: ny rapportmappe.
    pub smoke: Option<PathBuf>,
    /// Røyktest: bilde med minst ett ansikt.
    pub smoke_image: Option<PathBuf>,
    /// Argumentene før `--home` når røyktesten starter samme exe på nytt.
    pub relaunch_prefix: Vec<String>,
}

static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// `\n\nLogg: <sti>` når loggen er startet.
pub fn log_hint() -> String {
    LOG_PATH
        .get()
        .map(|p| format!("\n\nLogg: {}", p.display()))
        .unwrap_or_default()
}

/// Datamappa og om førstegangsvalget må lagres i pekerfila (`data_root` i `desktop.py`).
pub fn data_root(explicit: Option<&Path>) -> (PathBuf, bool) {
    let env_set = std::env::var_os(storage::ENV_HOME).is_some_and(|v| !v.is_empty());
    if explicit.is_some() || env_set || storage::read_pointer().is_some() {
        return (storage::resolve_root(explicit), false);
    }
    let var = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let root = if cfg!(windows) {
        var("LOCALAPPDATA")
            .unwrap_or_else(|| paths::home_dir().join("AppData").join("Local"))
            .join("FotoMaxxer")
    } else {
        var("XDG_DATA_HOME")
            .unwrap_or_else(|| paths::home_dir().join(".local").join("share"))
            .join("fotomaxxer")
    };
    (paths::resolve(&paths::expand_user(&root)), true)
}

/// Start desktop-modus. Exit-kode 0 = ok (også når en annen instans ble hentet frem).
pub fn run(args: Args) -> ExitCode {
    log::install_panic_hook();
    match start(args) {
        Ok(code) => {
            info!("Desktop avsluttet (kode {code})");
            ExitCode::from(code)
        }
        Err(message) => {
            error!("Desktop-oppstart feilet: {message}");
            dialog::error(&format!(
                "FotoMaxxer kunne ikke starte.\n\n{message}{}",
                log_hint()
            ));
            ExitCode::from(1)
        }
    }
}

fn start(mut args: Args) -> Result<u8, String> {
    // Røyktesten: ny mappe, datamappe under den, og aldri pekerfila.
    let smoke = match (&args.smoke, &args.smoke_image) {
        (None, None) => None,
        (Some(dir), Some(image)) if !dir.exists() && image.is_file() => {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let dir = paths::resolve(dir);
            if args.home.is_none() {
                args.home = Some(dir.join("library"));
            }
            Some((dir, paths::resolve(image)))
        }
        _ => {
            return Err(
                "Røyktesten trenger en ny rapportmappe (--smoke) og et lesbart bilde (--smoke-image)."
                    .into(),
            );
        }
    };
    let (root, first_run) = data_root(args.home.as_deref());
    std::fs::create_dir_all(&root).map_err(|e| format!("{}: {e}", root.display()))?;

    let instance = match instance::acquire(&root).map_err(|e| format!("låsfila: {e}"))? {
        instance::Acquire::Owned(i) => i,
        instance::Acquire::Busy => {
            if instance::activate_existing(&root) {
                return Ok(0);
            }
            return Err(
                "FotoMaxxer bruker allerede denne datamappa, men vinduet svarer ikke.".into(),
            );
        }
    };

    let log_path = log::init(&root).map_err(|e| format!("loggen: {e}"))?;
    let _ = LOG_PATH.set(log_path);
    info!(
        "Desktop starter (versjon {}); datamappe: {}",
        env!("CARGO_PKG_VERSION"),
        root.display()
    );
    if fotomaxxer_api::spa::EmbeddedSpa::detect().is_none() {
        return Err("Grensesnittet mangler. Bygg web/dist med npm ci og npm run build.".into());
    }
    let store = Storage::new(&root);
    store.ensure_dirs().map_err(|e| e.to_string())?;
    let conn = db::connect(&store.db_path()).map_err(|e| e.to_string())?;
    db::init_db(&conn).map_err(|e| e.to_string())?;
    drop(conn);
    if first_run && smoke.is_none() {
        storage::write_pointer(&root).map_err(|e| format!("pekerfila: {e}"))?;
        info!("Skrev pekerfila {}", storage::config_path().display());
    }
    // Tjeneren løser datamappa fra miljøet for hver forespørsel (som `serve --home`).
    // SAFETY: ingen andre tråder leser miljøet ennå (bare loggen og panikkhåndtereren finnes).
    unsafe { std::env::set_var(storage::ENV_HOME, &root) };

    let server = Server::start().map_err(|e| format!("tjeneren: {e}"))?;
    let secret = server::random_token();

    let event_loop = match build_event_loop() {
        Ok(l) => l,
        Err(e) => {
            warning!("Hendelsesløkken kunne ikke lages: {e}");
            return Ok(fallback(&server, &instance, secret, &e));
        }
    };
    let webview_dir = webview_dir(smoke.as_ref().map(|(d, _)| d.as_path()));
    let app = match window::create(&event_loop, &server, webview_dir.as_deref()) {
        Ok(app) => app,
        Err(e) => {
            warning!("Vinduet kunne ikke lages: {e}");
            drop(event_loop);
            return Ok(fallback(&server, &instance, secret, &e));
        }
    };
    let proxy = event_loop.create_proxy();
    instance
        .listen(secret, move || {
            proxy.send_event(UserEvent::Activate).is_ok()
        })
        .map_err(|e| format!("instanskanalen: {e}"))?;
    info!("Vinduet er åpnet mot {}", server.origin);

    let activations = Arc::new(AtomicUsize::new(0));
    let smoke = smoke.map(|(dir, image)| {
        info!("Røyktest: rapport i {}", dir.display());
        smoke::Smoke::new(
            dir,
            image,
            root.clone(),
            args.relaunch_prefix.clone(),
            activations.clone(),
        )
    });
    let code = window::run(event_loop, app, &server, activations, smoke);
    drop(instance);
    Ok(code as u8)
}

/// tao kaster panikk når GTK ikke kan startes (ingen skjerm); det blir en vanlig feil.
fn build_event_loop() -> Result<EventLoop<UserEvent>, String> {
    #[cfg(target_os = "linux")]
    gtk::init().map_err(|e| format!("GTK kunne ikke startes (ingen skjerm?): {e}"))?;
    std::panic::catch_unwind(|| EventLoopBuilder::<UserEvent>::with_user_event().build()).map_err(
        |panic| {
            panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "ukjent feil".into())
        },
    )
}

/// WebView2 trenger en skrivbar mappe for brukerdata (standarden ved siden av exe-en kan
/// være skrivebeskyttet). Den holdes utenfor datamappa, så pakka forblir flyttbar.
/// WebKitGTK kjører i inkognito og trenger ingen.
fn webview_dir(smoke_dir: Option<&Path>) -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    if let Some(dir) = smoke_dir {
        return Some(dir.join("webview"));
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| paths::home_dir().join("AppData").join("Local"));
    Some(base.join("FotoMaxxer").join("WebView2"))
}

/// Åpne `url` i standardnettleseren.
fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    let mut cmd = {
        // `start` i cmd tolker `&` o.l.; url.dll-handleren tar URL-en som den er.
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler").arg(url);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(not(any(windows, target_os = "macos")))]
    let mut cmd = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}

/// Reserve uten vindu: åpne nettleseren med en engangslenke (setter sesjons-cookien) og
/// hold tjeneren oppe til brukeren trykker OK. En ny start åpner nettleseren på nytt.
/// Avslutningen er den samme som fra vinduet: jobbporten stenges, en pågående jobb
/// avbrytes ved neste trygge punkt, og prosessen venter til tjeneren og jobben er stoppet.
fn fallback(server: &Server, instance: &instance::Instance, secret: String, why: &str) -> u8 {
    let launch = |server: &Server| {
        let url = server.launch_url();
        if let Err(e) = open_browser(&url) {
            error!("Kunne ikke åpne nettleseren: {e}");
        }
    };
    info!(
        "Reserve: åpner standardnettleseren mot {} ({why})",
        server.origin
    );
    launch(server);
    let launcher = server.launcher();
    if let Err(e) = instance.listen(secret, move || {
        let url = launcher();
        open_browser(&url).is_ok()
    }) {
        warning!("Instanskanalen kunne ikke startes: {e}");
    }
    let shown = dialog::info(&format!(
        "Vinduet kunne ikke åpnes, så FotoMaxxer kjører i nettleseren på {}.\n\n\
         Trykk OK for å avslutte.{}",
        server.origin,
        log_hint()
    ));
    if !shown {
        // Uten skjerm: lev til Ctrl-C/SIGTERM, som `serve`.
        info!("Reserve uten dialog: venter på Ctrl-C");
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt.block_on(fotomaxxer_api::shutdown_signal()),
            Err(e) => error!("Kunne ikke vente på signal: {e}"),
        }
    }
    info!("Reserve: avslutter");
    server.request_stop(true);
    server.wait();
    0
}
