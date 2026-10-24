//! `<datamappe>/logs/desktop.log` med enkel rotasjon (~2 MB × 3), som Pythons
//! `RotatingFileHandler(maxBytes=2_000_000, backupCount=3)` i `fotomaxxer/desktop.py`.
//! Panikk logges også ([`install_panic_hook`]).

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const MAX_BYTES: u64 = 2_000_000;
const BACKUPS: u32 = 3;

struct Logger {
    path: PathBuf,
    file: File,
    size: u64,
}

static LOG: OnceLock<Mutex<Logger>> = OnceLock::new();

fn open(path: &Path) -> std::io::Result<(File, u64)> {
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    let size = file.metadata()?.len();
    Ok((file, size))
}

/// Start loggingen. Returnerer stien til loggfila.
pub fn init(root: &Path) -> std::io::Result<PathBuf> {
    let path = root.join("logs").join("desktop.log");
    fs::create_dir_all(path.parent().expect("logs/"))?;
    let (file, size) = open(&path)?;
    let _ = LOG.set(Mutex::new(Logger {
        path: path.clone(),
        file,
        size,
    }));
    Ok(path)
}

/// `desktop.log` → `.1` → `.2` → `.3` (den eldste forsvinner).
fn rotate(log: &mut Logger) -> std::io::Result<()> {
    let backup = |n: u32| PathBuf::from(format!("{}.{n}", log.path.display()));
    let _ = fs::remove_file(backup(BACKUPS));
    for n in (1..BACKUPS).rev() {
        let _ = fs::rename(backup(n), backup(n + 1));
    }
    // Windows kan ikke gi nytt navn til en åpen fil: bytt håndtaket først.
    let (tmp, _) = open(&log.path.with_extension("log.tmp"))?;
    log.file = tmp;
    fs::rename(&log.path, backup(1))?;
    let (file, size) = open(&log.path)?;
    log.file = file;
    log.size = size;
    let _ = fs::remove_file(log.path.with_extension("log.tmp"));
    Ok(())
}

/// Skriv én linje: `2026-10-24T12:00:00+00:00 INFO melding`. Uten [`init`] går linja
/// til stderr.
pub fn write(level: &str, message: &str) {
    let line = format!("{} {level} {message}\n", fotomaxxer_api::jobs::now_iso());
    let Some(log) = LOG.get() else {
        eprint!("{line}");
        return;
    };
    let mut log = log.lock().unwrap_or_else(|e| e.into_inner());
    if log.size > 0 && log.size + line.len() as u64 > MAX_BYTES {
        let _ = rotate(&mut log);
    }
    if log.file.write_all(line.as_bytes()).is_ok() {
        log.size += line.len() as u64;
    }
    let _ = log.file.flush();
}

/// Logg panikk (med backtrace) før standardhåndtereren kjører.
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current()
            .name()
            .unwrap_or("ukjent")
            .to_string();
        let backtrace = std::backtrace::Backtrace::force_capture();
        write("PANIC", &format!("tråd «{thread}»: {info}\n{backtrace}"));
        default(info);
    }));
}

macro_rules! info {
    ($($arg:tt)*) => { $crate::desktop::log::write("INFO", &format!($($arg)*)) };
}
macro_rules! warning {
    ($($arg:tt)*) => { $crate::desktop::log::write("WARNING", &format!($($arg)*)) };
}
macro_rules! error {
    ($($arg:tt)*) => { $crate::desktop::log::write("ERROR", &format!($($arg)*)) };
}
pub(crate) use {error, info, warning};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roterer_ved_grensen() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("desktop.log");
        fs::write(&path, vec![b'x'; MAX_BYTES as usize - 10]).unwrap();
        let (file, size) = open(&path).unwrap();
        let mut log = Logger {
            path: path.clone(),
            file,
            size,
        };
        rotate(&mut log).unwrap();
        assert_eq!(log.size, 0);
        assert_eq!(
            fs::metadata(format!("{}.1", path.display())).unwrap().len(),
            MAX_BYTES - 10
        );
        for _ in 0..4 {
            rotate(&mut log).unwrap();
        }
        assert!(Path::new(&format!("{}.3", path.display())).exists());
        assert!(!Path::new(&format!("{}.4", path.display())).exists());
    }
}
