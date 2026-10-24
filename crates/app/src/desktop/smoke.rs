//! `--smoke`: røyktest med det ekte vinduet — motparten til `fotomaxxer/desktop_smoke.py`.
//!
//! `fotomaxxer-gui --smoke <ny mappe> --smoke-image <bilde med ansikt>` sjekker at
//!
//! - React monteres i webview-en, og vinduet viser appens origin,
//! - navigasjon til en fremmed nettside og til `file://` avvises, og nye vinduer blokkeres,
//! - API-et uten token og med fremmed `Origin` gir 403,
//! - en ny start med samme `--home` henter frem denne instansen og avslutter med 0,
//! - «Oppdater alt» på en kilde med testbildet finner minst ett ansikt (uten
//!   OpenRouter-nøkkel, så AI-merkingen hoppes over),
//! - lukking under en jobb avbryter ved et trygt punkt (resten står som pending, og
//!   databasen er hel), og
//! - tjeneren og jobben er stoppet før prosessen avslutter.
//!
//! Rapporten skrives til `<mappe>/desktop-report.json` som
//! `{"ok": bool, "checks": [{"ok", "label"}], …}`; exit-kode 0 betyr at alt er ok.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fotomaxxer_api::desktop_access::SESSION_HEADER;
use fotomaxxer_api::jobs::CANCELLED_MESSAGE;
use serde_json::{Map, Value, json};
use tao::event_loop::EventLoopProxy;

use super::log::{error, info};
use super::server::Server;
use super::window::{AppWindow, UserEvent, is_local, lock};

/// Hele røyktesten skal bli ferdig innen ca. 3 minutter.
const DEADLINE: Duration = Duration::from_secs(170);
/// Første innlasting under xvfb (programvaregjengivelse) har tatt ca. 30 s i CI.
const MOUNT_DEADLINE: Duration = Duration::from_secs(90);
/// Antall bildevarianter i avbruddstesten (flere enn analysatorene rekker før avbruddet).
const CANCEL_IMAGES: usize = 24;

const MOUNT_JS: &str = r#"(() => {
  const root = document.getElementById('root');
  return {
    mounted: !!root && root.childElementCount > 0,
    text: ((root && root.innerText) || '').slice(0, 400),
    width: window.innerWidth,
    height: window.innerHeight,
    href: location.href,
  };
})()"#;

#[derive(Default)]
pub struct Report {
    checks: Vec<(bool, String)>,
    extra: Map<String, Value>,
}

impl Report {
    fn check(&mut self, ok: bool, label: &str) {
        info!("Røyktest: {} {label}", if ok { "OK  " } else { "FEIL" });
        self.checks.push((ok, label.to_string()));
    }
}

pub type SharedReport = Arc<Mutex<Report>>;

fn check(report: &SharedReport, ok: bool, label: &str) {
    lock(report).check(ok, label);
}

pub enum SmokeAction {
    None,
    /// Tidsfristen er nådd: lukk (med avbrudd).
    Close,
}

enum Phase {
    Mount,
    /// Navigasjonssjekkene: steg og når forrige steg startet.
    Navigation(usize, Instant),
    Worker,
    Closing,
}

pub struct Smoke {
    dir: PathBuf,
    image: PathBuf,
    home: PathBuf,
    /// Argumentene før `--home` for en ny start av samme exe (`["desktop"]` for CLI-en).
    relaunch_prefix: Vec<String>,
    report: SharedReport,
    started: Instant,
    phase: Phase,
    next_poll: Instant,
    eval: Arc<Mutex<Option<String>>>,
    eval_pending: bool,
    last_dom: Option<Value>,
    activations: Arc<AtomicUsize>,
    closing_job_id: Option<String>,
    cancel_source: Arc<Mutex<Option<i64>>>,
}

impl Smoke {
    pub fn new(
        dir: PathBuf,
        image: PathBuf,
        home: PathBuf,
        relaunch_prefix: Vec<String>,
        activations: Arc<AtomicUsize>,
    ) -> Self {
        let now = Instant::now();
        Self {
            dir,
            image,
            home,
            relaunch_prefix,
            report: Arc::default(),
            started: now,
            phase: Phase::Mount,
            next_poll: now,
            eval: Arc::default(),
            eval_pending: false,
            last_dom: None,
            activations,
            closing_job_id: None,
            cancel_source: Arc::default(),
        }
    }

    /// Kalles hvert 100. ms fra hendelsesløkken.
    pub fn tick(
        &mut self,
        app: &AppWindow,
        server: &Server,
        proxy: &EventLoopProxy<UserEvent>,
    ) -> SmokeAction {
        if self.started.elapsed() > DEADLINE && !matches!(self.phase, Phase::Closing) {
            check(
                &self.report,
                false,
                "Røyktesten ble ferdig innen tidsfristen",
            );
            self.phase = Phase::Closing;
            return SmokeAction::Close;
        }
        match self.phase {
            Phase::Mount => self.tick_mount(app),
            Phase::Navigation(step, at) => self.tick_navigation(app, server, proxy, step, at),
            Phase::Worker | Phase::Closing => SmokeAction::None,
        }
    }

    fn tick_mount(&mut self, app: &AppWindow) -> SmokeAction {
        if self.started.elapsed() > MOUNT_DEADLINE {
            let last = self.last_dom.take().unwrap_or(Value::Null);
            lock(&self.report).extra.insert("dom".into(), last);
            check(
                &self.report,
                false,
                "React monteres i webview-en innen tidsfristen",
            );
            self.phase = Phase::Closing;
            return SmokeAction::Close;
        }
        if self.eval_pending {
            let Some(result) = lock(&self.eval).take() else {
                // Et kall som startet før siden var lastet, kan bli borte; prøv igjen.
                if Instant::now() >= self.next_poll + Duration::from_secs(2) {
                    self.eval_pending = false;
                }
                return SmokeAction::None;
            };
            self.eval_pending = false;
            let dom: Value = serde_json::from_str(&result).unwrap_or(Value::String(result));
            let text = dom["text"].as_str().unwrap_or("");
            let ready = dom["mounted"] == true
                && !text.is_empty()
                && !text.contains("Laster")
                && dom["width"].as_f64().unwrap_or(0.0) >= 300.0
                && dom["height"].as_f64().unwrap_or(0.0) >= 200.0;
            self.last_dom = Some(dom.clone());
            if ready {
                let mut r = lock(&self.report);
                r.check(true, "React monteres i webview-en");
                r.extra.insert("dom".into(), dom);
                r.extra.insert(
                    "ui_ready_seconds".into(),
                    json!(round3(self.started.elapsed().as_secs_f64())),
                );
                drop(r);
                self.phase = Phase::Navigation(0, Instant::now());
                return SmokeAction::None;
            }
        }
        if !self.eval_pending && Instant::now() >= self.next_poll {
            self.next_poll = Instant::now() + Duration::from_millis(300);
            let slot = self.eval.clone();
            self.eval_pending = app
                .webview
                .evaluate_script_with_callback(MOUNT_JS, move |result| {
                    *lock(&slot) = Some(result);
                })
                .is_ok();
        }
        SmokeAction::None
    }

    fn tick_navigation(
        &mut self,
        app: &AppWindow,
        server: &Server,
        proxy: &EventLoopProxy<UserEvent>,
        step: usize,
        at: Instant,
    ) -> SmokeAction {
        let foreign = "https://example.com/";
        let local_file = if cfg!(windows) {
            "file:///C:/Windows/win.ini"
        } else {
            "file:///etc/passwd"
        };
        let url_is_local = || {
            app.webview
                .url()
                .is_ok_and(|u| is_local(&u, &server.origin))
        };
        let wait = Duration::from_millis(1500);
        match step {
            0 => {
                check(
                    &self.report,
                    url_is_local(),
                    "Vinduet viser appens tildelte origin",
                );
                let _ = app
                    .webview
                    .evaluate_script(&format!("location.href = {foreign:?}"));
            }
            1 | 2 if at.elapsed() < wait => return SmokeAction::None,
            1 => {
                let blocked = lock(&app.blocked)
                    .navigations
                    .iter()
                    .any(|u| u.starts_with(foreign));
                check(
                    &self.report,
                    blocked && url_is_local(),
                    "Navigasjon til en fremmed nettside avvises",
                );
                let _ = app
                    .webview
                    .evaluate_script(&format!("location.href = {local_file:?}"));
            }
            2 => {
                // WebKit kan stoppe `file://` fra en http-side selv før handleren spørres;
                // det som teller, er at vinduet blir på appens origin.
                let seen = lock(&app.blocked)
                    .navigations
                    .iter()
                    .any(|u| u.starts_with("file:"));
                lock(&self.report)
                    .extra
                    .insert("file_navigation_seen_by_handler".into(), json!(seen));
                check(
                    &self.report,
                    url_is_local(),
                    "Navigasjon til lokale filer avvises",
                );
                let _ = app
                    .webview
                    .evaluate_script("window.open('https://example.com/nytt-vindu')");
            }
            3 if at.elapsed() < wait => return SmokeAction::None,
            _ => {
                let new_windows = lock(&app.blocked).new_windows.len();
                lock(&self.report)
                    .extra
                    .insert("new_windows_blocked".into(), json!(new_windows));
                check(
                    &self.report,
                    url_is_local(),
                    "Nytt vindu (window.open) åpnes ikke, og vinduet blir på appens origin",
                );
                self.spawn_worker(server, proxy.clone());
                self.phase = Phase::Worker;
                return SmokeAction::None;
            }
        }
        self.phase = Phase::Navigation(step + 1, Instant::now());
        SmokeAction::None
    }

    fn spawn_worker(&self, server: &Server, proxy: EventLoopProxy<UserEvent>) {
        let ctx = Worker {
            port: server.origin.rsplit(':').next().unwrap_or("0").to_string(),
            token: server.token.clone(),
            report: self.report.clone(),
            dir: self.dir.clone(),
            image: self.image.clone(),
            home: self.home.clone(),
            relaunch_prefix: self.relaunch_prefix.clone(),
            activations: self.activations.clone(),
            cancel_source: self.cancel_source.clone(),
        };
        let spawned = std::thread::Builder::new()
            .name("desktop-smoke".into())
            .spawn(move || {
                if let Err(e) = ctx.run() {
                    error!("Røyktesten feilet: {e}");
                    lock(&ctx.report).extra.insert("error".into(), json!(e));
                    ctx.check(false, "Røyktesten fullførte");
                }
                // Lukk uansett (som brukeren, med avbrudd), så rapporten skrives.
                let _ = proxy.send_event(UserEvent::SmokeClose);
            });
        if let Err(e) = spawned {
            check(
                &self.report,
                false,
                &format!("Røyktesttråden startet ({e})"),
            );
        }
    }

    /// Like før lukkingen starter (fra [`UserEvent::SmokeClose`]).
    pub fn on_close(&mut self, server: &Server) {
        self.phase = Phase::Closing;
        let runner = server.state().runner();
        let running = runner.is_running();
        self.closing_job_id = runner.current().job_id;
        check(
            &self.report,
            running,
            "Lukking starter mens en bakgrunnsjobb kjører",
        );
    }

    pub fn after_close_started(&mut self, server: &Server) {
        check(
            &self.report,
            server.state().runner().is_closing(),
            "Nye jobber er stengt før avslutning",
        );
    }

    /// Etter hendelsesløkken: siste sjekker, rapporten og exit-koden.
    pub fn finish(self, server: &Server, _app: &AppWindow) -> i32 {
        let runner = server.state().runner();
        let stopped = server.stopped() && !runner.is_running() && runner.wait(Some(Duration::ZERO));
        let mut report = lock(&self.report);
        report.check(
            stopped,
            "Tjener og bakgrunnsjobb er stoppet før prosessen avslutter",
        );
        let job = runner.current();
        if self.closing_job_id.is_some() && job.job_id == self.closing_job_id {
            report.check(
                job.message.as_deref() == Some(CANCELLED_MESSAGE),
                "Jobben stoppet ved et trygt punkt etter lukking",
            );
            report.extra.insert(
                "cancelled_job".into(),
                json!({"kind": job.kind, "step": job.step, "message": job.message}),
            );
        }
        if let Some(source_id) = *lock(&self.cancel_source) {
            match db_after_cancel(&self.home, source_id) {
                Ok((pending, done, integrity)) => {
                    report.extra.insert(
                        "after_cancel".into(),
                        json!({"faces_pending": pending, "faces_done": done, "integrity": integrity}),
                    );
                    report.check(
                        pending > 0,
                        "Avbruddet kom før alt var behandlet (resten står som pending)",
                    );
                    report.check(integrity == "ok", "Databasen er hel etter avbruddet");
                }
                Err(e) => {
                    report.check(false, &format!("Databasen kan leses etter avbruddet ({e})"))
                }
            }
        }
        report.extra.insert(
            "seconds".into(),
            json!(round3(self.started.elapsed().as_secs_f64())),
        );
        let ok = !report.extra.contains_key("error") && report.checks.iter().all(|(ok, _)| *ok);
        let mut out = Map::new();
        out.insert("ok".into(), json!(ok));
        out.insert(
            "checks".into(),
            Value::Array(
                report
                    .checks
                    .iter()
                    .map(|(ok, label)| json!({"ok": ok, "label": label}))
                    .collect(),
            ),
        );
        out.insert("origin".into(), json!(server.origin));
        out.extend(std::mem::take(&mut report.extra));
        let path = self.dir.join("desktop-report.json");
        let text = serde_json::to_string_pretty(&Value::Object(out)).unwrap_or_default();
        if let Err(e) = std::fs::write(&path, text) {
            error!("Kunne ikke skrive {}: {e}", path.display());
            return 1;
        }
        info!(
            "Røyktest {}: {}",
            if ok { "OK" } else { "FEILET" },
            path.display()
        );
        if ok { 0 } else { 1 }
    }
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

fn db_after_cancel(home: &Path, source_id: i64) -> Result<(i64, i64, String), String> {
    let store = fotomaxxer_core::Storage::new(home);
    let conn = fotomaxxer_core::db::connect(&store.db_path()).map_err(|e| e.to_string())?;
    let count = |status: &str| -> rusqlite::Result<i64> {
        conn.query_row(
            "SELECT COUNT(*) FROM processing_state WHERE stage = 'faces' AND status = ? \
             AND image_id IN (SELECT image_id FROM files WHERE source_id = ?)",
            rusqlite::params![status, source_id],
            |r| r.get(0),
        )
    };
    let pending = count("pending").map_err(|e| e.to_string())?;
    let done = count("done").map_err(|e| e.to_string())?;
    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    Ok((pending, done, integrity))
}

/// Arbeidet utenfor hovedtråden: HTTP-sjekker, ny start og jobbene.
struct Worker {
    port: String,
    token: String,
    report: SharedReport,
    dir: PathBuf,
    image: PathBuf,
    home: PathBuf,
    relaunch_prefix: Vec<String>,
    activations: Arc<AtomicUsize>,
    cancel_source: Arc<Mutex<Option<i64>>>,
}

struct Reply {
    status: u16,
    body: String,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

impl Worker {
    fn check(&self, ok: bool, label: &str) {
        check(&self.report, ok, label);
    }

    /// En enkel HTTP/1.1-forespørsel med `Connection: close`.
    fn http(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&Value>,
    ) -> Result<Reply, String> {
        let addr = format!("127.0.0.1:{}", self.port);
        let mut stream = TcpStream::connect(&addr).map_err(|e| format!("{addr}: {e}"))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .map_err(|e| e.to_string())?;
        let body = body.map(Value::to_string).unwrap_or_default();
        let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        if !body.is_empty() {
            req.push_str("Content-Type: application/json\r\n");
        }
        req.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
        stream
            .write_all(req.as_bytes())
            .map_err(|e| e.to_string())?;
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).map_err(|e| e.to_string())?;
        let raw = String::from_utf8_lossy(&raw);
        let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| format!("ugyldig svar på {method} {path}"))?;
        Ok(Reply {
            status,
            body: body.to_string(),
        })
    }

    /// Med sesjonstokenet (som vinduet).
    fn api(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Reply, String> {
        self.http(method, path, &[(SESSION_HEADER, &self.token)], body)
    }

    fn run(&self) -> Result<(), String> {
        let r = self.http("GET", "/api/health", &[], None)?;
        self.check(r.status == 403, "API uten token avvises (403)");
        let r = self.http(
            "GET",
            "/api/health",
            &[
                (SESSION_HEADER, &self.token),
                ("Origin", "https://example.com"),
            ],
            None,
        )?;
        self.check(r.status == 403, "API med fremmed Origin avvises (403)");
        let r = self.api("GET", "/api/health", None)?;
        self.check(r.status == 200, "API med token svarer");

        self.second_instance()?;
        self.update_finds_face()?;
        self.start_cancel_job()
    }

    fn second_instance(&self) -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let before = self.activations.load(Ordering::SeqCst);
        let mut child = Command::new(exe)
            .args(&self.relaunch_prefix)
            .arg("--home")
            .arg(&self.home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("ny start: {e}"))?;
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            match child.try_wait().map_err(|e| e.to_string())? {
                Some(status) => break Some(status),
                None if Instant::now() > deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                None => std::thread::sleep(Duration::from_millis(50)),
            }
        };
        let code = status.and_then(|s| s.code());
        let activated = self.activations.load(Ordering::SeqCst) > before;
        lock(&self.report).extra.insert(
            "second_instance".into(),
            json!({"exit_code": code, "activated": activated}),
        );
        self.check(
            code == Some(0) && activated,
            "Ny oppstart med samme datamappe fokuserer eksisterende instans (kode 0)",
        );
        Ok(())
    }

    fn add_source(&self, dir: &Path) -> Result<i64, String> {
        let r = self.api(
            "POST",
            "/api/sources",
            Some(&json!({"path": dir.to_string_lossy()})),
        )?;
        r.json()["id"]
            .as_i64()
            .ok_or_else(|| format!("kunne ikke legge til kilden ({}): {}", r.status, r.body))
    }

    /// Vent til jobben er ferdig; returnerer `/api/jobs/current`.
    fn wait_job(&self, timeout: Duration) -> Result<Value, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let cur = self.api("GET", "/api/jobs/current", None)?.json();
            if cur["running"] == false {
                return Ok(cur);
            }
            if Instant::now() > deadline {
                return Err(format!("jobben ble ikke ferdig: {cur}"));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn update_finds_face(&self) -> Result<(), String> {
        let photos = self.dir.join("bilder");
        std::fs::create_dir_all(&photos).map_err(|e| e.to_string())?;
        let name = self.image.file_name().ok_or("bildet mangler filnavn")?;
        std::fs::copy(&self.image, photos.join(name)).map_err(|e| e.to_string())?;
        let source = self.add_source(&photos)?;
        let r = self.api(
            "POST",
            "/api/jobs/update",
            Some(&json!({"source_id": source})),
        )?;
        if r.status != 202 {
            return Err(format!(
                "«Oppdater alt» ble ikke startet ({}): {}",
                r.status, r.body
            ));
        }
        let started = Instant::now();
        let cur = self.wait_job(Duration::from_secs(90))?;
        let progress = self.api("GET", "/api/progress", None)?.json();
        let faces = progress["faces_total"].as_i64().unwrap_or(0);
        let message = cur["message"].as_str().unwrap_or("").to_string();
        lock(&self.report).extra.insert(
            "pipeline".into(),
            json!({
                "message": message,
                "seconds": round3(started.elapsed().as_secs_f64()),
                "progress": progress,
            }),
        );
        self.check(
            !message.starts_with("Feil"),
            "«Oppdater alt» fullførte uten feil",
        );
        self.check(faces >= 1, "«Oppdater alt» finner minst ett ansikt");
        Ok(())
    }

    /// Start «Oppdater alt» på mange bildevarianter og be om lukking når jobben er i
    /// ansiktssteget (selve lukkingen skjer på hovedtråden).
    fn start_cancel_job(&self) -> Result<(), String> {
        let dir = self.dir.join("bilder-avbrudd");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let bytes = std::fs::read(&self.image).map_err(|e| e.to_string())?;
        let ext = self
            .image
            .extension()
            .map(|e| e.to_string_lossy().into_owned())
            .unwrap_or_else(|| "jpg".into());
        for i in 0..CANCEL_IMAGES {
            // Ulike bytes etter bildet gir ulike innholds-hasher (egne bilder), samme piksler.
            let mut data = bytes.clone();
            data.extend_from_slice(format!("fotomaxxer-smoke-{i}").as_bytes());
            std::fs::write(dir.join(format!("variant-{i:02}.{ext}")), data)
                .map_err(|e| e.to_string())?;
        }
        let source = self.add_source(&dir)?;
        *lock(&self.cancel_source) = Some(source);
        let r = self.api(
            "POST",
            "/api/jobs/update",
            Some(&json!({"source_id": source})),
        )?;
        if r.status != 202 {
            return Err(format!(
                "avbruddsjobben ble ikke startet ({}): {}",
                r.status, r.body
            ));
        }
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let cur = self.api("GET", "/api/jobs/current", None)?.json();
            if cur["running"] == false {
                return Err(format!("avbruddsjobben ble ferdig før lukking: {cur}"));
            }
            if cur["step"] == "faces-detect" {
                return Ok(()); // lukkingen sendes av tråden når vi returnerer
            }
            if Instant::now() > deadline {
                return Err(format!("avbruddsjobben nådde ikke ansiktssteget: {cur}"));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
