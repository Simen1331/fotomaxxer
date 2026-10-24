//! Én instans per datamappe — motparten til `SingleInstance` i `fotomaxxer/desktop_window.py`.
//!
//! - **Lås:** `<datamappe>/.desktop.lock` med en eksklusiv OS-lås (`File::try_lock`). Låsen
//!   slippes av operativsystemet når prosessen dør, så en krasj etterlater ingen foreldet lås.
//! - **Kanal:** første instans lytter på `127.0.0.1:<ledig port>` og skriver port og en
//!   tilfeldig hemmelighet til `<datamappe>/.desktop-instance.json`. En ny start som ikke
//!   får låsen, sender `activate <hemmelighet>\n` dit og venter på `ok\n`; vinduet
//!   hentes da frem (en tao-hendelse), og den nye prosessen avslutter med kode 0.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::log::{info, warning};

const LOCK_FILE: &str = ".desktop.lock";
const INFO_FILE: &str = ".desktop-instance.json";

/// Låsen for datamappa. Holdes til prosessen avslutter.
pub struct Instance {
    _lock: File,
    info_path: PathBuf,
}

pub enum Acquire {
    Owned(Instance),
    /// En annen prosess har låsen.
    Busy,
}

pub fn acquire(root: &Path) -> std::io::Result<Acquire> {
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(LOCK_FILE))?;
    match lock.try_lock() {
        Ok(()) => Ok(Acquire::Owned(Instance {
            _lock: lock,
            info_path: root.join(INFO_FILE),
        })),
        Err(fs::TryLockError::WouldBlock) => Ok(Acquire::Busy),
        Err(fs::TryLockError::Error(e)) => Err(e),
    }
}

impl Instance {
    /// Lytt etter nye starter. `activate` henter frem vinduet og gir `true` om det lyktes.
    pub fn listen(
        &self,
        secret: String,
        activate: impl Fn() -> bool + Send + 'static,
    ) -> std::io::Result<()> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        let tmp = self.info_path.with_extension("json.tmp");
        fs::write(
            &tmp,
            json!({"port": port, "secret": secret, "pid": std::process::id()}).to_string(),
        )?;
        fs::rename(&tmp, &self.info_path)?;
        std::thread::Builder::new()
            .name("desktop-instance".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut line = String::new();
                    if BufReader::new(&stream).read_line(&mut line).is_err() {
                        continue;
                    }
                    if line.trim_end() == format!("activate {secret}") {
                        let ok = activate();
                        info!("Ny oppstart ba om å hente frem vinduet (ok={ok})");
                        if ok {
                            let _ = (&stream).write_all(b"ok\n");
                        }
                    } else {
                        warning!("Ugyldig melding på instanskanalen");
                    }
                }
            })?;
        Ok(())
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.info_path);
    }
}

/// Be instansen som eier datamappa om å hente frem vinduet. `true` hvis den svarte `ok`
/// innen tidsfristen (den kan være midt i oppstarten, så vi prøver i noen sekunder).
pub fn activate_existing(root: &Path) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if try_activate(&root.join(INFO_FILE)).unwrap_or(false) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

fn try_activate(info_path: &Path) -> std::io::Result<bool> {
    let data: Value = serde_json::from_str(&fs::read_to_string(info_path)?)?;
    let (Some(port), Some(secret)) = (data["port"].as_u64(), data["secret"].as_str()) else {
        return Ok(false);
    };
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port as u16));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(500))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.write_all(format!("activate {secret}\n").as_bytes())?;
    let mut reply = String::new();
    BufReader::new(&stream).read_line(&mut reply)?;
    Ok(reply.trim_end() == "ok")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn andre_start_henter_frem_den_forste() {
        let tmp = tempfile::tempdir().unwrap();
        let Acquire::Owned(first) = acquire(tmp.path()).unwrap() else {
            panic!("første start skal få låsen");
        };
        assert!(matches!(acquire(tmp.path()).unwrap(), Acquire::Busy));
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        first
            .listen("hemmelig".into(), move || {
                c.fetch_add(1, Ordering::SeqCst);
                true
            })
            .unwrap();
        assert!(activate_existing(tmp.path()));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        drop(first);
        assert!(!tmp.path().join(INFO_FILE).exists());
        assert!(matches!(acquire(tmp.path()).unwrap(), Acquire::Owned(_)));
    }
}
