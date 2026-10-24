//! Eksport til mappe som bakgrunnsjobb (`POST /export`).
//!
//! Motparten til `api/routers/export.py`: den eneste fil-skrivende operasjonen for album.
//! Kopierer originalen (`files.path`, første fil per bilde) nummerert etter album-`position`
//! → `001_<originalnavn>.jpg` i en undermappe oppkalt etter albumet (eller rett i målet for
//! et utvalg). Manglende original hoppes over og rapporteres. Kjøres som bakgrunnsjobb via
//! den delte jobbkjøreren (én jobb om gangen, 409 «busy» ellers). **PDF er ikke her.**
//!
//! Byte-likhet med Python krever litt mer enn `std::fs::copy`:
//! - Stier vises som `pathlib` gjør det ([`PyPath`]: `str(Path(p).expanduser())`), både i
//!   sluttmeldingen og i feiltekstene.
//! - Kopieringen er `shutil.copy2` ([`copy2`]): mappe som mål, `SameFileError` (en eksport
//!   til originalmappa med originalnavn **må** stoppe før originalen åpnes for skriving),
//!   navngitte rør, innhold, tidsstempler og rettigheter.
//! - Jobbfeil får Pythons unntaksnavn og `OSError`-tekst (`FileExistsError: [Errno 17] File
//!   exists: '/sti'`), via [`fotomaxxer_core::storage::py_os_error`].

mod pyword;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use axum::extract::State;
use fotomaxxer_core::settings as settings_mod;
use fotomaxxer_core::storage::{mkdir_parents, py_os_error, py_repr};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{Map, Value};

use super::Routes;
use super::jobs::{Accepted, JobResult, accept, job_conn};
use crate::error::{ApiError, ApiResult};
use crate::jobs::{JobFailure, JobHandle};
use crate::schemas::ExportRequest;
use crate::state::{AppState, blocking};
use crate::validate::DbReq;

pub fn routes() -> Routes {
    Routes::new().post("/export", export)
}

/// `_safe_dirname`: `re.sub(r"[^\w\-. æøåÆØÅ]+", "_", name).strip().strip(".") or "album"`.
///
/// `\w` er Pythons Unicode-definisjon ([`pyword::is_word`]); hver sammenhengende bit med
/// andre tegn blir én `_`. Etter byttet er mellomrom det eneste blanktegnet som kan stå
/// igjen, så `strip()` fjerner bare mellomrom.
pub fn safe_dirname(name: &str) -> String {
    let mut cleaned = String::with_capacity(name.len());
    let mut in_run = false;
    for c in name.chars() {
        if pyword::is_word(c) || matches!(c, '-' | '.' | ' ') {
            cleaned.push(c);
            in_run = false;
        } else if !in_run {
            cleaned.push('_');
            in_run = true;
        }
    }
    let cleaned = cleaned.trim_matches(' ').trim_matches('.');
    if cleaned.is_empty() {
        "album".into()
    } else {
        cleaned.into()
    }
}

// --- pathlib-stier ------------------------------------------------------------------

/// En sti slik `pathlib.Path` ser den: anker (`""`, `"/"`, `"//"` på POSIX) og deler uten
/// tomme biter og `.` (`..` beholdes). `str()` gir samme tekst som Python.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PyPath {
    anchor: String,
    parts: Vec<String>,
}

#[cfg(not(windows))]
const SEP: &str = "/";
#[cfg(windows)]
const SEP: &str = "\\";

impl PyPath {
    /// `Path(text)` (`_parse_path` + `posixpath.splitroot`).
    #[cfg(not(windows))]
    pub fn parse(text: &str) -> Self {
        let (anchor, rest) = if !text.starts_with('/') {
            ("", text)
        } else if !text[1..].starts_with('/') || text[2..].starts_with('/') {
            ("/", &text[1..])
        } else {
            ("//", &text[2..])
        };
        Self {
            anchor: anchor.into(),
            parts: rest
                .split('/')
                .filter(|p| !p.is_empty() && *p != ".")
                .map(str::to_string)
                .collect(),
        }
    }

    /// Windows: tilnærmet via `std::path` (stasjon/UNC-prefiks + rot, `/` som `\`).
    /// Stiteksten i meldinger er ikke sammenlignet med Python på Windows.
    #[cfg(windows)]
    pub fn parse(text: &str) -> Self {
        use std::path::Component;
        let mut anchor = String::new();
        let mut parts = Vec::new();
        for c in Path::new(text).components() {
            match c {
                Component::Prefix(p) => anchor.push_str(&p.as_os_str().to_string_lossy()),
                Component::RootDir => anchor.push('\\'),
                Component::CurDir => {}
                Component::ParentDir => parts.push("..".to_string()),
                Component::Normal(n) => parts.push(n.to_string_lossy().into_owned()),
            }
        }
        Self { anchor, parts }
    }

    /// `str(path)`.
    pub fn as_string(&self) -> String {
        let s = format!("{}{}", self.anchor, self.parts.join(SEP));
        if s.is_empty() { ".".into() } else { s }
    }

    /// Stien for filsystemkall.
    pub fn fs(&self) -> PathBuf {
        PathBuf::from(self.as_string())
    }

    /// `path / other`.
    pub fn join(&self, other: &str) -> Self {
        let other = Self::parse(other);
        if !other.anchor.is_empty() {
            return other;
        }
        let mut out = self.clone();
        out.parts.extend(other.parts);
        out
    }

    /// `path.name`.
    pub fn name(&self) -> &str {
        self.parts.last().map(String::as_str).unwrap_or("")
    }

    /// `path.expanduser()`. `Err` er Pythons `RuntimeError` (ukjent `~bruker`).
    pub fn expanduser(self) -> Result<Self, String> {
        let first = match self.parts.first() {
            Some(first) if self.anchor.is_empty() && first.starts_with('~') => first,
            _ => return Ok(self),
        };
        let home = home_for(&first[1..])
            .ok_or_else(|| "Could not determine home directory.".to_string())?;
        let mut out = Self::parse(&home);
        out.parts.extend(self.parts.into_iter().skip(1));
        Ok(out)
    }
}

/// `os.path.expanduser("~" + user)` uten `~`-delen: `HOME` (eller hjemmemappa) for `""`,
/// ellers brukerens hjemmemappe fra `/etc/passwd`. `None` = ukjent bruker.
#[cfg(not(windows))]
fn home_for(user: &str) -> Option<String> {
    let home = if user.is_empty() {
        std::env::var("HOME").ok().unwrap_or_else(|| {
            fotomaxxer_core::paths::to_string(&fotomaxxer_core::paths::home_dir())
        })
    } else {
        let passwd = fs::read_to_string("/etc/passwd").ok()?;
        passwd.lines().find_map(|line| {
            let fields: Vec<&str> = line.split(':').collect();
            (fields.len() >= 7 && fields[0] == user).then(|| fields[5].to_string())
        })?
    };
    let trimmed = home.trim_end_matches('/');
    Some(if trimmed.is_empty() {
        "/".into()
    } else {
        trimmed.into()
    })
}

/// `ntpath.expanduser`: `~bruker` er en søskenmappe av egen hjemmemappe.
#[cfg(windows)]
fn home_for(user: &str) -> Option<String> {
    let home = fotomaxxer_core::paths::home_dir();
    let home = if user.is_empty() || home.file_name().is_some_and(|n| n == user) {
        home
    } else {
        home.parent().map(|p| p.join(user)).unwrap_or(home)
    };
    Some(fotomaxxer_core::paths::to_string(&home))
}

/// `repr(Path)`: `PosixPath('/a/b')` (`WindowsPath('…')` på Windows).
fn path_repr(text: &str) -> String {
    let class = if cfg!(windows) {
        "WindowsPath"
    } else {
        "PosixPath"
    };
    format!("{class}({})", py_repr(text))
}

// --- Feil som Python-unntak ------------------------------------------------------------

/// Pythons `OSError`-underklasse for feiltypen (Python velger den ut fra `errno`).
fn py_exception(kind: io::ErrorKind) -> &'static str {
    use io::ErrorKind as K;
    match kind {
        K::NotFound => "FileNotFoundError",
        K::PermissionDenied => "PermissionError",
        K::AlreadyExists => "FileExistsError",
        K::IsADirectory => "IsADirectoryError",
        K::NotADirectory => "NotADirectoryError",
        K::Interrupted => "InterruptedError",
        K::TimedOut => "TimeoutError",
        K::BrokenPipe => "BrokenPipeError",
        K::WouldBlock => "BlockingIOError",
        _ => "OSError",
    }
}

/// En feil som allerede har Pythons tekst (fra `py_os_error`).
fn os_failure(e: io::Error) -> JobFailure {
    JobFailure::new(py_exception(e.kind()), e.to_string())
}

/// `OSError` med én sti (`[Errno 13] Permission denied: '/sti'`).
fn os_error_at(e: io::Error, path: &str) -> JobFailure {
    os_failure(py_os_error(e, Path::new(path)))
}

/// `OSError` med to stier, som kopieringen i `shutil` setter (`… : 'src' -> 'dst'`).
fn os_error_copy(e: io::Error, src: &str, dst: &str) -> JobFailure {
    let text = match e.raw_os_error() {
        Some(code) => {
            let full = e.to_string();
            // Windows-meldinger slutter med «.» (og CRLF); Python fjerner dem, som
            // `py_os_error`.
            let strerror = full
                .strip_suffix(&format!(" (os error {code})"))
                .unwrap_or(&full)
                .trim_end_matches(['.', '\r', '\n'])
                .to_string();
            let tag = if cfg!(windows) { "WinError" } else { "Errno" };
            format!(
                "[{tag} {code}] {strerror}: {} -> {}",
                py_repr(src),
                py_repr(dst)
            )
        }
        None => e.to_string(),
    };
    JobFailure::new(py_exception(e.kind()), text)
}

fn db_failure(e: rusqlite::Error) -> JobFailure {
    JobFailure::from(fotomaxxer_core::Error::Db(e))
}

// --- shutil.copy2 -------------------------------------------------------------------------

/// `os.path.samefile`: samme enhet og inode (feil → `False`).
#[cfg(unix)]
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (fs::metadata(a), fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

#[cfg(unix)]
fn is_fifo(path: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    fs::metadata(path).is_ok_and(|m| m.file_type().is_fifo())
}

#[cfg(not(unix))]
fn is_fifo(_: &Path) -> bool {
    false
}

/// `shutil.copy2(src, dst)` med Pythons rekkefølge av sjekker og feiltekster.
pub fn copy2(src: &PyPath, dst: &PyPath) -> Result<(), JobFailure> {
    let src_s = src.as_string();
    let mut dst_s = dst.as_string();
    let mut dst_repr = path_repr(&dst_s);
    // `if os.path.isdir(dst): dst = os.path.join(dst, os.path.basename(src))` — nå en str.
    if Path::new(&dst_s).is_dir() {
        let base = src_s.rsplit(SEP).next().unwrap_or("");
        dst_s = if dst_s.ends_with(SEP) {
            format!("{dst_s}{base}")
        } else {
            format!("{dst_s}{SEP}{base}")
        };
        dst_repr = py_repr(&dst_s);
    }
    let (src_p, dst_p) = (Path::new(&src_s), Path::new(&dst_s));
    if same_file(src_p, dst_p) {
        return Err(JobFailure::new(
            "SameFileError",
            format!("{} and {dst_repr} are the same file", path_repr(&src_s)),
        ));
    }
    for p in [&src_s, &dst_s] {
        if is_fifo(Path::new(p)) {
            return Err(JobFailure::new(
                "SpecialFileError",
                format!("`{p}` is a named pipe"),
            ));
        }
    }
    // `open(src, "rb")`: en mappe gir `IsADirectoryError` i Python (Rust åpner den).
    #[cfg(unix)]
    if src_p.is_dir() {
        return Err(os_error_at(io::Error::from_raw_os_error(21), &src_s));
    }
    let mut reader = fs::File::open(src_p).map_err(|e| os_error_at(e, &src_s))?;
    let mut writer = fs::File::create(dst_p).map_err(|e| os_error_at(e, &dst_s))?;
    io::copy(&mut reader, &mut writer).map_err(|e| os_error_copy(e, &src_s, &dst_s))?;
    // `copystat`: ny `stat(src)`, så tidsstempler (ns) og til slutt rettigheter.
    let meta = fs::metadata(src_p).map_err(|e| os_error_at(e, &src_s))?;
    let mut times = fs::FileTimes::new();
    if let Ok(t) = meta.accessed() {
        times = times.set_accessed(t);
    }
    if let Ok(t) = meta.modified() {
        times = times.set_modified(t);
    }
    writer
        .set_times(times)
        .map_err(|e| os_error_at(e, &dst_s))?;
    drop(writer);
    fs::set_permissions(dst_p, meta.permissions()).map_err(|e| os_error_at(e, &dst_s))?;
    Ok(())
}

// --- Endepunktet ----------------------------------------------------------------------------

/// Alt endepunktet har sjekket før jobben startes.
#[derive(Debug)]
struct Plan {
    ordered_ids: Vec<i64>,
    subdir: Option<String>,
    dest_base: PyPath,
    keep_names: bool,
}

fn bad_request(message: &str) -> ApiError {
    ApiError::http(400, "bad_request", message)
}

/// `_resolve_targets`: (ordnede image_ids, undermappenavn | `None`).
fn resolve_targets(
    conn: &Connection,
    req: &ExportRequest,
) -> ApiResult<(Vec<i64>, Option<String>)> {
    let Some(album_id) = req.album_id else {
        return Ok((req.image_ids.clone().unwrap_or_default(), None));
    };
    let name: Option<String> = conn
        .query_row("SELECT name FROM albums WHERE id = ?", [album_id], |r| {
            r.get(0)
        })
        .optional()?;
    let Some(name) = name else {
        return Err(ApiError::http(404, "not_found", "album finnes ikke"));
    };
    let mut stmt = conn.prepare(
        "SELECT image_id FROM album_items WHERE album_id = ? ORDER BY position, image_id",
    )?;
    let ids = stmt
        .query_map([album_id], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<i64>>>()?;
    Ok((ids, Some(safe_dirname(&name))))
}

fn plan(conn: &Connection, req: ExportRequest) -> ApiResult<Plan> {
    if req.album_id.is_none() && req.image_ids.as_ref().is_none_or(Vec::is_empty) {
        return Err(bad_request("oppgi album_id eller image_ids"));
    }
    let (ordered_ids, subdir) = resolve_targets(conn, &req)?;
    if ordered_ids.is_empty() {
        return Err(bad_request("ingen bilder å eksportere"));
    }
    // `req.dest_path or get_setting(…)`: tom streng teller som ikke satt.
    let dest_root = match req.dest_path.filter(|d| !d.is_empty()) {
        Some(d) => Some(d),
        None => settings_mod::get_setting(conn, "default_export_dir")?,
    };
    let Some(dest_root) = dest_root.filter(|d| !d.is_empty()) else {
        return Err(ApiError::http(
            400,
            "no_dest",
            "ingen mål-mappe — sett en eller velg standard eksport-mappe i Innstillinger",
        ));
    };
    // `Path(dest_root).expanduser()` — en `RuntimeError` her er en 500 i Python.
    let dest_base = PyPath::parse(&dest_root)
        .expanduser()
        .map_err(|m| ApiError::internal(format!("RuntimeError: {m}")))?;
    Ok(Plan {
        ordered_ids,
        subdir,
        dest_base,
        keep_names: req.keep_original_names,
    })
}

async fn export(
    State(state): State<AppState>,
    req: DbReq<(), (), ExportRequest>,
) -> ApiResult<Accepted> {
    let DbReq { conn, body, .. } = req;
    let plan = blocking(move || plan(&conn, body)).await?;
    accept(&state, "export", move |run| run_export(run, plan))
}

fn progress(copied: usize, skipped: usize, total: usize) -> Map<String, Value> {
    let mut p = Map::new();
    p.insert("copied".into(), Value::from(copied));
    p.insert("skipped".into(), Value::from(skipped));
    p.insert("total".into(), Value::from(total));
    p
}

/// Selve jobben (egen tilkobling, som `job` i `export.py`).
fn run_export(run: &JobHandle, plan: Plan) -> JobResult {
    run.set_step("kopierer");
    let jconn = job_conn()?;
    let target_dir = match &plan.subdir {
        Some(subdir) => plan.dest_base.join(subdir),
        None => plan.dest_base.clone(),
    };
    let target_s = target_dir.as_string();
    if target_s.contains('\0') {
        // `os.mkdir` avviser stien før systemkallet (teksten er fra Python 3.14).
        return Err(JobFailure::new(
            "ValueError",
            "mkdir: embedded null character in path",
        ));
    }
    mkdir_parents(&target_dir.fs()).map_err(os_failure)?;
    let total = plan.ordered_ids.len();
    let (mut copied, mut skipped) = (0, 0);
    run.set_progress(progress(copied, skipped, total));
    for (i, image_id) in plan.ordered_ids.iter().enumerate() {
        let path: Option<String> = jconn
            .query_row(
                "SELECT path FROM files WHERE image_id = ? ORDER BY id LIMIT 1",
                [image_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(db_failure)?;
        let src = match path.map(|p| PyPath::parse(&p)) {
            Some(src) if src.fs().exists() => src,
            _ => {
                skipped += 1;
                run.set_progress(progress(copied, skipped, total));
                continue;
            }
        };
        let name = if plan.keep_names {
            src.name().to_string()
        } else {
            format!("{:03}_{}", i + 1, src.name())
        };
        copy2(&src, &target_dir.join(&name))?;
        copied += 1;
        run.set_progress(progress(copied, skipped, total));
    }
    let mut message = format!("Eksporterte {copied} bilder til {target_s}");
    if skipped > 0 {
        message.push_str(&format!(", hoppet over {skipped}"));
    }
    run.set_message(message);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fasit: `_safe_dirname` i `api/routers/export.py` (Python 3.14).
    #[test]
    fn safe_dirname_som_python() {
        let cases = [
            ("Sommer på hytta", "Sommer på hytta"),
            ("Ærlig talt: «Ørret» & Åse!", "Ærlig talt_ _Ørret_ _ Åse_"),
            ("a/b\\c", "a_b_c"),
            ("..", "album"),
            ("", "album"),
            ("   ", "album"),
            (" . a . ", " a "),
            ("abc .", "abc "),
            ("...skjult", "skjult"),
            ("tab\there", "tab_here"),
            ("nbsp\u{a0}x", "nbsp_x"),
            ("e\u{301}", "e_"),
            ("中文 Ωμέγα ß ²", "中文 Ωμέγα ß ²"),
            ("🙂🙂 smil 🙂", "_ smil _"),
            ("2011-07-01_tur.v2", "2011-07-01_tur.v2"),
            ("a\0b", "a_b"),
        ];
        for (name, want) in cases {
            assert_eq!(safe_dirname(name), want, "{name:?}");
        }
    }

    /// Fasit: `str(Path(p))` i Python 3.14.
    #[cfg(not(windows))]
    #[test]
    fn pathlib_tekst() {
        let cases = [
            ("/tmp/x/", "/tmp/x"),
            ("/tmp//x/./y", "/tmp/x/y"),
            ("//tmp/x", "//tmp/x"),
            ("///tmp/x", "/tmp/x"),
            ("./a/../b", "a/../b"),
            (".", "."),
            ("a", "a"),
            ("/", "/"),
            (" ", " "),
        ];
        for (raw, want) in cases {
            assert_eq!(PyPath::parse(raw).as_string(), want, "{raw:?}");
        }
        let p = PyPath::parse("/a/b.jpg");
        assert_eq!(p.name(), "b.jpg");
        assert_eq!(PyPath::parse("/").name(), "");
        assert_eq!(PyPath::parse(".").join("x").as_string(), "x");
        assert_eq!(PyPath::parse("/a").join("/b").as_string(), "/b");
        assert_eq!(PyPath::parse("/").join("Album").as_string(), "/Album");
    }

    #[cfg(unix)]
    #[test]
    fn expanduser_som_pathlib() {
        let home = std::env::var("HOME").unwrap();
        let home = home.trim_end_matches('/');
        assert_eq!(
            PyPath::parse("~/ut").expanduser().unwrap().as_string(),
            format!("{home}/ut")
        );
        assert_eq!(
            PyPath::parse("~").expanduser().unwrap().as_string(),
            if home.is_empty() { "/" } else { home }
        );
        assert_eq!(
            PyPath::parse("/a/~").expanduser().unwrap().as_string(),
            "/a/~"
        );
        assert_eq!(
            PyPath::parse("~root/x").expanduser().unwrap().as_string(),
            "/root/x"
        );
        assert!(PyPath::parse("~finnes-ikke-xyz/x").expanduser().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn copy2_som_shutil() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let src = root.join("bilde æ.jpg");
        fs::write(&src, b"innhold").unwrap();
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_300_000_000);
        fs::File::options()
            .write(true)
            .open(&src)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(old))
            .unwrap();
        let src_p = PyPath::parse(&src.to_string_lossy());
        let out = PyPath::parse(&root.to_string_lossy()).join("ut");
        mkdir_parents(&out.fs()).unwrap();

        copy2(&src_p, &out.join("001_bilde æ.jpg")).unwrap();
        let copy = out.fs().join("001_bilde æ.jpg");
        assert_eq!(fs::read(&copy).unwrap(), b"innhold");
        assert_eq!(fs::metadata(&copy).unwrap().modified().unwrap(), old);

        // Samme fil: originalen røres ikke.
        let err = copy2(&src_p, &src_p).unwrap_err();
        assert_eq!(err.kind, "SameFileError");
        let shown = format!("PosixPath('{}')", src.display());
        assert_eq!(
            err.message,
            format!("{shown} and {shown} are the same file")
        );
        assert_eq!(fs::read(&src).unwrap(), b"innhold");

        // Mål som er en mappe: kopien havner inni den.
        fs::create_dir(out.fs().join("002_x.jpg")).unwrap();
        copy2(&src_p, &out.join("002_x.jpg")).unwrap();
        assert!(out.fs().join("002_x.jpg/bilde æ.jpg").is_file());

        // Kilde som er en mappe.
        let err = copy2(&out, &out.join("003")).unwrap_err();
        assert_eq!(err.kind, "IsADirectoryError");
        assert_eq!(
            err.message,
            format!("[Errno 21] Is a directory: '{}'", out.as_string())
        );
    }

    // Rå feilkoder er errno på Unix (17 = EEXIST, 28 = ENOSPC), men Windows-koder på
    // Windows (17 = ERROR_NOT_SAME_DEVICE), så testen finnes i to varianter.
    #[cfg(windows)]
    #[test]
    fn os_feil_far_pythons_underklasse() {
        // ERROR_ALREADY_EXISTS → FileExistsError, som Pythons winerror → EEXIST.
        let e = py_os_error(io::Error::from_raw_os_error(183), Path::new(r"C:\tmp\fil"));
        let f = os_failure(e);
        assert_eq!(f.kind, "FileExistsError");
        assert!(f.message.starts_with("[WinError 183] "), "{}", f.message);
        assert!(f.message.ends_with(r": 'C:\\tmp\\fil'"), "{}", f.message);
        assert!(!f.message.contains(".:"), "{}", f.message);
        // ERROR_DISK_FULL → OSError, og Windows-meldingens punktum fjernes som i Python.
        let f = os_error_copy(io::Error::from_raw_os_error(112), r"C:\a", r"C:\b");
        assert_eq!(f.kind, "OSError");
        assert!(f.message.starts_with("[WinError 112] "), "{}", f.message);
        assert!(
            f.message.ends_with(r": 'C:\\a' -> 'C:\\b'"),
            "{}",
            f.message
        );
        assert!(!f.message.contains(".:"), "{}", f.message);
    }

    #[cfg(unix)]
    #[test]
    fn os_feil_far_pythons_underklasse() {
        let e = py_os_error(io::Error::from_raw_os_error(17), Path::new("/tmp/fil"));
        let f = os_failure(e);
        assert_eq!(f.kind, "FileExistsError");
        assert_eq!(f.message, "[Errno 17] File exists: '/tmp/fil'");
        let f = os_error_copy(io::Error::from_raw_os_error(28), "/a", "/b");
        assert_eq!(f.kind, "OSError");
        assert_eq!(
            f.message,
            "[Errno 28] No space left on device: '/a' -> '/b'"
        );
    }
}
