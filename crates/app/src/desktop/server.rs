//! Den innebygde axum-tjeneren på `127.0.0.1:<ledig port>` — motparten til
//! `fotomaxxer/desktop_server.py`.
//!
//! Socketen bindes før tjenertråden starter, så porten (og dermed origin) er kjent med en
//! gang, og forespørsler som kommer før tokio er i gang, venter i køen. Tjeneren kjører
//! med [`DesktopConfig`], så tilgangsporten (`desktop_access`) er aktiv.

use std::net::{Ipv4Addr, TcpListener};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use fotomaxxer_api::{AppState, DesktopConfig};
use tokio::sync::watch;

use super::log::{error, info, warning};

/// Hvor lenge åpne forbindelser får holde igjen avslutningen etter stoppsignalet.
const CONNECTION_GRACE: Duration = Duration::from_secs(5);

pub struct Server {
    pub origin: String,
    pub token: String,
    config: DesktopConfig,
    state: AppState,
    stop: watch::Sender<bool>,
    thread: Mutex<Option<JoinHandle<()>>>,
    error: Arc<Mutex<Option<String>>>,
}

/// 32 tilfeldige byte som heksadesimal tekst (sesjonstoken, engangskoder).
pub fn random_token() -> String {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).expect("operativsystemets tilfeldighetskilde");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

impl Server {
    /// Bind socketen og start tjenertråden. Datamappa må allerede være satt i
    /// `FOTOMAXXER_HOME` (tilstanden løser den derfra).
    pub fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let origin = format!("http://127.0.0.1:{port}");
        let token = random_token();
        let config = DesktopConfig::new(token.clone(), origin.clone());
        let state = AppState::new(Some(config.clone()))?;
        let (stop, stop_rx) = watch::channel(false);
        let error = Arc::new(Mutex::new(None));
        let thread = {
            let (state, error) = (state.clone(), error.clone());
            std::thread::Builder::new()
                .name("desktop-api".into())
                .spawn(move || {
                    if let Err(e) = run(listener, state, stop_rx) {
                        error!("Desktop-API feilet: {e}");
                        *error.lock().unwrap_or_else(|e| e.into_inner()) = Some(e.to_string());
                    }
                })?
        };
        info!("Tjeneren lytter på {origin}");
        Ok(Self {
            origin,
            token,
            config,
            state,
            stop,
            thread: Mutex::new(Some(thread)),
            error,
        })
    }

    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// En ny engangslenke som setter sesjons-cookien (for vinduet eller nettleseren).
    pub fn launch_url(&self) -> String {
        self.config.add_launch_code(random_token())
    }

    /// [`Self::launch_url`] som en funksjon andre tråder kan kalle.
    pub fn launcher(&self) -> impl Fn() -> String + Send + 'static {
        let config = self.config.clone();
        move || config.add_launch_code(random_token())
    }

    /// Steng jobbporten (skrivende forespørsler gir 503), be eventuelt jobben stoppe ved
    /// neste trygge punkt, og stopp tjeneren. Tjenertråden venter selv på jobben.
    pub fn request_stop(&self, cancel_job: bool) {
        let runner = self.state.runner();
        runner.begin_shutdown();
        if cancel_job && runner.is_running() {
            info!("Ber jobben stoppe ved neste trygge punkt");
            runner.request_cancel();
        }
        let _ = self.stop.send(true);
    }

    /// Er tjenertråden ferdig (tjeneren og jobben stoppet)?
    pub fn stopped(&self) -> bool {
        self.lock_thread()
            .as_ref()
            .is_none_or(JoinHandle::is_finished)
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Vent til tjenertråden er ferdig.
    pub fn wait(&self) {
        if let Some(thread) = self.lock_thread().take() {
            let _ = thread.join();
        }
    }

    fn lock_thread(&self) -> std::sync::MutexGuard<'_, Option<JoinHandle<()>>> {
        self.thread.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn run(
    listener: TcpListener,
    state: AppState,
    mut stop: watch::Receiver<bool>,
) -> std::io::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("desktop-api-worker")
        .build()?;
    let result = runtime.block_on(async {
        let listener = tokio::net::TcpListener::from_std(listener)?;
        let mut signal = stop.clone();
        let shutdown = async move {
            let _ = signal.wait_for(|v| *v).await;
        };
        tokio::select! {
            r = fotomaxxer_api::serve(listener, state.clone(), shutdown) => r,
            _ = async {
                let _ = stop.wait_for(|v| *v).await;
                tokio::time::sleep(CONNECTION_GRACE).await;
            } => {
                warning!("Åpne forbindelser ble ikke lukket i tide; avslutter likevel");
                Ok(())
            }
        }
    });
    // `serve` venter på jobben, men kan ha blitt avbrutt av tidsfristen over.
    state.runner().begin_shutdown();
    state.runner().wait(None);
    runtime.shutdown_timeout(Duration::from_secs(1));
    info!("Tjeneren og jobbkjøreren er stoppet");
    result
}
