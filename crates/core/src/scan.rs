//! Scan-steg: walk → whitelist → stat-fast-path → SHA-256 → dedup → images/files.
//!
//! Per `source`, aldri implisitt over alt. Idempotent: re-scan gir kun arbeid på
//! nye/endrede filer (stat-fast-path mot `scan_index` hopper over uendrede). Nye
//! hasher gir en ny `images`-rad med EXIF + geocode og `processing_state` pending
//! for derive/ai/faces. Kjente hasher dedupliseres. EXIF/geocode-feil stopper aldri
//! ingest — feltene blir bare NULL.
//!
//! Gangrekkefølge, stistrenger og `mtime`-verdier er de samme som Pythons
//! `os.walk`/`os.path.join`/`os.stat`, så en database skannet av Python-versjonen
//! gjenkjennes uten ny hashing.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::cancel;
use crate::error::{Error, Result};
use crate::exif;
use crate::geocode::Geocoder;
use crate::sources::Source;

pub const WHITELIST: &[&str] = &[".jpg", ".jpeg", ".png", ".heic", ".heif"];
const HASH_CHUNK: usize = 1 << 20; // 1 MiB
const COMMIT_EVERY: usize = 500; // flush scan_index jevnlig så re-scan er billig etter avbrudd
const PROGRESS_EVERY: u64 = 50; // rapportér fremdrift såpass ofte (live GUI-teller)

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ScanStats {
    /// Whitelist-filer behandlet.
    pub seen: u64,
    /// Filer utenfor whitelist.
    pub skipped_ext: u64,
    /// Faktisk lest + hashet.
    pub hashed: u64,
    /// Gjenbrukt hash via scan_index (ingen ny I/O).
    pub fast_path: u64,
    /// Nye images-rader.
    pub new_images: u64,
    /// Kjent hash (dedup).
    pub duplicates: u64,
    /// files-rader upsertet.
    pub files: u64,
    /// Filer hoppet over pga. feil.
    pub errors: u64,
    pub skipped_ext_types: BTreeMap<String, u64>,
}

impl ScanStats {
    pub fn summary(&self) -> String {
        format!(
            "sett={} hashet={} fast-path={} nye={} duplikater={} files={} ikke-bilde={} feil={}",
            self.seen,
            self.hashed,
            self.fast_path,
            self.new_images,
            self.duplicates,
            self.files,
            self.skipped_ext,
            self.errors
        )
    }
}

/// Pythons `os.path.splitext(name)[1].lower()`: ledende punktum teller ikke som endelse.
pub fn extension_lower(name: &str) -> String {
    let Some(dot) = name.rfind('.') else {
        return String::new();
    };
    if name[..dot].chars().all(|c| c == '.') {
        return String::new();
    }
    name[dot..].to_lowercase()
}

/// `os.stat(path).st_mtime` som nøyaktig samme flyttall (`sek + nsek * 1e-9`).
pub fn mtime_float(meta: &fs::Metadata) -> f64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        meta.mtime() as f64 + meta.mtime_nsec() as f64 * 1e-9
    }
    #[cfg(not(unix))]
    {
        match meta
            .modified()
            .map(|t| t.duration_since(std::time::UNIX_EPOCH))
        {
            Ok(Ok(d)) => d.as_secs() as f64 + d.subsec_nanos() as f64 * 1e-9,
            _ => 0.0,
        }
    }
}

/// `os.walk(root)` (top-down, følger ikke symlenkede mapper): kaller `visit` for hver
/// mappe med filnavnene i katalogrekkefølge. Ulesbare mapper hoppes stille over.
fn walk(dir: &Path, visit: &mut dyn FnMut(&Path, &[String]) -> Result<()>) -> Result<()> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };
    let (mut dirs, mut files) = (Vec::new(), Vec::new());
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // DirEntry.is_dir() i Python følger symlenker.
        let is_dir = fs::metadata(entry.path())
            .map(|m| m.is_dir())
            .unwrap_or(false);
        if is_dir {
            dirs.push(name);
        } else {
            files.push(name);
        }
    }
    visit(dir, &files)?;
    for name in dirs {
        let sub = dir.join(&name);
        let is_link = fs::symlink_metadata(&sub)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        if !is_link {
            walk(&sub, visit)?;
        }
    }
    Ok(())
}

/// Skann én kilde. `on_progress` kalles med løpende `stats` ca. hver 50. fil, så GUI-et
/// kan vise en levende teller (scan har ingen `processing_state` å lese fra under walken).
pub fn scan_source(
    conn: &Connection,
    source: &Source,
    geocoder: &mut Geocoder,
    mut on_progress: Option<&mut dyn FnMut(&ScanStats)>,
) -> Result<ScanStats> {
    let mut stats = ScanStats::default();
    let mut pending = 0usize;
    // Transaksjonen committes i batcher underveis (som Pythons `conn.commit()` hver
    // 500. fil), så den styres med rå BEGIN/COMMIT i stedet for et `Transaction`-objekt.
    conn.execute_batch("BEGIN")?;
    let result = walk(Path::new(&source.root_path), &mut |dirpath, filenames| {
        for name in filenames {
            if cancel::requested() {
                // Trygt punkt: hver fil før denne er skrevet ferdig. `last_scanned_at`
                // settes ikke, og neste skanning tar resten (hurtigsjekken hopper over
                // det som er gjort).
                conn.execute_batch("COMMIT")?;
                return Err(Error::Cancelled);
            }
            let ext = extension_lower(name);
            if !WHITELIST.contains(&ext.as_str()) {
                stats.skipped_ext += 1;
                *stats.skipped_ext_types.entry(ext).or_default() += 1;
                continue;
            }
            let path = dirpath.join(name);
            let path_str = path.to_string_lossy().into_owned();
            let Ok(meta) = fs::metadata(&path) else {
                stats.errors += 1;
                continue;
            };
            stats.seen += 1;
            let (size, mtime) = (meta.len() as i64, mtime_float(&meta));

            let content_hash = match fast_path_hash(conn, &path_str, size, mtime)? {
                Some(h) => {
                    stats.fast_path += 1;
                    h
                }
                None => {
                    let Ok(h) = hash_file(&path) else {
                        stats.errors += 1;
                        continue;
                    };
                    upsert_scan_index(conn, &path_str, size, mtime, &h)?;
                    stats.hashed += 1;
                    h
                }
            };

            let image_id = match image_id_for_hash(conn, &content_hash)? {
                Some(id) => {
                    stats.duplicates += 1;
                    id
                }
                None => {
                    stats.new_images += 1;
                    insert_image(conn, &content_hash, &path, geocoder)?
                }
            };

            upsert_file(conn, image_id, source.id, &path_str)?;
            stats.files += 1;

            pending += 1;
            if pending >= COMMIT_EVERY {
                conn.execute_batch("COMMIT; BEGIN")?;
                pending = 0;
            }
            if stats.seen % PROGRESS_EVERY == 0
                && let Some(cb) = on_progress.as_deref_mut()
            {
                cb(&stats);
            }
        }
        Ok(())
    })
    .and_then(|()| {
        if let Some(cb) = on_progress {
            cb(&stats); // siste flush så telleren lander på sluttsummen
        }
        // file_count lagres ikke — det utledes live ved lesing (sources::SELECT).
        conn.execute(
            "UPDATE sources SET last_scanned_at = datetime('now') WHERE id = ?",
            [source.id],
        )?;
        conn.execute_batch("COMMIT")?;
        Ok(())
    });
    if let Err(e) = result {
        let _ = conn.execute_batch("ROLLBACK");
        return Err(e);
    }
    Ok(stats)
}

fn fast_path_hash(conn: &Connection, path: &str, size: i64, mtime: f64) -> Result<Option<String>> {
    let row: Option<(i64, f64, String)> = conn
        .query_row(
            "SELECT size, mtime, content_hash FROM scan_index WHERE path = ?",
            [path],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    Ok(row.and_then(|(s, m, h)| (s == size && m == mtime).then_some(h)))
}

/// SHA-256 (hex) av filinnholdet, lest i 1 MiB-biter.
pub fn hash_file(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; HASH_CHUNK];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn upsert_scan_index(
    conn: &Connection,
    path: &str,
    size: i64,
    mtime: f64,
    hash: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO scan_index (path, size, mtime, content_hash) VALUES (?, ?, ?, ?) \
         ON CONFLICT(path) DO UPDATE SET size=excluded.size, mtime=excluded.mtime, \
         content_hash=excluded.content_hash",
        params![path, size, mtime, hash],
    )?;
    Ok(())
}

fn image_id_for_hash(conn: &Connection, hash: &str) -> Result<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT id FROM images WHERE content_hash = ?",
            [hash],
            |r| r.get(0),
        )
        .optional()?)
}

fn insert_image(
    conn: &Connection,
    hash: &str,
    path: &Path,
    geocoder: &mut Geocoder,
) -> Result<i64> {
    let meta = exif::extract(path);
    let place_name = geocoder.lookup(meta.gps_lat, meta.gps_lon);
    conn.execute(
        "INSERT INTO images (content_hash, width, height, taken_at, year, season, \
         camera_make, camera_model, gps_lat, gps_lon, place_name) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            hash,
            meta.width,
            meta.height,
            meta.taken_at,
            meta.year,
            meta.season,
            meta.camera_make,
            meta.camera_model,
            meta.gps_lat,
            meta.gps_lon,
            place_name
        ],
    )?;
    let image_id = conn.last_insert_rowid();
    for stage in ["derive", "ai", "faces"] {
        conn.execute(
            "INSERT INTO processing_state (image_id, stage, status) VALUES (?, ?, 'pending')",
            params![image_id, stage],
        )?;
    }
    Ok(image_id)
}

fn upsert_file(conn: &Connection, image_id: i64, source_id: i64, path: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO files (image_id, source_id, path) VALUES (?, ?, ?) \
         ON CONFLICT(path) DO UPDATE SET last_seen_at = datetime('now'), \
         image_id = excluded.image_id, source_id = excluded.source_id",
        params![image_id, source_id, path],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endelse_som_splitext() {
        assert_eq!(extension_lower("IMG_1.JPG"), ".jpg");
        assert_eq!(extension_lower("a.b.HeIc"), ".heic");
        assert_eq!(extension_lower(".hidden"), "");
        assert_eq!(extension_lower("..jpg"), "");
        assert_eq!(extension_lower("a."), ".");
        assert_eq!(extension_lower("README"), "");
    }

    #[test]
    fn hash_er_sha256_hex() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x");
        fs::write(&p, b"abc").unwrap();
        assert_eq!(
            hash_file(&p).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
