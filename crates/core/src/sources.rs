//! Kilde-mapper: legg til, list, fjern + klargjør pakke for handoff.
//!
//! En `source` er en mappe brukeren vil indeksere. **Jernregel: kilde-filer på disk
//! røres aldri herfra.** `remove` sletter kun appens egne data — DB-rader og de
//! avledede `derived/<hash>_*.jpg`-filene — med orphan-opprydding for bilder som
//! mister alle sine fysiske kopier.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;

use crate::error::{Error, Result};
use crate::paths;
use crate::storage::Storage;

/// Alle kilde-lesninger går via denne. `file_count` beregnes **live** som en korrelert
/// COUNT av `files` — et lagret øyeblikksbilde ville lyve når overlappende kilder
/// reassigner delte filers `source_id`. (Kolonnen `sources.file_count` er vestigial.)
const SELECT: &str = "SELECT id, root_path, source_type, added_at, last_scanned_at, profile_id, \
     (SELECT COUNT(*) FROM files WHERE files.source_id = sources.id) AS file_count \
     FROM sources";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Source {
    pub id: i64,
    pub root_path: String,
    pub source_type: String,
    pub added_at: String,
    pub last_scanned_at: Option<String>,
    pub profile_id: Option<i64>,
    pub file_count: i64,
}

fn from_row(row: &Row) -> rusqlite::Result<Source> {
    Ok(Source {
        id: row.get("id")?,
        root_path: row.get("root_path")?,
        source_type: row.get("source_type")?,
        added_at: row.get("added_at")?,
        last_scanned_at: row.get("last_scanned_at")?,
        profile_id: row.get("profile_id")?,
        file_count: row.get("file_count")?,
    })
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct RemoveStats {
    pub sources_removed: i64,
    pub files_removed: i64,
    pub scan_index_removed: i64,
    /// Orphans ryddet helt.
    pub images_removed: i64,
    /// Fysiske thumbnails slettet fra disk.
    pub derived_files_removed: i64,
}

impl std::ops::AddAssign for RemoveStats {
    fn add_assign(&mut self, o: Self) {
        self.sources_removed += o.sources_removed;
        self.files_removed += o.files_removed;
        self.scan_index_removed += o.scan_index_removed;
        self.images_removed += o.images_removed;
        self.derived_files_removed += o.derived_files_removed;
    }
}

impl RemoveStats {
    pub fn summary(&self) -> String {
        format!(
            "kilder={} files={} scan_index={} orphan-bilder={} thumbnails={}",
            self.sources_removed,
            self.files_removed,
            self.scan_index_removed,
            self.images_removed,
            self.derived_files_removed
        )
    }
}

/// Registrer en kilde-mappe. `root` lagres som absolutt, resolvet sti.
///
/// `profile_id` (valgfritt) peker på en tagging-profil; `None` = arv globalt
/// `settings['default_profile_id']`.
pub fn add(
    conn: &Connection,
    root: &Path,
    source_type: &str,
    profile_id: Option<i64>,
) -> Result<Source> {
    let path = paths::resolve(root);
    if !path.exists() {
        return Err(Error::Source(format!(
            "Mappa finnes ikke: {}",
            path.display()
        )));
    }
    if !path.is_dir() {
        return Err(Error::Source(format!("Ikke en mappe: {}", path.display())));
    }
    let root_str = paths::to_string(&path);
    let inserted = conn.execute(
        "INSERT INTO sources (root_path, source_type, profile_id) VALUES (?, ?, ?)",
        params![root_str, source_type, profile_id],
    );
    match inserted {
        Err(rusqlite::Error::SqliteFailure(e, _))
            if e.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            return Err(Error::Source(format!(
                "Kilden er allerede lagt til: {root_str}"
            )));
        }
        other => {
            other?;
        }
    }
    get_by_path(conn, &path)?.ok_or_else(|| Error::Source(format!("Ukjent kilde: {root_str}")))
}

/// Sett (eller fjern) per-kilde tagging-profil. `None` = arv globalt.
///
/// Endrer kun `sources.profile_id` — bilder merkes ikke på nytt automatisk
/// (penge-disiplin); re-merking går via den gated `tag --retag`-flyten.
pub fn set_profile(conn: &Connection, target: &str, profile_id: Option<i64>) -> Result<Source> {
    let src = resolve_target(conn, target)?
        .ok_or_else(|| Error::Source(format!("Ukjent kilde: {target}")))?;
    conn.execute(
        "UPDATE sources SET profile_id = ? WHERE id = ?",
        params![profile_id, src.id],
    )?;
    get_by_id(conn, src.id)?.ok_or_else(|| Error::Source(format!("Ukjent kilde: {target}")))
}

pub fn list_sources(conn: &Connection) -> Result<Vec<Source>> {
    let mut stmt = conn.prepare(&format!("{SELECT} ORDER BY id"))?;
    let rows = stmt
        .query_map([], from_row)?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

pub fn get_by_id(conn: &Connection, id: i64) -> Result<Option<Source>> {
    Ok(conn
        .query_row(&format!("{SELECT} WHERE id = ?"), [id], from_row)
        .optional()?)
}

pub fn get_by_path(conn: &Connection, root: &Path) -> Result<Option<Source>> {
    let path = paths::to_string(&paths::resolve(root));
    Ok(conn
        .query_row(&format!("{SELECT} WHERE root_path = ?"), [path], from_row)
        .optional()?)
}

/// Kilde fra id (bare sifre) eller sti.
fn resolve_target(conn: &Connection, target: &str) -> Result<Option<Source>> {
    if !target.is_empty() && target.chars().all(|c| c.is_ascii_digit()) {
        return match target.parse::<i64>() {
            Ok(id) => get_by_id(conn, id),
            Err(_) => Ok(None),
        };
    }
    get_by_path(conn, &PathBuf::from(target))
}

/// Escape `%`, `_` og `\` for `LIKE … ESCAPE '\'`.
fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Fjern en kilde + dens app-data. Kilde-filer på disk røres ALDRI.
///
/// 1. Slett `sources`-raden (`files` cascade-slettes via FK) + `scan_index` under roten.
/// 2. Orphan-opprydding: bilder som mister *alle* `files` slettes helt (cascade til
///    derived/ai_tags/image_tags/faces/processing_state/album_items) **og** de fysiske
///    `derived/<hash>_*.jpg` på disk. Bilder med kopier i andre kilder blir liggende.
pub fn remove(conn: &Connection, storage: &Storage, target: &str) -> Result<RemoveStats> {
    let src = resolve_target(conn, target)?
        .ok_or_else(|| Error::Source(format!("Ukjent kilde: {target}")))?;
    let mut stats = RemoveStats {
        sources_removed: 1,
        ..Default::default()
    };

    let candidates: Vec<i64> = conn
        .prepare("SELECT DISTINCT image_id FROM files WHERE source_id = ?")?
        .query_map([src.id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    stats.files_removed = conn.query_row(
        "SELECT COUNT(*) FROM files WHERE source_id = ?",
        [src.id],
        |r| r.get(0),
    )?;

    let sep = std::path::MAIN_SEPARATOR;
    let prefix = format!("{}{sep}", src.root_path.trim_end_matches(sep));

    let tx = conn.unchecked_transaction()?;
    // files cascade-slettes når sources-raden går
    tx.execute("DELETE FROM sources WHERE id = ?", [src.id])?;
    stats.scan_index_removed = tx.execute(
        "DELETE FROM scan_index WHERE path LIKE ? ESCAPE '\\'",
        [format!("{}%", like_escape(&prefix))],
    )? as i64;

    for image_id in candidates {
        let still_referenced = tx
            .query_row(
                "SELECT 1 FROM files WHERE image_id = ? LIMIT 1",
                [image_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if still_referenced {
            continue; // kopi finnes i annen kilde — la bildet ligge
        }
        // orphan: slett fysiske thumbnails før raden (basename → resolv via storage)
        let derived: Vec<String> = tx
            .prepare("SELECT path FROM derived WHERE image_id = ?")?
            .query_map([image_id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for name in derived {
            let fp = storage.derived_dir().join(name);
            if fp.exists() {
                std::fs::remove_file(&fp)?;
                stats.derived_files_removed += 1;
            }
        }
        tx.execute("DELETE FROM images WHERE id = ?", [image_id])?;
        stats.images_removed += 1;
    }
    tx.commit()?;
    Ok(stats)
}

/// Tøm pakka for deling: fjern alle kilder (med opprydding) + tøm settings.
///
/// Til slutt `VACUUM` så slettede rader ikke overlever som lesbare bytes i
/// frilistesider — ellers kan en API-nøkkel graves ut av `fotomaxxer.db` med
/// `strings` etter handoff. Kilde-originalene på disk er urørt.
pub fn prepare_for_handoff(conn: &Connection, storage: &Storage) -> Result<RemoveStats> {
    let mut total = RemoveStats::default();
    for src in list_sources(conn)? {
        total += remove(conn, storage, &src.id.to_string())?;
    }
    conn.execute("DELETE FROM settings", [])?;
    conn.execute_batch("VACUUM")?;
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    struct Env {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        store: Storage,
        conn: Connection,
    }

    fn env() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let root = paths::resolve(tmp.path());
        let store = Storage::new(&root.join("pakke"));
        store.ensure_dirs().unwrap();
        let conn = db::connect(&store.db_path()).unwrap();
        db::init_db(&conn).unwrap();
        Env {
            _tmp: tmp,
            root,
            store,
            conn,
        }
    }

    /// Ett bilde med én fil i kilden, ett derivat på disk.
    fn add_image(e: &Env, source_id: i64, hash: &str, path: &Path) -> i64 {
        e.conn
            .execute("INSERT INTO images (content_hash) VALUES (?)", [hash])
            .unwrap();
        let image_id = e.conn.last_insert_rowid();
        e.conn
            .execute(
                "INSERT INTO files (image_id, source_id, path) VALUES (?, ?, ?)",
                params![image_id, source_id, paths::to_string(path)],
            )
            .unwrap();
        let name = format!("{hash}_300.jpg");
        std::fs::write(e.store.derived_dir().join(&name), "thumb").unwrap();
        e.conn
            .execute(
                "INSERT INTO derived (image_id, size, path) VALUES (?, '300', ?)",
                params![image_id, name],
            )
            .unwrap();
        image_id
    }

    #[test]
    fn add_validerer_og_avviser_duplikat() {
        let e = env();
        let dir = e.root.join("bilder æøå");
        std::fs::create_dir_all(&dir).unwrap();
        let s = add(&e.conn, &dir, "local", None).unwrap();
        assert_eq!(s.root_path, paths::to_string(&dir));
        assert_eq!(s.file_count, 0);
        let dup = add(&e.conn, &dir.join("."), "local", None).unwrap_err();
        assert_eq!(
            dup.to_string(),
            format!("Kilden er allerede lagt til: {}", dir.display())
        );
        let missing = add(&e.conn, &e.root.join("finnes-ikke"), "local", None).unwrap_err();
        assert!(missing.to_string().starts_with("Mappa finnes ikke: "));
        std::fs::write(e.root.join("fil.txt"), "x").unwrap();
        assert!(
            add(&e.conn, &e.root.join("fil.txt"), "local", None)
                .unwrap_err()
                .to_string()
                .starts_with("Ikke en mappe: ")
        );
    }

    #[test]
    fn remove_rydder_orphans_men_beholder_delte_bilder_og_originaler() {
        let e = env();
        let (a, b) = (e.root.join("a"), e.root.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let sa = add(&e.conn, &a, "local", None).unwrap();
        let sb = add(&e.conn, &b, "local", None).unwrap();
        let original = a.join("x.jpg");
        std::fs::write(&original, "original").unwrap();

        let orphan = add_image(&e, sa.id, "orphan", &original);
        let shared = add_image(&e, sa.id, "shared", &a.join("y.jpg"));
        e.conn
            .execute(
                "INSERT INTO files (image_id, source_id, path) VALUES (?, ?, ?)",
                params![shared, sb.id, paths::to_string(&b.join("y.jpg"))],
            )
            .unwrap();
        e.conn
            .execute(
                "INSERT INTO scan_index (path, size, mtime, content_hash) VALUES (?, 1, 1.0, 'orphan')",
                [paths::to_string(&original)],
            )
            .unwrap();
        assert_eq!(list_sources(&e.conn).unwrap()[0].file_count, 2);

        let stats = remove(&e.conn, &e.store, &sa.id.to_string()).unwrap();
        assert_eq!(
            stats,
            RemoveStats {
                sources_removed: 1,
                files_removed: 2,
                scan_index_removed: 1,
                images_removed: 1,
                derived_files_removed: 1
            }
        );
        let images: Vec<i64> = e
            .conn
            .prepare("SELECT id FROM images")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(images, [shared]);
        assert!(!images.contains(&orphan));
        assert!(!e.store.derived_dir().join("orphan_300.jpg").exists());
        assert!(e.store.derived_dir().join("shared_300.jpg").exists());
        assert_eq!(
            std::fs::read_to_string(&original).unwrap(),
            "original",
            "jernregelen"
        );
        assert!(remove(&e.conn, &e.store, &sa.id.to_string()).is_err());
    }

    #[test]
    fn handoff_tommer_alt_og_nokkelen_lekker_ikke() {
        let e = env();
        let a = e.root.join("a");
        std::fs::create_dir_all(&a).unwrap();
        let sa = add(&e.conn, &a, "local", None).unwrap();
        add_image(&e, sa.id, "h1", &a.join("x.jpg"));
        let secret = "sk-or-v1-HEMMELIG-NOKKEL-1234567890";
        crate::settings::set_setting(&e.conn, "openrouter_api_key", secret).unwrap();

        let stats = prepare_for_handoff(&e.conn, &e.store).unwrap();
        assert_eq!(stats.sources_removed, 1);
        let count = |t: &str| -> i64 {
            e.conn
                .query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(
            (count("sources"), count("images"), count("settings")),
            (0, 0, 0)
        );
        let bytes = std::fs::read(e.store.db_path()).unwrap();
        assert!(
            !bytes.windows(secret.len()).any(|w| w == secret.as_bytes()),
            "nøkkelen finnes i db-fila"
        );
    }

    #[test]
    fn like_prefiks_escapes() {
        assert_eq!(like_escape(r"C:\a_b%"), r"C:\\a\_b\%");
    }
}
