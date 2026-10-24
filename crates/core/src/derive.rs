//! Derive-steg: nedskalerte arbeidskopier.
//!
//! For hvert bilde med `processing_state(stage='derive', status='pending')` lages to
//! JPEG-er — lengste kant 1024 (AI + lightbox) og 300 (GUI-rutenett) — riktig orientert
//! (EXIF). HEIC leses via libheif. Tungt I/O+CPU-arbeid parallelliseres over en
//! `rayon`-pool. Idempotent: `done`-bilder hoppes over. Én råtten fil stopper ikke
//! jobben — den markeres `error` og resten fortsetter.
//!
//! Bildene behandles og skrives til databasen i **jobbrekkefølge** (som Pythons
//! `pool.map`): arbeiderne trekker neste jobb fra en felles teller, og resultatene
//! sorteres tilbake før de skrives. Da blir `derived.id` den samme som i Python-versjonen,
//! og `processing_state` beveger seg jevnt (commit hver 16.) så `/progress` er live.
//!
//! Bærbarhet: `derived.path` lagrer kun filnavnet (`<hash>_<size>.jpg`), aldri en
//! absolutt sti — resolves ved kjøretid via `storage`.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use rusqlite::{Connection, params};

use crate::cancel;
use crate::error::{Error, Result};
use crate::imageio::{self, DecodeError};
use crate::storage::{self, Storage};
use crate::{jpegenc, resample};

/// `(størrelsesnavn, lengste kant)`; navnet havner i `derived.size` og filnavnet.
pub const SIZES: [(&str, u32); 2] = [("1024", 1024), ("300", 300)];
pub const JPEG_QUALITY: u8 = 85;
/// Commit i batcher så `/progress` beveger seg live under jobben.
const COMMIT_EVERY: usize = 16;
/// Feilteksten når bildet ikke har noen fil som finnes (ordrett som i Python).
pub const MISSING_SOURCE: &str = "ingen lesbar kildefil";

#[derive(Debug, Default, Clone, PartialEq)]
pub struct DeriveStats {
    pub pending: u64,
    pub done: u64,
    pub errors: u64,
}

impl DeriveStats {
    pub fn summary(&self) -> String {
        format!(
            "pending={} done={} feil={}",
            self.pending, self.done, self.errors
        )
    }
}

/// Én jobb: les original, orientér, skriv hver størrelse. Ren av DB.
pub fn derive_one(src: &Path, outputs: &[(PathBuf, u32)]) -> Result<(), DecodeError> {
    let opened = imageio::open_for_derive(src)?;
    let base = &opened.image;
    // Pillow tar med kildens kommentar (`info["comment"]`); for lang til én markør feiler
    // libjpeg (`JERR_BAD_LENGTH`) når den skal lagre.
    let comment = opened.comment.as_deref();
    if comment.is_some_and(|c| c.len() > 65533) {
        // Pillows JPEG-koder bruker libjpegs standard feilutskrift, som skriver
        // meldingen til stderr før unntaket.
        eprintln!("Bogus marker length");
        return Err(DecodeError::EncodeBroken);
    }
    for (out_path, target) in outputs {
        // Lengste kant ≤ mål, beholder aspekt, aldri oppskalering.
        let thumb = resample::thumbnail(base, *target);
        let img = thumb.as_ref().unwrap_or(base);
        std::fs::write(
            out_path,
            jpegenc::encode_with_comment(img, JPEG_QUALITY, comment),
        )?;
    }
    Ok(())
}

struct Job {
    image_id: i64,
    src: PathBuf,
    hash: String,
}

/// Kjør derive på alle pending bilder. `source_id` begrenser til bilder med en fil i
/// den kilden (per-mappe-kjøring); `None` = alle pending. `workers` = antall tråder
/// (`None` = antall kjerner). `on_progress` kalles etter hvert ferdige bilde.
pub fn derive(
    conn: &Connection,
    storage: &Storage,
    workers: Option<usize>,
    source_id: Option<i64>,
    mut on_progress: Option<&mut dyn FnMut(&DeriveStats)>,
) -> Result<DeriveStats> {
    storage.ensure_dirs()?;
    let mut sql = String::from(
        "SELECT i.id AS image_id, i.content_hash AS content_hash,
               (SELECT f.path FROM files f WHERE f.image_id = i.id LIMIT 1) AS src
        FROM images i
        JOIN processing_state ps ON ps.image_id = i.id
        WHERE ps.stage = 'derive' AND ps.status = 'pending'",
    );
    let mut params: Vec<i64> = Vec::new();
    if let Some(id) = source_id {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM files f WHERE f.image_id = i.id AND f.source_id = ?)",
        );
        params.push(id);
    }
    sql.push_str(" ORDER BY i.id");
    let rows: Vec<(i64, String, Option<String>)> = conn
        .prepare(&sql)?
        .query_map(rusqlite::params_from_iter(params), |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?;

    let mut stats = DeriveStats {
        pending: rows.len() as u64,
        ..DeriveStats::default()
    };
    // Transaksjonen committes i batcher underveis (som Pythons `conn.commit()`), så den
    // styres med rå BEGIN/COMMIT i stedet for et `Transaction`-objekt.
    conn.execute_batch("BEGIN")?;
    let result = (|| -> Result<()> {
        let mut jobs = Vec::with_capacity(rows.len());
        for (image_id, hash, src) in rows {
            // `os.path.exists` følger symlenker; brutt lenke = finnes ikke.
            match src.filter(|s| !s.is_empty() && Path::new(s).exists()) {
                Some(src) => jobs.push(Job {
                    image_id,
                    src: PathBuf::from(src),
                    hash,
                }),
                None => {
                    set_state(conn, image_id, "error", Some(MISSING_SOURCE))?;
                    stats.errors += 1;
                }
            }
        }

        let mut since_commit = 0;
        run_ordered(
            &jobs,
            workers,
            |job| {
                let outputs: Vec<(PathBuf, u32)> = SIZES
                    .iter()
                    .map(|(size, target)| (storage.derived_path(&job.hash, size), *target))
                    .collect();
                // En panikk i en dekoder skal bli en myk feil på fila, ikke stoppe jobben.
                catch_unwind(AssertUnwindSafe(|| derive_one(&job.src, &outputs)))
                    .map_err(|panic| format!("Panikk: {}", panic_text(panic.as_ref())))?
                    .map_err(|e| e.to_string())
            },
            |job, outcome| {
                match outcome {
                    Err(error) => {
                        set_state(conn, job.image_id, "error", Some(&error))?;
                        stats.errors += 1;
                    }
                    Ok(()) => {
                        for (size, _) in SIZES {
                            let name = storage::derived_filename(&job.hash, size);
                            upsert_derived(conn, job.image_id, size, &name)?;
                        }
                        set_state(conn, job.image_id, "done", None)?;
                        stats.done += 1;
                    }
                }
                since_commit += 1;
                if since_commit >= COMMIT_EVERY {
                    conn.execute_batch("COMMIT; BEGIN")?;
                    since_commit = 0;
                }
                if let Some(cb) = on_progress.as_deref_mut() {
                    cb(&stats);
                }
                if cancel::requested() {
                    // Trygt punkt: alt til og med dette bildet committes. Arbeiderne gjør
                    // ferdig filene de holder på med; de står som pending og lages på nytt.
                    conn.execute_batch("COMMIT")?;
                    return Err(Error::Cancelled);
                }
                Ok(())
            },
        )?;
        conn.execute_batch("COMMIT")?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = conn.execute_batch("ROLLBACK");
        return Err(e);
    }
    Ok(stats)
}

/// Parallell `map` med resultatene levert til `sink` i inndata-rekkefølge (som Pythons
/// `pool.map`). Arbeiderne trekker neste element fra en felles teller i stedet for
/// rayons rekursive oppdeling, så det som er ferdig er (nesten) et sammenhengende
/// prefiks: `sink` får resultatene fortløpende, og bare noen få venter i bufferet.
/// Feiler `sink`, avbrytes resten (arbeiderne gjør ferdig elementet de holder på med).
pub fn run_ordered<T: Sync, R: Send>(
    items: &[T],
    workers: Option<usize>,
    work: impl Fn(&T) -> R + Sync,
    mut sink: impl FnMut(&T, R) -> Result<()>,
) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers.unwrap_or(0))
        .build()
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let next = AtomicUsize::new(0);
    let (tx, rx) = mpsc::channel::<(usize, R)>();

    std::thread::scope(|scope| {
        scope.spawn(|| {
            let tx = tx; // flyttes hit: kanalen lukkes når alle arbeiderne er ferdige
            pool.broadcast(|_| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(i) else { break };
                    if tx.send((i, work(item))).is_err() {
                        break; // mottakeren har gitt opp
                    }
                }
            });
        });

        let mut buffered: Vec<Option<R>> = (0..items.len()).map(|_| None).collect();
        let mut cursor = 0;
        for (i, result) in rx {
            buffered[i] = Some(result);
            while let Some(result) = buffered.get_mut(cursor).and_then(Option::take) {
                sink(&items[cursor], result)?;
                cursor += 1;
            }
        }
        Ok(())
    })
}

fn panic_text(panic: &(dyn Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "ukjent".into())
}

fn upsert_derived(conn: &Connection, image_id: i64, size: &str, filename: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO derived (image_id, size, path) VALUES (?, ?, ?) \
         ON CONFLICT(image_id, size) DO UPDATE SET path = excluded.path",
        params![image_id, size, filename],
    )?;
    Ok(())
}

fn set_state(conn: &Connection, image_id: i64, status: &str, error: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE processing_state SET status = ?, error = ?, updated_at = datetime('now') \
         WHERE image_id = ? AND stage = 'derive'",
        params![status, error, image_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, paths};

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(name)
    }

    /// Ett bilde med én fil og `derive` pending; returnerer image_id.
    fn add_image(conn: &Connection, hash: &str, path: &Path) -> i64 {
        conn.execute("INSERT INTO images (content_hash) VALUES (?)", [hash])
            .unwrap();
        let id = conn.last_insert_rowid();
        conn.execute(
            "INSERT OR IGNORE INTO sources (id, root_path) VALUES (1, ?)",
            ["/kilde"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO files (image_id, source_id, path) VALUES (?, 1, ?)",
            params![id, paths::to_string(path)],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO processing_state (image_id, stage) VALUES (?, 'derive')",
            [id],
        )
        .unwrap();
        id
    }

    fn state(conn: &Connection, id: i64) -> (String, Option<String>) {
        conn.query_row(
            "SELECT status, error FROM processing_state WHERE image_id = ? AND stage = 'derive'",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    }

    #[test]
    fn myke_feil_rekkefolge_og_idempotens() {
        let tmp = tempfile::tempdir().unwrap();
        let root = paths::resolve(tmp.path());
        let store = Storage::new(&root.join("pakke"));
        store.ensure_dirs().unwrap();
        let conn = db::connect(&store.db_path()).unwrap();
        db::init_db(&conn).unwrap();

        let lib = root.join("bilder");
        std::fs::create_dir_all(&lib).unwrap();
        // HEIC med feil endelse: gjenkjennes på innholdet, som i Pillow.
        std::fs::copy(fixture("astronaut.heic"), lib.join("heic.jpg")).unwrap();
        std::fs::copy(fixture("astronaut.png"), lib.join("png.png")).unwrap();
        std::fs::write(lib.join("korrupt.jpg"), "ikke et bilde").unwrap();
        std::fs::copy(fixture("astronaut_prog422_rst.jpg"), lib.join("prog.jpg")).unwrap();

        let heic = add_image(&conn, "aa", &lib.join("heic.jpg"));
        let png = add_image(&conn, "bb", &lib.join("png.png"));
        let korrupt = add_image(&conn, "cc", &lib.join("korrupt.jpg"));
        let borte = add_image(&conn, "dd", &lib.join("slettet.jpg"));
        let prog = add_image(&conn, "ee", &lib.join("prog.jpg"));

        let mut calls = 0;
        let mut progress = |_: &DeriveStats| calls += 1;
        let stats = derive(&conn, &store, Some(3), None, Some(&mut progress)).unwrap();
        assert_eq!(
            stats,
            DeriveStats {
                pending: 5,
                done: 3,
                errors: 2
            }
        );
        assert_eq!(stats.summary(), "pending=5 done=3 feil=2");
        assert_eq!(calls, 4, "fremdrift per behandlet jobb");

        assert_eq!(
            state(&conn, borte),
            ("error".into(), Some(MISSING_SOURCE.into()))
        );
        let (status, error) = state(&conn, korrupt);
        assert_eq!(status, "error");
        assert!(
            error
                .unwrap()
                .starts_with("UnidentifiedImageError: cannot identify image file")
        );
        for id in [heic, png, prog] {
            assert_eq!(state(&conn, id), ("done".into(), None));
        }

        // derived i jobbrekkefølge (1024 før 300), bare filnavn i `path`.
        let rows: Vec<(i64, i64, String, String)> = conn
            .prepare("SELECT id, image_id, size, path FROM derived ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let order: Vec<(i64, &str)> = rows.iter().map(|(_, i, s, _)| (*i, s.as_str())).collect();
        assert_eq!(
            order,
            [
                (heic, "1024"),
                (heic, "300"),
                (png, "1024"),
                (png, "300"),
                (prog, "1024"),
                (prog, "300")
            ]
        );
        assert_eq!(rows[0].3, "aa_1024.jpg");

        // Størrelser: 512×512 er innenfor 1024 (uendret), 300 skaleres; 203×149 er innenfor begge.
        let dims = |name: &str| image::image_dimensions(store.derived_dir().join(name)).unwrap();
        assert_eq!(dims("aa_1024.jpg"), (512, 512));
        assert_eq!(dims("bb_300.jpg"), (300, 300));
        assert_eq!(dims("ee_300.jpg"), (203, 149));

        // Idempotent: ingenting igjen.
        let again = derive(&conn, &store, None, None, None).unwrap();
        assert_eq!(again.summary(), "pending=0 done=0 feil=0");
    }

    #[test]
    fn source_id_begrenser_til_kilden() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Storage::new(&paths::resolve(tmp.path()));
        store.ensure_dirs().unwrap();
        let conn = db::connect(&store.db_path()).unwrap();
        db::init_db(&conn).unwrap();
        add_image(&conn, "aa", &fixture("astronaut_prog422_rst.jpg"));
        let stats = derive(&conn, &store, Some(1), Some(2), None).unwrap();
        assert_eq!(stats.pending, 0);
        let stats = derive(&conn, &store, Some(1), Some(1), None).unwrap();
        assert_eq!((stats.pending, stats.done), (1, 1));
    }

    #[test]
    fn avbrudd_committer_det_ferdige_og_fortsetter_neste_gang() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;
        let tmp = tempfile::tempdir().unwrap();
        let store = Storage::new(&paths::resolve(tmp.path()));
        store.ensure_dirs().unwrap();
        let conn = db::connect(&store.db_path()).unwrap();
        db::init_db(&conn).unwrap();
        let ids: Vec<i64> = ["aa", "bb", "cc"]
            .iter()
            .map(|h| {
                let path = store.root().join(format!("{h}.jpg"));
                std::fs::copy(fixture("astronaut_prog422_rst.jpg"), &path).unwrap();
                add_image(&conn, h, &path)
            })
            .collect();
        let flag = Arc::new(AtomicBool::new(false));
        let mut stop_after_first = |_: &DeriveStats| flag.store(true, Ordering::Relaxed);
        let result = crate::cancel::scoped(flag.clone(), || {
            derive(&conn, &store, Some(1), None, Some(&mut stop_after_first))
        });
        assert!(matches!(result, Err(crate::Error::Cancelled)));
        assert_eq!(state(&conn, ids[0]), ("done".into(), None));
        assert_eq!(state(&conn, ids[2]).0, "pending");
        let rest = derive(&conn, &store, Some(1), None, None).unwrap();
        assert_eq!((rest.pending, rest.done), (2, 2));
    }

    #[test]
    fn run_ordered_leverer_i_rekkefolge() {
        let items: Vec<u64> = (0..200).collect();
        let mut seen = Vec::new();
        run_ordered(
            &items,
            Some(8),
            |&i| {
                // Ujevn arbeidsmengde, så trådene blir ferdige i tilfeldig rekkefølge.
                std::thread::sleep(std::time::Duration::from_micros((i * 7919) % 500));
                i * 2
            },
            |&i, r| {
                assert_eq!(r, i * 2);
                seen.push(i);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(seen, items);
    }
}
