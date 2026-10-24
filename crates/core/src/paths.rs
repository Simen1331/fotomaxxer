//! Stinormalisering som gir samme strenger som Pythons `Path(p).expanduser().resolve()`.
//!
//! Absolutte stier lagres i databasen (`sources.root_path`, `files.path`) og
//! sammenlignes som tekst, så Rust må produsere nøyaktig samme form som Python:
//! symlenker løst, ingen `\\?\`-prefiks på Windows (derfor `dunce`), og ikke-
//! eksisterende hale beholdt (Pythons `resolve(strict=False)`).
//!
//! På Unix er ikke-eksisterende stier en port av `posixpath.realpath(strict=False)`
//! (Python 3.14): også symlenker med manglende mål følges (`brutt-lenke` →
//! målet), og `..` etter en symlenke virker på målet.

use std::path::{Component, Path, PathBuf};

/// Hjemmemappa (`Path.home()`): `HOME` på Unix, `USERPROFILE` på Windows.
pub fn home_dir() -> PathBuf {
    std::env::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// `~` / `~/…` → hjemmemappa (`Path.expanduser()`).
pub fn expand_user(p: &Path) -> PathBuf {
    let mut comps = p.components();
    match comps.next() {
        Some(Component::Normal(first)) if first == "~" => home_dir().join(comps.as_path()),
        _ => p.to_path_buf(),
    }
}

/// `Path(p).expanduser().resolve()` — absolutt, symlenker løst, også om stien ikke finnes.
pub fn resolve(p: &Path) -> PathBuf {
    let p = expand_user(p);
    let abs = if p.is_absolute() {
        p
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };
    resolve_existing_prefix(&abs)
}

#[cfg(unix)]
fn resolve_existing_prefix(abs: &Path) -> PathBuf {
    dunce::canonicalize(abs).unwrap_or_else(|_| realpath(abs))
}

/// `posixpath.realpath(path, strict=False)` for en absolutt sti.
#[cfg(unix)]
fn realpath(abs: &Path) -> PathBuf {
    use std::collections::HashMap;
    use std::ffi::{OsStr, OsString};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    enum Item {
        Part(Vec<u8>),
        /// Symlenken er ferdig løst; husk resultatet (Pythons `None`-markør).
        Resolved(Vec<u8>),
    }
    let parts = |bytes: &[u8]| -> Vec<Item> {
        bytes
            .split(|b| *b == b'/')
            .rev()
            .map(|p| Item::Part(p.to_vec()))
            .collect()
    };
    let mut rest = parts(abs.as_os_str().as_bytes());
    let mut path: Vec<u8> = b"/".to_vec();
    let mut seen: HashMap<Vec<u8>, Option<Vec<u8>>> = HashMap::new();
    while let Some(item) = rest.pop() {
        let name = match item {
            Item::Resolved(link) => {
                seen.insert(link, Some(path.clone()));
                continue;
            }
            Item::Part(name) => name,
        };
        if name.is_empty() || name == b"." {
            continue;
        }
        if name == b".." {
            let cut = path.iter().rposition(|b| *b == b'/').unwrap_or(0);
            path.truncate(cut.max(1));
            continue;
        }
        let mut newpath = path.clone();
        if path != b"/" {
            newpath.push(b'/');
        }
        newpath.extend_from_slice(&name);
        let os = OsStr::from_bytes(&newpath);
        let is_link = match std::fs::symlink_metadata(os) {
            Ok(m) => m.file_type().is_symlink(),
            Err(_) => false,
        };
        if !is_link {
            path = newpath;
            continue;
        }
        if let Some(cached) = seen.get(&newpath) {
            // Ferdig løst før (bruk svaret), eller en løkke (behold stien som den er).
            path = cached.clone().unwrap_or(newpath);
            continue;
        }
        let Ok(target) = std::fs::read_link(os) else {
            path = newpath;
            continue;
        };
        let target = target.into_os_string().into_vec();
        if target.starts_with(b"/") {
            path = b"/".to_vec();
        }
        seen.insert(newpath.clone(), None);
        rest.push(Item::Resolved(newpath));
        rest.extend(parts(&target));
    }
    PathBuf::from(OsString::from_vec(path))
}

#[cfg(not(unix))]
fn resolve_existing_prefix(abs: &Path) -> PathBuf {
    if let Ok(real) = dunce::canonicalize(abs) {
        return real;
    }
    // Finnes ikke: løs den lengste eksisterende forelderen og legg halen på leksikalsk.
    match (abs.parent(), abs.file_name()) {
        (Some(parent), Some(name)) => resolve_existing_prefix(parent).join(name),
        (Some(parent), None) => {
            // Siste komponent er `..` eller `.`
            let base = resolve_existing_prefix(parent);
            match abs.components().next_back() {
                Some(Component::ParentDir) => base.parent().map(Path::to_path_buf).unwrap_or(base),
                _ => base,
            }
        }
        (None, _) => abs.to_path_buf(),
    }
}

/// En sti som tekst, slik Python `str(path)` gir den.
pub fn to_string(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eksisterende_sti_kanoniseres() {
        let dir = tempfile::tempdir().unwrap();
        let real = dunce::canonicalize(dir.path()).unwrap();
        assert_eq!(resolve(&dir.path().join("a/../")), real);
        assert_eq!(resolve(&dir.path().join(".")), real);
    }

    #[test]
    fn ikke_eksisterende_hale_beholdes() {
        let dir = tempfile::tempdir().unwrap();
        let real = dunce::canonicalize(dir.path()).unwrap();
        assert_eq!(
            resolve(&dir.path().join("ny/mappe")),
            real.join("ny").join("mappe")
        );
        assert_eq!(resolve(&dir.path().join("ny/../annen")), real.join("annen"));
    }

    /// Fasit fra Pythons `Path(p).resolve()` på samme tre.
    #[cfg(unix)]
    #[test]
    fn symlenker_med_manglende_mal_som_python() {
        let dir = tempfile::tempdir().unwrap();
        let real = dunce::canonicalize(dir.path()).unwrap();
        std::fs::create_dir(real.join("a")).unwrap();
        std::os::unix::fs::symlink(real.join("finnes-ikke"), real.join("brutt")).unwrap();
        std::os::unix::fs::symlink("a", real.join("rel")).unwrap();
        std::os::unix::fs::symlink("løkke", real.join("løkke")).unwrap();
        assert_eq!(resolve(&real.join("brutt")), real.join("finnes-ikke"));
        assert_eq!(resolve(&real.join("brutt/x/..")), real.join("finnes-ikke"));
        assert_eq!(resolve(&real.join("rel/ny")), real.join("a/ny"));
        assert_eq!(resolve(&real.join("rel/../ny")), real.join("ny"));
        assert_eq!(resolve(&real.join("løkke/x")), real.join("løkke/x"));
        assert_eq!(
            resolve(Path::new("/finnes/ikke/..")),
            PathBuf::from("/finnes")
        );
    }

    #[test]
    fn tilde_ekspanderes() {
        assert_eq!(expand_user(Path::new("~/x")), home_dir().join("x"));
        assert_eq!(expand_user(Path::new("/a/~")), PathBuf::from("/a/~"));
    }
}
