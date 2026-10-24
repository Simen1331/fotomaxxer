//! Faces-steg: deteksjon + ArcFace-embedding, og klynging til personer.
//!
//! To operasjoner mot samme stage (`faces`), portert fra `fotomaxxer/faces.py`:
//!
//! * [`detect`] — per bilde: SCRFD + ArcFace (`fotomaxxer-faces`) på 1024-derivatet →
//!   én `faces`-rad per ansikt (bbox i **originalkoordinater**, embedding som
//!   L2-normalisert `f32`-BLOB). 0 ansikter er gyldig → `done` likevel. CPU-bundet →
//!   flere analysatorer i parallelle tråder. Myk per-bilde-feil.
//! * [`cluster`] — jobber på lagrede vektorer, ikke bilder. Inkrementell tilordning av
//!   NULL-ansikter til nærmeste eksisterende klynge er standard (bevarer navn brukeren har
//!   satt); HDBSCAN føder nye klynger fra resten. `full` re-klynger alt og re-mapper
//!   navn via sentroide-matching.
//!
//! Kurateringsfunksjonene (GUI-mutasjoner) committer selv og gir [`Error::Faces`] med
//! samme tekst som Pythons `ValueError` ved ukjent ansikt/person.
//!
//! Klyngingen gir samme database som Python for samme embeddings: HDBSCAN, sentroider
//! og likheter regnes bit-likt (se `fotomaxxer_faces::{hdbscan, linalg}`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;

use fotomaxxer_faces::{EMBED_DIM, FaceAnalyzer, hdbscan, linalg};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};

use crate::error::{Error, Result};
use crate::storage::Storage;
use crate::{cancel, imageio, pyjson, resources, settings};

#[derive(Debug, Default, Clone, PartialEq)]
pub struct FaceStats {
    pub pending: u64,
    pub done: u64,
    pub errors: u64,
    pub faces_found: u64,
}

impl FaceStats {
    pub fn summary(&self) -> String {
        format!(
            "pending={} done={} ansikter={} feil={}",
            self.pending, self.done, self.faces_found, self.errors
        )
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ClusterStats {
    pub faces_total: u64,
    pub assigned_incremental: u64,
    pub new_clusters: u64,
    pub clusters_total: u64,
    pub unassigned: u64,
    pub names_remapped: u64,
    pub full: bool,
}

impl ClusterStats {
    pub fn summary(&self) -> String {
        let head = if self.full {
            "full re-klynging"
        } else {
            "inkrementell"
        };
        format!(
            "{head}: ansikter={} klynger={} tilordnet={} nye={} utilordnet={} navn-remappet={}",
            self.faces_total,
            self.clusters_total,
            self.assigned_incremental,
            self.new_clusters,
            self.unassigned,
            self.names_remapped
        )
    }
}

// --- Deteksjon ---------------------------------------------------------------

/// Ett bilde som skal analyseres.
struct Job {
    image_id: i64,
    derived: PathBuf,
    width: Option<i64>,
    height: Option<i64>,
}

/// Ansiktene i ett bilde: (`bbox`-JSON, embedding-bytes), eller en feilmelding.
type Outcome = std::result::Result<Vec<(String, Vec<u8>)>, String>;

/// Detektér ansikter på pending (ev. error) bilder → `faces`-rader. Idempotent.
///
/// `workers` = antall parallelle analysatorer (standard: se [`plan_workers`]).
/// `source_id` begrenser til bilder med en fil i den kilden (per-mappe-kjøring);
/// `None` = alle pending. Resultatene skrives og committes ett bilde om gangen i
/// bilde-id-rekkefølge, så `faces`/`processing_state` beveger seg live under kjøringen.
pub fn detect(
    conn: &Connection,
    storage: &Storage,
    workers: Option<usize>,
    retry_errors: bool,
    source_id: Option<i64>,
) -> Result<FaceStats> {
    let jobs = pending_jobs(conn, storage, retry_errors, source_id)?;
    let mut stats = FaceStats {
        pending: jobs.len() as u64,
        ..Default::default()
    };
    if jobs.is_empty() {
        return Ok(stats);
    }
    let model_dir = resources::face_model_dir()?;
    let (workers, intra_threads) = plan_workers(workers, jobs.len());
    let analyzers = load_analyzers(&model_dir, workers, intra_threads)?;

    let next = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let (tx, rx) = mpsc::channel::<(usize, Outcome)>();
    std::thread::scope(|scope| {
        for mut analyzer in analyzers {
            let (tx, jobs, next, stop) = (tx.clone(), &jobs, &next, &stop);
            scope.spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(job) = jobs.get(i) else { break };
                    if tx.send((i, detect_one(&mut analyzer, job))).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        // Skriv i bilde-rekkefølge (som Pythons ordnede `pool.map`) etter hvert som
        // resultatene kommer; et DB-feil stopper arbeiderne og returneres.
        let mut ready: BTreeMap<usize, Outcome> = BTreeMap::new();
        let mut write_at = 0usize;
        let result = (|| -> Result<()> {
            for (i, outcome) in rx.iter() {
                ready.insert(i, outcome);
                while let Some(outcome) = ready.remove(&write_at) {
                    persist(conn, jobs[write_at].image_id, outcome, &mut stats)?;
                    write_at += 1;
                }
                if cancel::requested() {
                    // Trygt punkt: hvert bilde committes for seg. Arbeiderne gjør ferdig
                    // bildet de holder på med; det som kommer i rekkefølge skrives ennå.
                    stop.store(true, Ordering::Relaxed);
                }
            }
            if cancel::requested() && write_at < jobs.len() {
                return Err(Error::Cancelled);
            }
            Ok(())
        })();
        if result.is_err() {
            stop.store(true, Ordering::Relaxed);
        }
        result
    })?;
    Ok(stats)
}

/// Bildene som venter på ansiktsdeteksjon, i id-rekkefølge (samme SQL som Python).
fn pending_jobs(
    conn: &Connection,
    storage: &Storage,
    retry_errors: bool,
    source_id: Option<i64>,
) -> Result<Vec<Job>> {
    let statuses: &[&str] = if retry_errors {
        &["pending", "error"]
    } else {
        &["pending"]
    };
    let placeholders = vec!["?"; statuses.len()].join(",");
    let source_clause = if source_id.is_some() {
        " AND EXISTS (SELECT 1 FROM files f WHERE f.image_id = i.id AND f.source_id = ?)"
    } else {
        ""
    };
    let sql = format!(
        "SELECT i.id, i.content_hash, i.width, i.height \
         FROM images i \
         JOIN processing_state pf ON pf.image_id = i.id AND pf.stage = 'faces' \
                                 AND pf.status IN ({placeholders}) \
         JOIN processing_state pd ON pd.image_id = i.id AND pd.stage = 'derive' AND pd.status = 'done' \
         JOIN derived d           ON d.image_id  = i.id AND d.size = '1024' \
         WHERE 1=1{source_clause} \
         ORDER BY i.id"
    );
    let mut values: Vec<rusqlite::types::Value> =
        statuses.iter().map(|s| s.to_string().into()).collect();
    if let Some(id) = source_id {
        values.push(id.into());
    }
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params_from_iter(values), |r| {
            Ok(Job {
                image_id: r.get(0)?,
                derived: storage.derived_path(&r.get::<_, String>(1)?, "1024"),
                width: r.get(2)?,
                height: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// Antall analysatorer og ONNX-tråder per analysator.
///
/// Målt på Ryzen 7 3700X (16 tråder, 240 bilder): 1 analysator med alle kjerner
/// 114 ms/bilde, 2 → 88, 4 → 78, 8 → 74, 16 → 89. Hver analysator holder sine egne
/// ONNX-sesjoner (~430 MB), og `Session::run` krever eksklusiv tilgang, så flere
/// analysatorer koster minne lineært. Standard er derfor `kjerner / 4`, begrenset til
/// 1–2 (maks ~0,9 GB, så en svak Windows-maskin tåler det), og kjernene deles likt
/// mellom dem. Antall tråder endrer ikke resultatet
/// (embeddingene er bit-like med 1 og 16 tråder). `workers` overstyrer antallet.
pub fn plan_workers(workers: Option<usize>, jobs: usize) -> (usize, Option<usize>) {
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    let n = workers
        .unwrap_or((cores / 4).clamp(1, 2))
        .clamp(1, jobs.max(1));
    (n, Some((cores / n).max(1)))
}

/// Last `n` analysatorer parallelt (modellinnlasting tar noen hundre ms hver).
fn load_analyzers(
    model_dir: &Path,
    n: usize,
    intra_threads: Option<usize>,
) -> Result<Vec<FaceAnalyzer>> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..n)
            .map(|_| scope.spawn(|| FaceAnalyzer::with_threads(model_dir, intra_threads)))
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .map_err(|_| Error::Models("modell-lasting krasjet".into()))?
                    .map_err(|e| {
                        Error::Models(format!(
                            "kunne ikke laste ansiktsmodellene fra {}: {e}",
                            model_dir.display()
                        ))
                    })
            })
            .collect()
    })
}

/// Les 1024-derivatet, detektér ansikter og skalér bbox til originalkoordinater.
fn detect_one(analyzer: &mut FaceAnalyzer, job: &Job) -> Outcome {
    let img =
        imageio::open_rgb(&job.derived).map_err(|e| format!("kunne ikke lese derivat: {e}"))?;
    let (thumb_w, thumb_h) = img.dimensions();
    let faces = analyzer
        .analyze(&img)
        .map_err(|e| format!("ansiktsanalyse feilet: {e}"))?;
    let (sx, sy) = (scale(job.width, thumb_w), scale(job.height, thumb_h));
    Ok(faces
        .iter()
        .map(|f| {
            let [x1, y1, x2, y2] = f.detection.bbox;
            let bbox = [
                py_round(x1 * sx),
                py_round(y1 * sy),
                py_round((x2 - x1) * sx),
                py_round((y2 - y1) * sy),
            ];
            let emb: Vec<u8> = f.embedding.iter().flat_map(|v| v.to_le_bytes()).collect();
            (pyjson::dumps(&bbox), emb)
        })
        .collect())
}

/// `(orig / thumb) if orig else 1.0` i Python-`float`, brukt mot `float32`-bbox
/// (numpy regner `float32 * float` i `float32`).
fn scale(orig: Option<i64>, thumb: u32) -> f32 {
    match orig {
        Some(o) if o != 0 && thumb != 0 => (o as f64 / thumb as f64) as f32,
        _ => 1.0,
    }
}

/// Pythons `round()` på en `float32`: nærmeste heltall, halvveis til partall.
fn py_round(v: f32) -> i64 {
    v.round_ties_even() as i64
}

/// Erstatt `faces` for bildet og sett faces-state (done også for 0 ansikter, ellers error).
fn persist(
    conn: &Connection,
    image_id: i64,
    outcome: Outcome,
    stats: &mut FaceStats,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    match outcome {
        Ok(faces) => {
            tx.execute("DELETE FROM faces WHERE image_id = ?", [image_id])?;
            for (bbox, emb) in &faces {
                // locked=0: et nydetektert ansikt er ferskt — re-deteksjon nullstiller
                // en eventuell tidligere frys (raden er uansett slettet over).
                tx.execute(
                    "INSERT INTO faces (image_id, bbox, embedding, cluster_id, locked) \
                     VALUES (?, ?, ?, NULL, 0)",
                    params![image_id, bbox, emb],
                )?;
            }
            set_state(&tx, image_id, "done", None)?;
            stats.faces_found += faces.len() as u64;
            stats.done += 1;
        }
        Err(message) => {
            set_state(&tx, image_id, "error", Some(&message))?;
            stats.errors += 1;
        }
    }
    tx.commit()?;
    Ok(())
}

fn set_state(conn: &Connection, image_id: i64, status: &str, error: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE processing_state SET status = ?, error = ?, updated_at = datetime('now') \
         WHERE image_id = ? AND stage = 'faces'",
        params![status, error, image_id],
    )?;
    Ok(())
}

// --- Klynging ----------------------------------------------------------------

/// Alle ansikter i id-rekkefølge med embedding, klynge og lås.
struct FaceSet {
    ids: Vec<i64>,
    embs: Vec<Vec<f32>>,
    cluster_ids: Vec<Option<i64>>,
    locked: Vec<bool>,
}

impl FaceSet {
    fn load(conn: &Connection) -> Result<Self> {
        let mut set = FaceSet {
            ids: Vec::new(),
            embs: Vec::new(),
            cluster_ids: Vec::new(),
            locked: Vec::new(),
        };
        let mut stmt =
            conn.prepare("SELECT id, embedding, cluster_id, locked FROM faces ORDER BY id")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let id: i64 = r.get(0)?;
            let blob: Vec<u8> = r.get(1)?;
            if blob.len() != EMBED_DIM * 4 {
                return Err(Error::Faces(format!(
                    "ansikt {id} har ugyldig embedding ({} byte, ventet {})",
                    blob.len(),
                    EMBED_DIM * 4
                )));
            }
            set.ids.push(id);
            set.embs.push(
                blob.as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| f32::from_le_bytes(*c))
                    .collect(),
            );
            set.cluster_ids.push(r.get(2)?);
            set.locked.push(r.get::<_, i64>(3)? != 0);
        }
        Ok(set)
    }

    fn len(&self) -> usize {
        self.ids.len()
    }

    /// Sentroide av ansiktene med indeksene `members`.
    fn centroid(&self, members: &[usize]) -> Vec<f32> {
        let rows: Vec<&[f32]> = members.iter().map(|&i| self.embs[i].as_slice()).collect();
        linalg::centroid(&rows)
    }

    /// Indeksene til ansiktene i klyngen `cid` (låste medregnet).
    fn members_of(&self, cid: i64) -> Vec<usize> {
        (0..self.len())
            .filter(|&i| self.cluster_ids[i] == Some(cid))
            .collect()
    }

    fn hdbscan(&self, subset: &[usize], min_cluster_size: usize) -> Result<Vec<i64>> {
        let points: Vec<&[f32]> = subset.iter().map(|&i| self.embs[i].as_slice()).collect();
        hdbscan::fit_predict(&points, min_cluster_size).map_err(|e| Error::Faces(e.to_string()))
    }
}

/// Klyng ansikter til personer. Inkrementell (standard) eller full re-klynging.
///
/// `threshold` = cosinus-terskel for tilordning (standard: `face_match_threshold`).
pub fn cluster(conn: &Connection, full: bool, threshold: Option<f64>) -> Result<ClusterStats> {
    let threshold = match threshold {
        Some(t) => t,
        None => setting_or(conn, "face_match_threshold", 0.5, parse_py_float)?,
    };
    let min_cluster_size = setting_or(conn, "face_min_cluster_size", 3, parse_py_int)?;

    let faces = FaceSet::load(conn)?;
    let mut stats = ClusterStats {
        faces_total: faces.len() as u64,
        full,
        ..Default::default()
    };
    if faces.len() == 0 {
        return Ok(stats);
    }

    let tx = conn.unchecked_transaction()?;
    if full {
        cluster_full(&tx, &faces, min_cluster_size, threshold, &mut stats)?;
    } else {
        cluster_incremental(&tx, &faces, min_cluster_size, threshold, &mut stats)?;
    }
    tx.commit()?;
    let count =
        |sql: &str| -> Result<u64> { Ok(conn.query_row(sql, [], |r| r.get::<_, i64>(0))? as u64) };
    stats.clusters_total = count("SELECT COUNT(*) FROM persons")?;
    stats.unassigned = count("SELECT COUNT(*) FROM faces WHERE cluster_id IS NULL")?;
    Ok(stats)
}

/// `T(get_setting(key) or default)`: tom/manglende verdi gir standarden.
fn setting_or<T>(
    conn: &Connection,
    key: &str,
    default: T,
    parse: fn(&str) -> Option<T>,
) -> Result<T> {
    match settings::get_setting(conn, key)? {
        Some(v) if !v.is_empty() => {
            parse(&v).ok_or_else(|| Error::Faces(format!("ugyldig verdi for {key}: {v:?}")))
        }
        _ => Ok(default),
    }
}

/// Pythons `float(s)`: tåler omkringliggende mellomrom.
fn parse_py_float(s: &str) -> Option<f64> {
    s.trim().parse().ok()
}

/// Pythons `int(s)` for ikke-negative størrelser.
fn parse_py_int(s: &str) -> Option<usize> {
    let t = s.trim();
    t.strip_prefix('+').unwrap_or(t).parse().ok()
}

/// Nærmeste sentroide (første ved likhet, som Pythons `max`) og likheten.
fn best_match(v: &[f32], centroids: &[(i64, Vec<f32>)]) -> Option<(i64, f64)> {
    let mut best: Option<(i64, f64)> = None;
    for (cid, c) in centroids {
        let sim = linalg::sdot(v, c) as f64;
        if best.is_none_or(|(_, b)| sim > b) {
            best = Some((*cid, sim));
        }
    }
    best
}

/// Opprett én ny person per HDBSCAN-etikett (stigende, støy hoppes over) og tilordne
/// medlemmene. Returnerer (cluster_id, medlemmer) i opprettelsesrekkefølge.
fn birth(
    conn: &Connection,
    faces: &FaceSet,
    pool: &[usize],
    labels: &[i64],
) -> Result<Vec<(i64, Vec<usize>)>> {
    let mut by_label: BTreeMap<i64, Vec<usize>> = BTreeMap::new();
    for (j, &label) in labels.iter().enumerate() {
        if label != hdbscan::NOISE {
            by_label.entry(label).or_default().push(pool[j]);
        }
    }
    let mut born = Vec::new();
    for members in by_label.into_values() {
        conn.execute("INSERT INTO persons (name) VALUES (NULL)", [])?;
        let new_cid = conn.last_insert_rowid();
        for &i in &members {
            conn.execute(
                "UPDATE faces SET cluster_id = ? WHERE id = ?",
                params![new_cid, faces.ids[i]],
            )?;
        }
        born.push((new_cid, members));
    }
    Ok(born)
}

fn cluster_incremental(
    conn: &Connection,
    faces: &FaceSet,
    min_cluster_size: usize,
    threshold: f64,
    stats: &mut ClusterStats,
) -> Result<()> {
    // Sentroider regnes fra klynge-medlemmer (også låste); kun NULL-poolen tilordnes,
    // og låste ansikter holdes utenfor den (et ignorert søppel-ansikt med locked=1,
    // cluster_id=NULL blir aldri re-tilordnet).
    let mut centroids: Vec<(i64, Vec<f32>)> = Vec::new();
    let mut stmt = conn.prepare("SELECT cluster_id FROM persons")?;
    let person_ids: Vec<i64> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for cid in person_ids {
        let members = faces.members_of(cid);
        if !members.is_empty() {
            centroids.push((cid, faces.centroid(&members)));
        }
    }

    let mut remaining = Vec::new();
    for i in (0..faces.len()).filter(|&i| faces.cluster_ids[i].is_none() && !faces.locked[i]) {
        if let Some((cid, sim)) = best_match(&faces.embs[i], &centroids)
            && sim >= threshold
        {
            conn.execute(
                "UPDATE faces SET cluster_id = ? WHERE id = ?",
                params![cid, faces.ids[i]],
            )?;
            stats.assigned_incremental += 1;
            continue;
        }
        remaining.push(i);
    }

    // Fødsel: HDBSCAN på gjenstående NULL-pool alene.
    if remaining.len() >= min_cluster_size {
        let labels = faces.hdbscan(&remaining, min_cluster_size)?;
        stats.new_clusters += birth(conn, faces, &remaining, &labels)?.len() as u64;
    }
    Ok(())
}

fn cluster_full(
    conn: &Connection,
    faces: &FaceSet,
    min_cluster_size: usize,
    threshold: f64,
    stats: &mut ClusterStats,
) -> Result<()> {
    // Snapshot navngitte klyngers sentroider FØR vi river dem (medlemskap inkluderer
    // låste medlemmer, jf. inkrementell).
    let mut old_named: Vec<(String, Vec<f32>)> = Vec::new();
    let mut stmt = conn.prepare("SELECT cluster_id, name FROM persons WHERE name IS NOT NULL")?;
    let named: Vec<(i64, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (cid, name) in named {
        let members = faces.members_of(cid);
        if !members.is_empty() {
            old_named.push((name, faces.centroid(&members)));
        }
    }

    // Full re-klynging rører kun ikke-låste ansikter. Låste beholder cluster_id
    // (manuelt tilordnet) eller forblir NULL (ignorert søppel) — uberørt.
    let movable: Vec<usize> = (0..faces.len()).filter(|&i| !faces.locked[i]).collect();

    // Nullstill kun de flyttbare; så slett personer ingen (låst) ansikt lenger peker på.
    for &i in &movable {
        if faces.cluster_ids[i].is_some() {
            conn.execute(
                "UPDATE faces SET cluster_id = NULL WHERE id = ?",
                [faces.ids[i]],
            )?;
        }
    }
    conn.execute(
        "DELETE FROM persons WHERE cluster_id NOT IN \
         (SELECT cluster_id FROM faces WHERE cluster_id IS NOT NULL)",
        [],
    )?;

    let mut new_centroids: Vec<(i64, Vec<f32>)> = Vec::new();
    if movable.len() >= min_cluster_size {
        let labels = faces.hdbscan(&movable, min_cluster_size)?;
        for (cid, members) in birth(conn, faces, &movable, &labels)? {
            new_centroids.push((cid, faces.centroid(&members)));
            stats.new_clusters += 1;
            stats.assigned_incremental += members.len() as u64;
        }
    }

    // Re-map navn: hver gammel navngitt sentroide → nærmeste nye klynge.
    for (name, old_c) in &old_named {
        let Some((cid, sim)) = best_match(old_c, &new_centroids) else {
            break;
        };
        if sim >= threshold {
            conn.execute(
                "UPDATE persons SET name = ? WHERE cluster_id = ?",
                params![name, cid],
            )?;
            stats.names_remapped += 1;
        }
    }
    Ok(())
}

// --- Korrigering / kuratering (GUI-mutasjoner) --------------------------------
//
// Rene funksjoner API-laget wrapper (PUT /api/faces/{id}, POST/PUT/DELETE
// /api/persons…). Hver tar en connection, gjør én logisk endring og committer.
// Klynging respekterer resultatet: et frosset ansikt (locked=1) rører cluster()
// aldri; et tilordnet ansikt (cluster_id != NULL) hoppes over av inkrementell.

fn face_exists(conn: &Connection, face_id: i64) -> Result<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM faces WHERE id = ?", [face_id], |_| Ok(()))
        .optional()?
        .is_some())
}

fn person_exists(conn: &Connection, cluster_id: i64) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM persons WHERE cluster_id = ?",
            [cluster_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn require_face(conn: &Connection, face_id: i64) -> Result<()> {
    if face_exists(conn, face_id)? {
        Ok(())
    } else {
        Err(Error::Faces(format!("ansikt {face_id} finnes ikke")))
    }
}

fn require_person(conn: &Connection, cluster_id: i64) -> Result<()> {
    if person_exists(conn, cluster_id)? {
        Ok(())
    } else {
        Err(Error::Faces(format!("person {cluster_id} finnes ikke")))
    }
}

/// Flytt ett ansikt til en annen person, eller løsne det (`cluster_id = None`).
pub fn reassign_face(conn: &Connection, face_id: i64, cluster_id: Option<i64>) -> Result<()> {
    require_face(conn, face_id)?;
    if let Some(cid) = cluster_id {
        require_person(conn, cid)?;
    }
    conn.execute(
        "UPDATE faces SET cluster_id = ? WHERE id = ?",
        params![cluster_id, face_id],
    )?;
    Ok(())
}

/// Frys/ignorér eller tin opp ett ansikt.
///
/// `locked = true` + `cluster_id = None` = ignorer søppel-ansikt (statue/maleri):
/// [`cluster`] vil aldri re-tilordne det. `locked = true` på et tilordnet ansikt
/// fryser den manuelle tilordningen mot full re-klynging.
pub fn set_face_locked(conn: &Connection, face_id: i64, locked: bool) -> Result<()> {
    require_face(conn, face_id)?;
    conn.execute(
        "UPDATE faces SET locked = ? WHERE id = ?",
        params![i64::from(locked), face_id],
    )?;
    Ok(())
}

/// Opprett en ny person av et utvalg ansikter (splitt). Returnerer `cluster_id`.
pub fn create_person_from_faces(
    conn: &Connection,
    face_ids: &[i64],
    name: Option<&str>,
) -> Result<i64> {
    if face_ids.is_empty() {
        return Err(Error::Faces("face_ids er tom".into()));
    }
    for &fid in face_ids {
        require_face(conn, fid)?;
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute("INSERT INTO persons (name) VALUES (?)", [name])?;
    let cluster_id = tx.last_insert_rowid();
    for &fid in face_ids {
        tx.execute(
            "UPDATE faces SET cluster_id = ? WHERE id = ?",
            params![cluster_id, fid],
        )?;
    }
    tx.commit()?;
    Ok(cluster_id)
}

/// Slå sammen to personer: flytt alle ansikter til `into` og slett kilden.
pub fn merge_persons(
    conn: &Connection,
    source_cluster_id: i64,
    into_cluster_id: i64,
) -> Result<()> {
    if source_cluster_id == into_cluster_id {
        return Err(Error::Faces(
            "kan ikke slå sammen en person med seg selv".into(),
        ));
    }
    require_person(conn, source_cluster_id)?;
    require_person(conn, into_cluster_id)?;
    let tx = conn.unchecked_transaction()?;
    // Flytt ansiktene først, så peker ingen på kilden når den slettes.
    tx.execute(
        "UPDATE faces SET cluster_id = ? WHERE cluster_id = ?",
        params![into_cluster_id, source_cluster_id],
    )?;
    tx.execute(
        "DELETE FROM persons WHERE cluster_id = ?",
        [source_cluster_id],
    )?;
    tx.commit()?;
    Ok(())
}

/// Slett en person. Ansiktene løsnes (`cluster_id → NULL` via ON DELETE SET NULL).
pub fn delete_person(conn: &Connection, cluster_id: i64) -> Result<()> {
    require_person(conn, cluster_id)?;
    conn.execute("DELETE FROM persons WHERE cluster_id = ?", [cluster_id])?;
    Ok(())
}

/// Kryss bort en søppel-klynge permanent. Returnerer antall ignorerte ansikter.
///
/// Til forskjell fra [`delete_person`] (som bare løsner ansiktene, så de blir
/// re-klynget til samme person ved neste [`cluster`]): her **låses** alle ansiktene og
/// løsnes (`locked=1, cluster_id=NULL`). Et låst, uklynget ansikt rører [`cluster`]
/// aldri — så personen forsvinner fra lista og gjenoppstår aldri. Bruk for tilfeldig
/// søppel (statuer, malerier, halve ansikter).
pub fn ignore_person(conn: &Connection, cluster_id: i64) -> Result<u64> {
    require_person(conn, cluster_id)?;
    let tx = conn.unchecked_transaction()?;
    let n = tx.execute(
        "UPDATE faces SET locked = 1, cluster_id = NULL WHERE cluster_id = ?",
        [cluster_id],
    )?;
    tx.execute("DELETE FROM persons WHERE cluster_id = ?", [cluster_id])?;
    tx.commit()?;
    Ok(n as u64)
}

/// Navngi/omdøp en person (`name = None` fjerner navnet). Wrapper PUT /api/persons/{id}.
pub fn set_person_name(conn: &Connection, cluster_id: i64, name: Option<&str>) -> Result<()> {
    require_person(conn, cluster_id)?;
    conn.execute(
        "UPDATE persons SET name = ? WHERE cluster_id = ?",
        params![name, cluster_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.pragma_update(None, "foreign_keys", "ON").unwrap();
        db::init_db(&c).unwrap();
        c
    }

    /// Normalisert 512-vektor i retningen `dir` (akse, vekt), litt forskjøvet mot `jitter`.
    fn emb(dir: &[(usize, f32)], jitter: usize, amount: f32) -> Vec<u8> {
        let mut v = vec![0.0f32; EMBED_DIM];
        for &(axis, w) in dir {
            v[axis] = w;
        }
        v[jitter] += amount;
        linalg::normalize(&v)
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect()
    }

    fn add_image(c: &Connection, hash: &str) -> i64 {
        c.execute(
            "INSERT INTO images (content_hash, width, height) VALUES (?, 2048, 1536)",
            [hash],
        )
        .unwrap();
        c.last_insert_rowid()
    }

    fn add_face(c: &Connection, image_id: i64, embedding: Vec<u8>) -> i64 {
        c.execute(
            "INSERT INTO faces (image_id, bbox, embedding) VALUES (?, '[1, 2, 3, 4]', ?)",
            params![image_id, embedding],
        )
        .unwrap();
        c.last_insert_rowid()
    }

    const A: &[(usize, f32)] = &[(0, 1.0)];
    /// cos(A, B) ≈ 0,45 (under terskelen 0,5), men A og B ligger nærmere hverandre
    /// (avstand ≈ 1,05) enn det fjerne ansiktet ligger fra begge (≈ 1,41).
    const B: &[(usize, f32)] = &[(0, 1.0), (1, 2.0)];

    /// To personer (A og B) à 4 ansikter + ett fjernt ansikt.
    fn people(c: &Connection) -> (Vec<i64>, Vec<i64>, i64) {
        let img = add_image(c, "h1");
        let a = (0..4)
            .map(|k| add_face(c, img, emb(A, 10 + k, 0.05 + 0.01 * k as f32)))
            .collect();
        let b = (0..4)
            .map(|k| add_face(c, img, emb(B, 20 + k, 0.1 + 0.02 * k as f32)))
            .collect();
        let stray = add_face(c, img, emb(&[(300, 1.0)], 301, 0.3));
        (a, b, stray)
    }

    fn cluster_of(c: &Connection, face_id: i64) -> Option<i64> {
        c.query_row(
            "SELECT cluster_id FROM faces WHERE id = ?",
            [face_id],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn person_count(c: &Connection) -> i64 {
        c.query_row("SELECT COUNT(*) FROM persons", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn inkrementell_foder_og_tilordner_nye_ansikter() {
        let c = conn();
        let (a, b, stray) = people(&c);
        let s = cluster(&c, false, None).unwrap();
        assert_eq!(
            s.summary(),
            "inkrementell: ansikter=9 klynger=2 tilordnet=0 nye=2 utilordnet=1 navn-remappet=0"
        );
        let (pa, pb) = (cluster_of(&c, a[0]).unwrap(), cluster_of(&c, b[0]).unwrap());
        assert_ne!(pa, pb);
        assert!(a.iter().all(|&f| cluster_of(&c, f) == Some(pa)));
        assert_eq!(cluster_of(&c, stray), None);

        // Et nytt ansikt av A tilordnes eksisterende person; ingenting nytt fødes.
        let new = add_face(&c, 1, emb(A, 40, 0.08));
        let s = cluster(&c, false, None).unwrap();
        assert_eq!((s.assigned_incremental, s.new_clusters), (1, 0));
        assert_eq!(cluster_of(&c, new), Some(pa));
        // Over terskelen 0,999 tilordnes det ikke.
        c.execute("UPDATE faces SET cluster_id = NULL WHERE id = ?", [new])
            .unwrap();
        let s = cluster(&c, false, Some(0.999)).unwrap();
        assert_eq!(s.assigned_incremental, 0);
        assert_eq!(cluster_of(&c, new), None);
    }

    #[test]
    fn full_gjenbruker_navn_og_respekterer_laaste() {
        let c = conn();
        let (a, b, stray) = people(&c);
        cluster(&c, false, None).unwrap();
        let pa = cluster_of(&c, a[0]).unwrap();
        let pb = cluster_of(&c, b[0]).unwrap();
        set_person_name(&c, pa, Some("Bjørn")).unwrap();
        // Lås ett B-ansikt hos B og ignorer det fjerne ansiktet.
        set_face_locked(&c, b[3], true).unwrap();
        set_face_locked(&c, stray, true).unwrap();

        let s = cluster(&c, true, None).unwrap();
        assert!(s.full && s.names_remapped == 1, "{}", s.summary());
        let named: i64 = c
            .query_row(
                "SELECT cluster_id FROM persons WHERE name = 'Bjørn'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(a.iter().all(|&f| cluster_of(&c, f) == Some(named)));
        assert_eq!(cluster_of(&c, b[3]), Some(pb), "låst tilordning består");
        assert!(
            person_exists(&c, pb).unwrap(),
            "person med låst medlem overlever"
        );
        assert_eq!(cluster_of(&c, stray), None, "ignorert ansikt røres aldri");
        let s = cluster(&c, false, None).unwrap();
        assert_eq!(cluster_of(&c, stray), None);
        assert_eq!(s.unassigned, 1);
    }

    #[test]
    fn kuratering_endrer_og_klyngingen_respekterer() {
        let c = conn();
        let (a, b, _) = people(&c);
        cluster(&c, false, None).unwrap();
        let (pa, pb) = (cluster_of(&c, a[0]).unwrap(), cluster_of(&c, b[0]).unwrap());

        reassign_face(&c, a[0], Some(pb)).unwrap();
        assert_eq!(cluster_of(&c, a[0]), Some(pb));
        reassign_face(&c, a[0], None).unwrap();
        assert_eq!(cluster_of(&c, a[0]), None);

        let split = create_person_from_faces(&c, &[a[1], a[2]], Some("Splitt")).unwrap();
        assert_eq!(cluster_of(&c, a[1]), Some(split));
        let name: Option<String> = c
            .query_row(
                "SELECT name FROM persons WHERE cluster_id = ?",
                [split],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(name.as_deref(), Some("Splitt"));

        merge_persons(&c, split, pa).unwrap();
        assert!(!person_exists(&c, split).unwrap());
        assert_eq!(cluster_of(&c, a[1]), Some(pa));

        // delete løsner (ON DELETE SET NULL). Inkrementelt er B alene i NULL-poolen én
        // klynge ⇒ støy (allow_single_cluster=False); full re-klynging føder den igjen.
        delete_person(&c, pb).unwrap();
        assert!(b.iter().all(|&f| cluster_of(&c, f).is_none()));
        assert_eq!(cluster(&c, false, None).unwrap().new_clusters, 0);
        assert_eq!(cluster(&c, true, None).unwrap().new_clusters, 2);
        let pb2 = cluster_of(&c, b[0]).unwrap();
        let pa2 = cluster_of(&c, a[0]).unwrap();
        set_person_name(&c, pa2, Some("Bjørn")).unwrap();
        set_person_name(&c, pa2, None).unwrap();

        // ignore låser og løsner; personen gjenoppstår aldri.
        assert_eq!(ignore_person(&c, pb2).unwrap(), 4);
        let locked: i64 = c
            .query_row(
                "SELECT SUM(locked) FROM faces WHERE cluster_id IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(locked, 4);
        let before = person_count(&c);
        cluster(&c, false, None).unwrap();
        cluster(&c, true, None).unwrap();
        assert!(b.iter().all(|&f| cluster_of(&c, f).is_none()));
        assert!(person_count(&c) <= before);
        // Tint opp ett ignorert ansikt: da er det med i NULL-poolen igjen.
        set_face_locked(&c, b[0], false).unwrap();
        let locked: i64 = c
            .query_row("SELECT locked FROM faces WHERE id = ?", [b[0]], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(locked, 0);
    }

    #[test]
    fn kuratering_feiler_med_pythons_meldinger() {
        let c = conn();
        let (a, _, _) = people(&c);
        cluster(&c, false, None).unwrap();
        let pa = cluster_of(&c, a[0]).unwrap();
        let msg = |r: Result<()>| r.unwrap_err().to_string();
        assert_eq!(msg(reassign_face(&c, 999, None)), "ansikt 999 finnes ikke");
        assert_eq!(
            msg(reassign_face(&c, a[0], Some(77))),
            "person 77 finnes ikke"
        );
        assert_eq!(
            msg(set_face_locked(&c, 999, true)),
            "ansikt 999 finnes ikke"
        );
        assert_eq!(
            create_person_from_faces(&c, &[], None)
                .unwrap_err()
                .to_string(),
            "face_ids er tom"
        );
        assert_eq!(
            create_person_from_faces(&c, &[a[0], 999], None)
                .unwrap_err()
                .to_string(),
            "ansikt 999 finnes ikke"
        );
        assert_eq!(
            msg(merge_persons(&c, pa, pa)),
            "kan ikke slå sammen en person med seg selv"
        );
        assert_eq!(msg(merge_persons(&c, 77, pa)), "person 77 finnes ikke");
        assert_eq!(msg(merge_persons(&c, pa, 77)), "person 77 finnes ikke");
        assert_eq!(msg(delete_person(&c, 77)), "person 77 finnes ikke");
        assert_eq!(
            ignore_person(&c, 77).unwrap_err().to_string(),
            "person 77 finnes ikke"
        );
        assert_eq!(
            msg(set_person_name(&c, 77, Some("x"))),
            "person 77 finnes ikke"
        );
        assert!(matches!(delete_person(&c, 77), Err(Error::Faces(_))));
    }

    #[test]
    fn klynging_uten_ansikter_og_ugyldige_innstillinger() {
        let c = conn();
        let s = cluster(&c, true, None).unwrap();
        assert_eq!(
            s.summary(),
            "full re-klynging: ansikter=0 klynger=0 tilordnet=0 nye=0 utilordnet=0 navn-remappet=0"
        );
        people(&c);
        settings::set_setting(&c, "face_min_cluster_size", "1").unwrap();
        assert!(
            cluster(&c, false, None).is_err(),
            "HDBSCAN krever min_cluster_size ≥ 2"
        );
        assert_eq!(person_count(&c), 0, "feil ruller tilbake");
        settings::set_setting(&c, "face_min_cluster_size", "").unwrap();
        settings::set_setting(&c, "face_match_threshold", " 0.5 ").unwrap();
        assert_eq!(cluster(&c, false, None).unwrap().new_clusters, 2);
    }

    #[test]
    fn bbox_skaleres_som_python() {
        assert_eq!(py_round(2.5), 2);
        assert_eq!(py_round(3.5), 4);
        assert_eq!(py_round(-2.5), -2);
        assert_eq!(scale(Some(3024), 1024), (3024.0f64 / 1024.0) as f32);
        assert_eq!(scale(None, 1024), 1.0);
        assert_eq!(scale(Some(0), 1024), 1.0);
    }

    #[test]
    fn detect_uten_ventende_bilder_trenger_ingen_modeller() {
        let c = conn();
        let dir = tempfile::tempdir().unwrap();
        let s = detect(&c, &Storage::new(dir.path()), None, false, None).unwrap();
        assert_eq!(s.summary(), "pending=0 done=0 ansikter=0 feil=0");
    }

    /// Ende til ende med ekte modeller (hoppes over når de ikke finnes, f.eks. i CI).
    #[test]
    fn detect_skriver_ansikter_og_feiler_mykt() {
        if resources::face_model_dir().is_err() {
            eprintln!("hopper over: ansiktsmodellene finnes ikke");
            return;
        }
        let c = conn();
        let dir = tempfile::tempdir().unwrap();
        let store = Storage::new(dir.path());
        store.ensure_dirs().unwrap();
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/astronaut.jpg");
        std::fs::copy(&fixture, store.derived_path("god", "1024")).unwrap();
        std::fs::write(store.derived_path("korrupt", "1024"), b"ikke jpeg").unwrap();
        for hash in ["god", "korrupt", "venter"] {
            let id = add_image(&c, hash);
            let derive = if hash == "venter" { "pending" } else { "done" };
            c.execute(
                "INSERT INTO processing_state (image_id, stage, status) VALUES (?, 'derive', ?), (?, 'faces', 'pending')",
                params![id, derive, id],
            )
            .unwrap();
            c.execute(
                "INSERT INTO derived (image_id, size, path) VALUES (?, '1024', ?)",
                params![id, format!("{hash}_1024.jpg")],
            )
            .unwrap();
        }
        let s = detect(&c, &store, Some(2), false, None).unwrap();
        assert_eq!((s.pending, s.done, s.errors), (2, 1, 1), "{}", s.summary());
        assert!(s.faces_found >= 1, "{}", s.summary());
        let (bbox, len): (String, i64) = c
            .query_row(
                "SELECT bbox, length(embedding) FROM faces WHERE image_id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(len, 2048);
        let parsed: Vec<i64> = serde_json::from_str(&bbox).unwrap();
        assert!(bbox.contains(", ") && parsed.len() == 4, "{bbox}");
        let state = |id: i64| -> (String, Option<String>) {
            c.query_row(
                "SELECT status, error FROM processing_state WHERE image_id = ? AND stage = 'faces'",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
        };
        assert_eq!(state(1), ("done".into(), None));
        assert_eq!(state(2).0, "error");
        assert!(state(2).1.unwrap().starts_with("kunne ikke lese derivat"));
        assert_eq!(state(3).0, "pending", "derive ikke ferdig ⇒ ikke med");

        // Idempotent; --retry-errors tar feilede med igjen; source_id filtrerer.
        assert_eq!(detect(&c, &store, None, false, None).unwrap().pending, 0);
        assert_eq!(detect(&c, &store, None, true, Some(99)).unwrap().pending, 0);
        assert_eq!(detect(&c, &store, None, true, None).unwrap().errors, 1);
    }
}
