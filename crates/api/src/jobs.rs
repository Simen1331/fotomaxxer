//! Bakgrunnsjobb-kjører: én jobb om gangen — motparten til `api/jobs.py`.
//!
//! Én bruker → maks **én** kjørende jobb. Et forsøk på å starte en til mens en kjører
//! gir [`JobBusy`] (ruterne svarer 409). Hver jobb kjører i en egen OS-tråd og åpner sin
//! **egen** SQLite-tilkobling — aldri delt med forespørslene.
//!
//! Tilstanden ([`JobState`]) er det `GET /api/jobs/current` viser. Jobben oppdaterer den
//! via [`JobHandle`] (Pythons `run.update(...)`). Når jobben er ferdig settes
//! `message = "Ferdig."` hvis jobben ikke satte en selv; en feil gir
//! `"Feil: {Type}: {melding}"` med Pythons unntaksnavn (se [`JobFailure`]).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

/// Synlig jobbtilstand (`runner._state` i Python).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct JobState {
    pub running: bool,
    pub kind: Option<String>,
    pub step: Option<String>,
    pub started_at: Option<String>,
    pub message: Option<String>,
    /// Fritt JSON-objekt (`{"scan_seen": …}`, `{"copied": …}`); rekkefølgen bevares.
    pub progress: Map<String, Value>,
    pub job_id: Option<String>,
    /// Kilden jobben er avgrenset til (`None` = alle eller ikke satt).
    pub source_id: Option<i64>,
}

/// En jobb kjører allerede (eller tjeneren avslutter) → HTTP 409.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobBusy;

/// Feilen en jobb ender med. `kind` er navnet på Python-unntaket som ville kommet
/// (`TaggingError`, `ValueError`, `OSError` …), så meldingen blir
/// `"Feil: {kind}: {message}"` som i `api/jobs.py`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobFailure {
    pub kind: String,
    pub message: String,
}

impl JobFailure {
    pub fn new(kind: &str, message: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for JobFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

impl From<fotomaxxer_core::Error> for JobFailure {
    /// Kjernefeil → nærmeste Python-unntak. Tekstene kan avvike fra Pythons der
    /// Python ville vist en unntakstekst fra et bibliotek (sqlite3, OSError).
    fn from(e: fotomaxxer_core::Error) -> Self {
        use fotomaxxer_core::Error;
        match e {
            Error::Db(e) => {
                let kind = match &e {
                    rusqlite::Error::SqliteFailure(f, _)
                        if f.code == rusqlite::ErrorCode::ConstraintViolation =>
                    {
                        "IntegrityError"
                    }
                    _ => "OperationalError",
                };
                Self::new(kind, e.to_string())
            }
            Error::Io(e) => {
                let kind = match e.kind() {
                    std::io::ErrorKind::NotFound => "FileNotFoundError",
                    std::io::ErrorKind::PermissionDenied => "PermissionError",
                    _ => "OSError",
                };
                Self::new(kind, e.to_string())
            }
            Error::Json(e) => Self::new("JSONDecodeError", e.to_string()),
            Error::Schema(m) => Self::new("RuntimeError", m),
            Error::Source(m) => Self::new("SourceError", m),
            Error::Profile(m) => Self::new("ProfileError", m),
            Error::Faces(m) => Self::new("ValueError", m),
            Error::Models(m) => Self::new("RuntimeError", m),
            // `ThreadPoolExecutor(max_workers=-1)` er en ValueError i Python.
            Error::Tagging(m) if m == MAX_WORKERS => Self::new("ValueError", m),
            Error::Tagging(m) => Self::new("TaggingError", m),
            Error::Cancelled => Self::new(CANCELLED, "avbrutt"),
        }
    }
}

/// `concurrent.futures`-meldingen for `max_workers <= 0`.
pub const MAX_WORKERS: &str = "max_workers must be greater than 0";

/// `JobFailure::kind` for en jobb som ble avbrutt ved et trygt punkt.
pub const CANCELLED: &str = "Cancelled";

/// `current.message` etter et avbrudd ([`JobRunner::request_cancel`]).
pub const CANCELLED_MESSAGE: &str =
    "Avbrutt. Det som var ferdig er lagret; neste kjøring fortsetter.";

struct Inner {
    closing: bool,
    /// Avbruddsflagget til jobben som kjører (nytt for hver jobb).
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    state: JobState,
}

/// Holder maks én kjørende jobb og dens synlige tilstand. Klones billig (delt).
#[derive(Clone)]
pub struct JobRunner {
    inner: Arc<Mutex<Inner>>,
}

impl Default for JobRunner {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                closing: false,
                cancel: Arc::default(),
                thread: None,
                state: JobState::default(),
            })),
        }
    }
}

/// Håndtaket jobben får for å rapportere fremdrift (Pythons `run`).
#[derive(Clone)]
pub struct JobHandle {
    runner: JobRunner,
}

impl JobHandle {
    /// `run.update(**kw)`: endre synlig tilstand.
    pub fn update(&self, f: impl FnOnce(&mut JobState)) {
        f(&mut self.runner.lock().state);
    }

    pub fn set_step(&self, step: &str) {
        self.update(|s| s.step = Some(step.to_string()));
    }

    pub fn set_message(&self, message: impl Into<String>) {
        let message = message.into();
        self.update(|s| s.message = Some(message));
    }

    /// Erstatter hele `progress` (som `run.update(progress={…})`).
    pub fn set_progress(&self, progress: Map<String, Value>) {
        self.update(|s| s.progress = progress);
    }

    pub fn set_source_id(&self, source_id: Option<i64>) {
        self.update(|s| s.source_id = source_id);
    }

    pub fn current(&self) -> JobState {
        self.runner.current()
    }
}

impl JobRunner {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        // En panikk i en jobb skjer aldri mens låsen holdes; forgiftning ignoreres.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn is_running(&self) -> bool {
        self.lock().state.running
    }

    pub fn is_closing(&self) -> bool {
        self.lock().closing
    }

    /// Steng jobbporten før vi venter; en ny jobb kan ikke snike seg inn.
    pub fn begin_shutdown(&self) {
        self.lock().closing = true;
    }

    /// Be jobben som kjører stoppe ved neste trygge punkt (se `fotomaxxer_core::cancel`).
    /// Brukes bare av desktop-modus; uten kall oppfører jobbene seg som før.
    pub fn request_cancel(&self) {
        self.lock().cancel.store(true, Ordering::Relaxed);
    }

    /// Vent til jobbtråden er ferdig (`None` = uten tidsgrense). `true` = ingen jobb kjører.
    pub fn wait(&self, timeout: Option<Duration>) -> bool {
        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            let mut inner = self.lock();
            let finished = inner.thread.as_ref().is_none_or(JoinHandle::is_finished);
            if finished {
                if let Some(handle) = inner.thread.take() {
                    drop(inner);
                    let _ = handle.join();
                }
                return true;
            }
            drop(inner);
            if deadline.is_some_and(|d| Instant::now() >= d) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Kopi av synlig tilstand.
    pub fn current(&self) -> JobState {
        self.lock().state.clone()
    }

    /// Start `target` i en bakgrunnstråd. [`JobBusy`] om en jobb kjører eller tjeneren
    /// avslutter. `target` må åpne sin egen databasetilkobling.
    pub fn start<F>(&self, kind: &str, target: F) -> Result<String, JobBusy>
    where
        F: FnOnce(&JobHandle) -> Result<(), JobFailure> + Send + 'static,
    {
        let mut inner = self.lock();
        if inner.closing || inner.state.running {
            return Err(JobBusy);
        }
        let job_id = new_job_id();
        inner.state = JobState {
            running: true,
            kind: Some(kind.to_string()),
            started_at: Some(now_iso()),
            job_id: Some(job_id.clone()),
            ..JobState::default()
        };
        // Den forrige (ferdige) tråden ryddes før en ny startes.
        if let Some(old) = inner.thread.take() {
            let _ = old.join();
        }
        let handle = JobHandle {
            runner: self.clone(),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        inner.cancel = cancel.clone();
        let thread = std::thread::Builder::new()
            .name(format!("job-{kind}"))
            .spawn(move || {
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    fotomaxxer_core::cancel::scoped(cancel, || target(&handle))
                }));
                handle.update(|s| {
                    match outcome {
                        Ok(Ok(())) => {
                            if s.message.is_none() {
                                s.message = Some("Ferdig.".into());
                            }
                        }
                        Ok(Err(failure)) if failure.kind == CANCELLED => {
                            s.message = Some(CANCELLED_MESSAGE.into());
                        }
                        Ok(Err(failure)) => s.message = Some(format!("Feil: {failure}")),
                        Err(panic) => {
                            let text = panic
                                .downcast_ref::<&str>()
                                .map(|s| s.to_string())
                                .or_else(|| panic.downcast_ref::<String>().cloned())
                                .unwrap_or_else(|| "ukjent".into());
                            s.message = Some(format!("Feil: panic: {text}"));
                        }
                    }
                    s.running = false;
                });
            });
        match thread {
            Ok(t) => {
                inner.thread = Some(t);
                Ok(job_id)
            }
            Err(e) => {
                inner.state.running = false;
                inner.state.message = Some(format!("Feil: RuntimeError: {e}"));
                Err(JobBusy)
            }
        }
    }
}

/// `uuid.uuid4().hex[:12]`: 12 tilfeldige heksadesimale sifre.
fn new_job_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    );
    format!("{:012x}", h.finish() & 0xffff_ffff_ffff)
}

/// `datetime.now(timezone.utc).isoformat(timespec="seconds")`, f.eks.
/// `2026-10-23T19:24:26+00:00`.
pub fn now_iso() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    iso_utc(secs)
}

fn iso_utc(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}+00:00",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Dager siden 1970-01-01 → (år, måned, dag) (Howard Hinnants algoritme).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn iso_tidsstempel_som_python() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00+00:00");
        assert_eq!(iso_utc(1_792_783_466), "2026-10-23T19:24:26+00:00");
        assert_eq!(iso_utc(951_782_400), "2000-02-29T00:00:00+00:00");
        let now = now_iso();
        assert_eq!(now.len(), 25);
        assert!(now.ends_with("+00:00"));
    }

    #[test]
    fn jobb_id_er_tolv_hex() {
        let a = new_job_id();
        assert_eq!(a.len(), 12);
        assert!(
            a.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_ne!(a, new_job_id());
    }

    #[test]
    fn en_jobb_om_gangen_og_ferdig_melding() {
        let runner = JobRunner::default();
        assert_eq!(runner.current(), JobState::default());
        let (tx, rx) = mpsc::channel::<()>();
        let id = runner
            .start("scan", move |run| {
                run.set_step("scan");
                run.set_source_id(Some(3));
                rx.recv().unwrap();
                Ok(())
            })
            .unwrap();
        assert_eq!(id.len(), 12);
        assert!(runner.is_running());
        assert_eq!(runner.start("derive", |_| Ok(())), Err(JobBusy));
        tx.send(()).unwrap();
        assert!(runner.wait(Some(Duration::from_secs(5))));
        let s = runner.current();
        assert!(!s.running);
        assert_eq!(s.kind.as_deref(), Some("scan"));
        assert_eq!(s.step.as_deref(), Some("scan"));
        assert_eq!(s.message.as_deref(), Some("Ferdig."));
        assert_eq!(s.source_id, Some(3));
        assert_eq!(s.job_id.as_deref(), Some(id.as_str()));
    }

    #[test]
    fn ny_jobb_nullstiller_tilstanden() {
        let runner = JobRunner::default();
        runner
            .start("a", |run| {
                run.set_source_id(Some(1));
                run.set_message("Egen melding.");
                Ok(())
            })
            .unwrap();
        runner.wait(None);
        assert_eq!(runner.current().message.as_deref(), Some("Egen melding."));
        runner.start("b", |_| Ok(())).unwrap();
        runner.wait(None);
        let s = runner.current();
        assert_eq!((s.kind.as_deref(), s.source_id), (Some("b"), None));
    }

    #[test]
    fn feil_og_panikk_blir_melding() {
        let runner = JobRunner::default();
        runner
            .start("tag", |run| {
                run.set_message("overskrives");
                Err(fotomaxxer_core::Error::Tagging("mangler nøkkel".into()).into())
            })
            .unwrap();
        runner.wait(None);
        assert_eq!(
            runner.current().message.as_deref(),
            Some("Feil: TaggingError: mangler nøkkel")
        );
        runner
            .start("x", |_| {
                Err(fotomaxxer_core::Error::Tagging(MAX_WORKERS.into()).into())
            })
            .unwrap();
        runner.wait(None);
        assert_eq!(
            runner.current().message.as_deref(),
            Some("Feil: ValueError: max_workers must be greater than 0")
        );
        runner.start("p", |_| panic!("au")).unwrap();
        runner.wait(None);
        let s = runner.current();
        assert_eq!(s.message.as_deref(), Some("Feil: panic: au"));
        assert!(!s.running);
    }

    #[test]
    fn avbrudd_ved_trygt_punkt() {
        let runner = JobRunner::default();
        let (tx, rx) = mpsc::channel::<()>();
        runner
            .start("scan", move |_| {
                for _ in 0..1000 {
                    fotomaxxer_core::cancel::check()?;
                    let _ = rx.recv_timeout(Duration::from_millis(5));
                }
                Ok(())
            })
            .unwrap();
        runner.request_cancel();
        assert!(runner.wait(Some(Duration::from_secs(5))));
        drop(tx);
        assert_eq!(runner.current().message.as_deref(), Some(CANCELLED_MESSAGE));
        // Neste jobb får et nytt (ikke satt) flagg.
        runner
            .start("scan", |_| Ok(fotomaxxer_core::cancel::check()?))
            .unwrap();
        runner.wait(None);
        assert_eq!(runner.current().message.as_deref(), Some("Ferdig."));
    }

    #[test]
    fn avslutning_stenger_porten() {
        let runner = JobRunner::default();
        runner.begin_shutdown();
        assert!(runner.is_closing());
        assert_eq!(runner.start("scan", |_| Ok(())), Err(JobBusy));
        assert!(runner.wait(Some(Duration::from_millis(10))));
    }
}
