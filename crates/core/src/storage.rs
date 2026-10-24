//! Pakke-relativ sti-resolver.
//!
//! Hele pakka — `fotomaxxer.db` og `derived/` — er én flyttbar mappe. Appens egne
//! stier resolves alltid relativt til pakke-rota, aldri hardkodet absolutt.
//! (`files.path` er det eneste som er absolutt — det peker til eksterne originaler.)
//!
//! Pakke-rota bestemmes i denne rekkefølgen: eksplisitt arg > `FOTOMAXXER_HOME` >
//! **bootstrap-pekerfil** (`~/.config/fotomaxxer/config.json` → `{"data_dir": …}`) >
//! gjeldende arbeidsmappe. Pekerfila bor utenfor pakka fordi stien ikke kan ligge i
//! `settings`-tabellen (vi trenger stien for å åpne databasen tabellen ligger i).

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::error::Result;
use crate::paths;

pub const ENV_HOME: &str = "FOTOMAXXER_HOME";
pub const DB_FILENAME: &str = "fotomaxxer.db";
pub const DERIVED_DIRNAME: &str = "derived";

const CONFIG_DIRNAME: &str = "fotomaxxer";
const CONFIG_FILENAME: &str = "config.json";
const POINTER_KEY: &str = "data_dir";

/// Sti til bootstrap-pekerfila (respekterer `XDG_CONFIG_HOME`, også på Windows).
pub fn config_path() -> PathBuf {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(v) if !v.is_empty() => paths::expand_user(Path::new(&v)),
        _ => paths::home_dir().join(".config"),
    };
    base.join(CONFIG_DIRNAME).join(CONFIG_FILENAME)
}

fn read_config() -> Option<Map<String, Value>> {
    let text = fs::read_to_string(config_path()).ok()?;
    match serde_json::from_str(&text).ok()? {
        Value::Object(map) => Some(map),
        _ => None,
    }
}

/// `data_dir` fra pekerfila, eller `None` om den mangler/er ugyldig.
pub fn read_pointer() -> Option<String> {
    match read_config()?.get(POINTER_KEY)? {
        Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        _ => None,
    }
}

/// Skriv `data_dir` til pekerfila (atomisk, bevarer andre nøkler).
pub fn write_pointer(data_dir: &Path) -> Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut data = read_config().unwrap_or_default();
    data.insert(
        POINTER_KEY.into(),
        Value::String(paths::to_string(&paths::resolve(data_dir))),
    );
    let tmp = path.with_file_name(format!("{CONFIG_FILENAME}.tmp"));
    fs::write(&tmp, serde_json::to_string_pretty(&data)?)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// Finn pakke-rota: eksplisitt arg > `FOTOMAXXER_HOME` > pekerfil > cwd.
pub fn resolve_root(explicit: Option<&Path>) -> PathBuf {
    if let Some(p) = explicit {
        return paths::resolve(p);
    }
    if let Some(env) = std::env::var_os(ENV_HOME).filter(|v| !v.is_empty()) {
        return paths::resolve(Path::new(&env));
    }
    if let Some(pointer) = read_pointer() {
        return paths::resolve(Path::new(&pointer));
    }
    paths::resolve(Path::new("."))
}

/// Flytt datamappa (`fotomaxxer.db` + `derived/`) til `new_dir`.
///
/// Kopi-så-bytt: kopiér alt først, bytt pekerfila kun når kopien er fullført.
/// Feiler kopien, rulles delvis kopi tilbake og pekerfil + gammel mappe står urørt.
/// Den gamle mappa beholdes som standard; `delete_source` fjerner den (best effort).
///
/// Filene kopieres som `shutil.copy2` (innhold, rettigheter og atime/mtime), så
/// `derived/`-filene beholder tidsstemplene (og dermed ETag-ene i `/derived`).
/// Feil kommer som `Error::Io` med Pythons `OSError`-tekst
/// (`[Errno 20] Not a directory: '/sti'`), som API-et viser uendret.
pub fn relocate_data(
    new_dir: &Path,
    current_root: Option<&Path>,
    delete_source: bool,
) -> Result<PathBuf> {
    let src = match current_root {
        Some(p) => paths::resolve(p),
        None => resolve_root(None),
    };
    let dst = paths::resolve(new_dir);
    if dst == src {
        return Ok(dst);
    }
    let (src_store, dst_store) = (Storage::new(&src), Storage::new(&dst));
    mkdir_parents(&dst)?;

    let mut copied: Vec<PathBuf> = Vec::new();
    let result = (|| -> std::io::Result<()> {
        if src_store.db_path().exists() {
            copied.push(dst_store.db_path());
            copy2(&src_store.db_path(), &dst_store.db_path())?;
        }
        if src_store.derived_dir().exists() {
            copied.push(dst_store.derived_dir());
            copy_tree(&src_store.derived_dir(), &dst_store.derived_dir())?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        // Rull tilbake delvis kopi; pekerfil + gammel mappe urørt.
        for path in copied {
            let _ = if path.is_dir() {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_file(&path)
            };
        }
        return Err(e.into());
    }

    // Kopien er trygg → bytt peker. Gammel mappe beholdes med mindre eksplisitt slettet.
    write_pointer(&dst)?;
    if delete_source {
        let _ = fs::remove_file(src_store.db_path());
        let _ = fs::remove_dir_all(src_store.derived_dir());
    }
    Ok(dst)
}

/// Pythons `repr(str)`: enkle fnutter, med mindre teksten har `'` men ikke `"`.
pub fn py_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `str(OSError)` slik Python viser den: `[Errno 13] Permission denied: '/sti'`
/// (`[WinError 5] …` på Windows).
pub fn py_os_error(e: std::io::Error, path: &Path) -> std::io::Error {
    let shown = py_repr(&paths::to_string(path));
    let text = match e.raw_os_error() {
        Some(code) => {
            let full = e.to_string();
            let strerror = full
                .strip_suffix(&format!(" (os error {code})"))
                .unwrap_or(&full)
                .trim_end_matches(['.', '\r', '\n'])
                .to_string();
            let tag = if cfg!(windows) { "WinError" } else { "Errno" };
            format!("[{tag} {code}] {strerror}: {shown}")
        }
        None => format!("{e}: {shown}"),
    };
    std::io::Error::new(e.kind(), text)
}

/// `Path.mkdir(parents=True, exist_ok=True)`, med stien som faktisk feilet i meldingen.
pub fn mkdir_parents(path: &Path) -> std::io::Result<()> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            match path.parent() {
                Some(parent) if parent != path => mkdir_parents(parent)?,
                _ => return Err(py_os_error(e, path)),
            }
            match fs::create_dir(path) {
                Err(_) if path.is_dir() => Ok(()),
                other => other.map_err(|e| py_os_error(e, path)),
            }
        }
        Err(_) if path.is_dir() => Ok(()),
        Err(e) => Err(py_os_error(e, path)),
    }
}

/// `shutil.copy2`: innhold, rettigheter og atime/mtime (nanosekunder).
fn copy2(src: &Path, dst: &Path) -> std::io::Result<()> {
    let meta = fs::metadata(src).map_err(|e| py_os_error(e, src))?;
    let mut reader = fs::File::open(src).map_err(|e| py_os_error(e, src))?;
    let mut writer = fs::File::create(dst).map_err(|e| py_os_error(e, dst))?;
    std::io::copy(&mut reader, &mut writer)?;
    let mut times = fs::FileTimes::new();
    if let Ok(t) = meta.accessed() {
        times = times.set_accessed(t);
    }
    if let Ok(t) = meta.modified() {
        times = times.set_modified(t);
    }
    writer.set_times(times)?;
    drop(writer);
    fs::set_permissions(dst, meta.permissions())?;
    Ok(())
}

/// `shutil.copytree(src, dst, dirs_exist_ok=True)` med `copy2`. Som i Python samles
/// feil per fil, kopieringen fortsetter, og til slutt kommer én feil med alle
/// (`[('src', 'dst', "[Errno 13] …")]`).
fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    let mut errors = Vec::new();
    copy_tree_into(src, dst, &mut errors)?;
    if errors.is_empty() {
        return Ok(());
    }
    let items: Vec<String> = errors
        .iter()
        .map(|(s, d, why)| {
            format!(
                "({}, {}, {})",
                py_repr(&paths::to_string(s)),
                py_repr(&paths::to_string(d)),
                py_repr(why)
            )
        })
        .collect();
    Err(std::io::Error::other(format!("[{}]", items.join(", "))))
}

fn copy_tree_into(
    src: &Path,
    dst: &Path,
    errors: &mut Vec<(PathBuf, PathBuf, String)>,
) -> std::io::Result<()> {
    let entries = fs::read_dir(src).map_err(|e| py_os_error(e, src))?;
    mkdir_parents(dst)?;
    for entry in entries {
        let entry = entry?;
        let (from, to) = (entry.path(), dst.join(entry.file_name()));
        // Symlenker følges (`symlinks=False`), som i Python.
        let result = if from.is_dir() {
            copy_tree_into(&from, &to, errors)
        } else {
            copy2(&from, &to)
        };
        if let Err(e) = result {
            errors.push((from, to, e.to_string()));
        }
    }
    Ok(())
}

/// Resolver for appens egne stier, rotet i pakke-mappa.
#[derive(Debug, Clone)]
pub struct Storage {
    root: PathBuf,
}

impl Storage {
    pub fn new(root: &Path) -> Self {
        Self {
            root: paths::resolve(root),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn db_path(&self) -> PathBuf {
        self.root.join(DB_FILENAME)
    }

    pub fn derived_dir(&self) -> PathBuf {
        self.root.join(DERIVED_DIRNAME)
    }

    /// `derived/<hash>_<size>.jpg`. Lagre kun filnavnet i DB-en, aldri en absolutt sti.
    pub fn derived_path(&self, content_hash: &str, size: &str) -> PathBuf {
        self.derived_dir()
            .join(derived_filename(content_hash, size))
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        fs::create_dir_all(self.derived_dir())?;
        Ok(())
    }
}

/// Filnavnet til en avledet kopi, slik det lagres i `derived.path`.
pub fn derived_filename(content_hash: &str, size: &str) -> String {
    format!("{content_hash}_{size}.jpg")
}

#[cfg(test)]
mod tests {
    //! Speiler `tests/verify_storage_relocate.py`. Miljøvariabler er prosess-globale,
    //! så alt som rører dem kjøres i én test.
    use super::*;

    fn make_root(path: &Path, marker: &str) -> PathBuf {
        fs::create_dir_all(path.join(DERIVED_DIRNAME)).unwrap();
        fs::write(path.join(DB_FILENAME), marker).unwrap();
        fs::write(path.join(DERIVED_DIRNAME).join("h_300.jpg"), "thumb").unwrap();
        paths::resolve(path)
    }

    #[test]
    fn resolve_root_og_flytting() {
        let tmp = tempfile::tempdir().unwrap();
        let tmp = paths::resolve(tmp.path());
        // SAFETY: eneste test i crate-en som rører miljøet; ingen andre tråder leser disse variablene.
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", tmp.join("cfg"));
            std::env::remove_var(ENV_HOME);
        }
        assert!(config_path().starts_with(&tmp), "pekerfila er isolert");

        // Prioritet: eksplisitt > env > peker > cwd.
        let a = make_root(&tmp.join("data-a"), "DB-A");
        let b = make_root(&tmp.join("data-b"), "DB-B");
        let c = make_root(&tmp.join("data-c"), "DB-C");
        assert_eq!(resolve_root(Some(&a)), a);
        write_pointer(&c).unwrap();
        assert_eq!(
            read_pointer().as_deref(),
            Some(paths::to_string(&c).as_str())
        );
        assert_eq!(resolve_root(None), c, "pekerfil");
        unsafe { std::env::set_var(ENV_HOME, &b) };
        assert_eq!(resolve_root(None), b, "env slår pekerfil");
        assert_eq!(resolve_root(Some(&a)), a, "eksplisitt slår env");
        unsafe { std::env::remove_var(ENV_HOME) };

        // Flytting beholder gammel mappe som standard, og filene beholder mtime (copy2).
        let mtime_before = fs::metadata(a.join(DERIVED_DIRNAME).join("h_300.jpg"))
            .unwrap()
            .modified()
            .unwrap();
        let moved = relocate_data(&tmp.join("ny"), Some(&a), false).unwrap();
        assert_eq!(
            fs::metadata(moved.join(DERIVED_DIRNAME).join("h_300.jpg"))
                .unwrap()
                .modified()
                .unwrap(),
            mtime_before
        );
        assert_eq!(fs::read_to_string(moved.join(DB_FILENAME)).unwrap(), "DB-A");
        assert!(moved.join(DERIVED_DIRNAME).join("h_300.jpg").exists());
        assert!(a.join(DB_FILENAME).exists(), "gammel db beholdt");
        assert_eq!(resolve_root(None), moved, "pekeren følger");

        // delete_source fjerner gammel db + derived.
        let moved2 = relocate_data(&tmp.join("ny2"), Some(&b), true).unwrap();
        assert!(moved2.join(DB_FILENAME).exists());
        assert!(!b.join(DB_FILENAME).exists() && !b.join(DERIVED_DIRNAME).exists());

        // dst == src er en no-op.
        assert_eq!(relocate_data(&c, Some(&c), true).unwrap(), c);
        assert!(c.join(DB_FILENAME).exists());

        // Mislykket kopi (mappa kan ikke lages under en fil) rører verken peker eller kilde,
        // og feilteksten er Pythons OSError-tekst.
        let blocker = tmp.join("blokkert");
        fs::write(&blocker, "ikke en mappe").unwrap();
        let before = read_pointer();
        let err = relocate_data(&blocker.join("barn"), Some(&c), true).unwrap_err();
        #[cfg(unix)]
        assert_eq!(
            err.to_string(),
            format!(
                "[Errno 20] Not a directory: '{}'",
                blocker.join("barn").display()
            )
        );
        let _ = err;
        assert_eq!(read_pointer(), before, "pekeren er urørt");
        assert!(c.join(DB_FILENAME).exists(), "kilden er urørt");

        // Andre nøkler i pekerfila bevares.
        let mut cfg = read_config().unwrap();
        cfg.insert("annet".into(), Value::Bool(true));
        fs::write(config_path(), serde_json::to_string(&cfg).unwrap()).unwrap();
        write_pointer(&a).unwrap();
        assert_eq!(
            read_config().unwrap().get("annet"),
            Some(&Value::Bool(true))
        );
    }

    #[test]
    fn python_repr_og_oserror() {
        assert_eq!(py_repr("/a/b"), "'/a/b'");
        assert_eq!(py_repr("Bjørn's"), "\"Bjørn's\"");
        assert_eq!(py_repr("a'\"b"), "'a\\'\"b'");
        assert_eq!(py_repr("C:\\x"), "'C:\\\\x'");
        #[cfg(unix)]
        assert_eq!(
            py_os_error(std::io::Error::from_raw_os_error(13), Path::new("/x")).to_string(),
            "[Errno 13] Permission denied: '/x'"
        );
    }

    #[test]
    fn derived_path_er_hash_og_storrelse() {
        let s = Storage::new(Path::new("/x"));
        assert!(
            s.derived_path("abc", "300")
                .ends_with("derived/abc_300.jpg")
        );
    }
}
