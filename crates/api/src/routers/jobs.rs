//! Pipeline-jobber: «Oppdater biblioteket»-kjeden, enkelt-steg (Avansert) og status.
//!
//! Motparten til `api/routers/jobs.py`. Alle tunge/skrivende operasjoner går via den
//! delte jobbkjøreren ([`crate::jobs`]): **én** jobb om gangen, hver i egen tråd med sin
//! **egen** SQLite-tilkobling. `/jobs/update` kjeder scan → derive → tag →
//! faces-detect → cluster og oppdaterer `current.step` per ledd. Stegene kaller
//! `fotomaxxer_core` direkte (ingen subprocess).
//!
//! **Kostnadsgrense:** GUI sjekker ai-pending via `/progress` og bekrefter med brukeren
//! før `update`/`tag` sendes — API-et kjører derfor `tag` med `assume_yes`. Mangler
//! OpenRouter-nøkkelen, hopper kjeden over `tag` med en melding i `current.message` og
//! fortsetter med ansikter.

use std::collections::BTreeMap;

use axum::extract::State;
use axum::http::StatusCode;
use fotomaxxer_core::geocode::Geocoder;
use fotomaxxer_core::{Error, cancel, db, derive, faces, scan, sources, tagging};
use rusqlite::Connection;
use serde_json::{Map, Value};

use super::Routes;
use crate::error::{ApiError, ApiResult, JsonResponse};
use crate::jobs::{JobFailure, JobHandle, MAX_WORKERS};
use crate::schemas::{
    CostSummary, DeriveRequest, FacesClusterRequest, FacesDetectRequest, JobAccepted, JobCurrent,
    Progress, ScanRequest, SourceIdQuery, StageProgress, TagRequest, UpdateRequest,
};
use crate::state::{AppState, blocking, make_storage};
use crate::validate::{DbReq, Req};

pub fn routes() -> Routes {
    Routes::new()
        .post("/jobs/update", update_library)
        .post("/jobs/scan", job_scan)
        .post("/jobs/derive", job_derive)
        .post("/jobs/tag", job_tag)
        .post("/jobs/faces-detect", job_faces_detect)
        .post("/jobs/faces-cluster", job_faces_cluster)
        .get("/jobs/current", current)
        .get("/progress", progress)
        .get("/cost", cost)
}

pub type JobResult = Result<(), JobFailure>;

/// `202 Accepted` med `JobAccepted`.
pub type Accepted = (StatusCode, JsonResponse<JobAccepted>);

// --- Steg-implementasjoner (kjøres inne i en jobb, egen tilkobling) ------------------

/// Jobbens egen tilkobling (`db.connect(make_storage().db_path)`, uten `init_db`).
pub fn job_conn() -> Result<Connection, JobFailure> {
    Ok(db::connect(&make_storage().db_path())?)
}

/// `ProcessPoolExecutor(max_workers=n)` godtar ikke `n <= 0`, men Python lager poolen bare
/// når det finnes arbeid. `has_work` sjekker det med samme utvalg som steget, så en
/// ugyldig `workers` feiler (uten å endre databasen) nøyaktig når Python ville feilet.
fn workers_arg(
    workers: Option<i64>,
    has_work: impl FnOnce() -> rusqlite::Result<bool>,
) -> Result<Option<usize>, JobFailure> {
    match workers {
        Some(n) if n <= 0 => {
            if has_work().map_err(|e| JobFailure::from(Error::Db(e)))? {
                Err(JobFailure::new("ValueError", MAX_WORKERS))
            } else {
                Ok(None)
            }
        }
        Some(n) => Ok(Some(n as usize)),
        None => Ok(None),
    }
}

/// Finnes et pending derive-bilde med lesbar kildefil (Pythons `jobs`-liste)?
fn derive_has_work(conn: &Connection, source_id: Option<i64>) -> rusqlite::Result<bool> {
    let mut sql = String::from(
        "SELECT (SELECT f.path FROM files f WHERE f.image_id = i.id LIMIT 1) AS src
         FROM images i JOIN processing_state ps ON ps.image_id = i.id
         WHERE ps.stage = 'derive' AND ps.status = 'pending'",
    );
    let params: Vec<i64> = source_id.into_iter().collect();
    if source_id.is_some() {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM files f WHERE f.image_id = i.id AND f.source_id = ?)",
        );
    }
    let mut stmt = conn.prepare(&sql)?;
    let srcs = stmt.query_map(rusqlite::params_from_iter(params), |r| {
        r.get::<_, Option<String>>(0)
    })?;
    for src in srcs {
        if src?.is_some_and(|p| std::path::Path::new(&p).exists()) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Finnes et bilde `faces detect` ville sendt til prosesspoolen?
fn detect_has_work(
    conn: &Connection,
    retry_errors: bool,
    source_id: Option<i64>,
) -> rusqlite::Result<bool> {
    let statuses = if retry_errors {
        "'pending', 'error'"
    } else {
        "'pending'"
    };
    let mut sql = format!(
        "SELECT 1 FROM images i
         JOIN processing_state pf ON pf.image_id = i.id AND pf.stage = 'faces'
                                 AND pf.status IN ({statuses})
         JOIN processing_state pd ON pd.image_id = i.id AND pd.stage = 'derive' AND pd.status = 'done'
         JOIN derived d           ON d.image_id  = i.id AND d.size = '1024'
         WHERE 1=1"
    );
    let params: Vec<i64> = source_id.into_iter().collect();
    if source_id.is_some() {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM files f WHERE f.image_id = i.id AND f.source_id = ?)",
        );
    }
    sql.push_str(" LIMIT 1");
    conn.query_row(&sql, rusqlite::params_from_iter(params), |_| Ok(()))
        .map(|()| true)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(false),
            e => Err(e),
        })
}

fn step_scan(jconn: &Connection, run: &JobHandle, source_id: Option<i64>) -> JobResult {
    cancel::check()?;
    run.set_step("scan");
    let targets = match source_id {
        Some(id) => sources::get_by_id(jconn, id)?.into_iter().collect(),
        None => sources::list_sources(jconn)?,
    };
    if targets.is_empty() {
        return Ok(());
    }
    let mut geocoder = Geocoder::default();
    let mut report = |st: &scan::ScanStats| {
        let mut p = Map::new();
        p.insert("scan_seen".into(), Value::from(st.seen));
        p.insert("scan_new".into(), Value::from(st.new_images));
        run.set_progress(p);
    };
    for row in &targets {
        scan::scan_source(jconn, row, &mut geocoder, Some(&mut report))?;
    }
    Ok(())
}

fn step_derive(
    jconn: &Connection,
    run: &JobHandle,
    workers: Option<i64>,
    source_id: Option<i64>,
) -> JobResult {
    cancel::check()?;
    run.set_step("derive");
    let workers = workers_arg(workers, || derive_has_work(jconn, source_id))?;
    derive::derive(jconn, &make_storage(), workers, source_id, None)?;
    Ok(())
}

/// AI-merking med `assume_yes` — kostnadsgrensen håndteres i GUI før kallet.
/// `soft`: mangler nøkkel (o.l. `TaggingError`) → hopp over med melding (brukes i kjeden).
fn step_tag(
    jconn: &Connection,
    run: &JobHandle,
    soft: bool,
    opts: tagging::TagOptions,
) -> JobResult {
    cancel::check()?;
    run.set_step("tag");
    match tagging::tag(jconn, &make_storage(), &opts, &mut |_| true) {
        Ok(_) => Ok(()),
        Err(Error::Tagging(m)) if soft && m != MAX_WORKERS => {
            run.set_message(format!("Hoppet over AI-merking: {m}"));
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

fn step_faces_detect(
    jconn: &Connection,
    run: &JobHandle,
    retry_errors: bool,
    workers: Option<i64>,
    source_id: Option<i64>,
) -> JobResult {
    cancel::check()?;
    run.set_step("faces-detect");
    let workers = workers_arg(workers, || detect_has_work(jconn, retry_errors, source_id))?;
    faces::detect(jconn, &make_storage(), workers, retry_errors, source_id)?;
    Ok(())
}

fn step_faces_cluster(jconn: &Connection, run: &JobHandle, full: bool) -> JobResult {
    cancel::check()?;
    run.set_step("cluster");
    faces::cluster(jconn, full, None)?;
    Ok(())
}

/// Start jobben (`202` + `JobAccepted`), eller 409 «busy» som `_accept` i Python.
/// Brukes også av eksport (`api/routers/export.py`).
pub fn accept<F>(state: &AppState, kind: &str, target: F) -> ApiResult<Accepted>
where
    F: FnOnce(&JobHandle) -> JobResult + Send + 'static,
{
    let job_id = state.runner().start(kind, target).map_err(|_| {
        ApiError::http(
            409,
            "busy",
            "en jobb kjører allerede — vent til den er ferdig",
        )
    })?;
    Ok((
        StatusCode::ACCEPTED,
        JsonResponse(JobAccepted {
            job_id,
            kind: kind.to_string(),
        }),
    ))
}

// --- Oppdater-kjeden -------------------------------------------------------------------

/// scan → derive → tag → faces-detect → cluster på alle (eller én kilde).
async fn update_library(
    State(state): State<AppState>,
    req: Req<(), (), UpdateRequest>,
) -> ApiResult<Accepted> {
    let source_id = req.body.source_id;
    accept(&state, "update", move |run| {
        run.set_source_id(source_id); // /progress?source_id kan da avgrense fremdriften
        let jconn = job_conn()?;
        step_scan(&jconn, run, source_id)?;
        step_derive(&jconn, run, None, source_id)?;
        let opts = tagging::TagOptions {
            assume_yes: true,
            source_id,
            ..Default::default()
        };
        step_tag(&jconn, run, true, opts)?;
        step_faces_detect(&jconn, run, false, None, source_id)?;
        step_faces_cluster(&jconn, run, false)?;
        if run.current().message.is_none() {
            run.set_message("Biblioteket er oppdatert.");
        }
        Ok(())
    })
}

// --- Avansert: enkelt-steg ----------------------------------------------------------------

async fn job_scan(
    State(state): State<AppState>,
    req: Req<(), (), ScanRequest>,
) -> ApiResult<Accepted> {
    let source_id = req.body.source_id;
    accept(&state, "scan", move |run| {
        run.set_source_id(source_id);
        let jconn = job_conn()?;
        step_scan(&jconn, run, source_id)?;
        run.set_message("Skanning ferdig.");
        Ok(())
    })
}

async fn job_derive(
    State(state): State<AppState>,
    req: Req<(), (), DeriveRequest>,
) -> ApiResult<Accepted> {
    let body = req.body;
    accept(&state, "derive", move |run| {
        run.set_source_id(body.source_id);
        let jconn = job_conn()?;
        step_derive(&jconn, run, body.workers, body.source_id)?;
        run.set_message("Arbeidskopier ferdig.");
        Ok(())
    })
}

async fn job_tag(
    State(state): State<AppState>,
    req: Req<(), (), TagRequest>,
) -> ApiResult<Accepted> {
    let body = req.body;
    accept(&state, "tag", move |run| {
        run.set_source_id(body.source_id);
        let jconn = job_conn()?;
        let opts = tagging::TagOptions {
            limit: body.limit,
            workers: body.workers,
            retag: body.retag,
            assume_yes: true,
            source_id: body.source_id,
        };
        step_tag(&jconn, run, false, opts)?;
        run.set_message("AI-merking ferdig.");
        Ok(())
    })
}

async fn job_faces_detect(
    State(state): State<AppState>,
    req: Req<(), (), FacesDetectRequest>,
) -> ApiResult<Accepted> {
    let body = req.body;
    accept(&state, "faces-detect", move |run| {
        run.set_source_id(body.source_id);
        let jconn = job_conn()?;
        step_faces_detect(&jconn, run, body.retry_errors, body.workers, body.source_id)?;
        run.set_message("Ansikts-deteksjon ferdig.");
        Ok(())
    })
}

async fn job_faces_cluster(
    State(state): State<AppState>,
    req: Req<(), (), FacesClusterRequest>,
) -> ApiResult<Accepted> {
    let full = req.body.full;
    accept(&state, "faces-cluster", move |run| {
        let jconn = job_conn()?;
        step_faces_cluster(&jconn, run, full)?;
        run.set_message("Klynging ferdig.");
        Ok(())
    })
}

// --- Status + fremdrift --------------------------------------------------------------------

async fn current(State(state): State<AppState>) -> JsonResponse<JobCurrent> {
    let s = state.runner().current();
    JsonResponse(JobCurrent {
        running: s.running,
        kind: s.kind,
        step: s.step,
        started_at: s.started_at,
        message: s.message,
        progress: s.progress,
        source_id: s.source_id,
    })
}

fn count(conn: &Connection, sql: &str, params: &[i64]) -> rusqlite::Result<i64> {
    conn.query_row(sql, rusqlite::params_from_iter(params), |r| r.get(0))
}

/// Aggregat over `processing_state` (GROUP BY stage, status) + telletall.
///
/// `source_id` avgrenser alt til bilder med en fil i den kilden — så kostnadsgaten og
/// per-mappe-fremdriften reflekterer kun den mappa, ikke hele biblioteket.
async fn progress(req: DbReq<(), SourceIdQuery>) -> ApiResult<JsonResponse<Progress>> {
    let DbReq { conn, query, .. } = req;
    blocking(move || {
        let source_id = query.source_id;
        let (img_in, params): (&str, Vec<i64>) = match source_id {
            Some(id) => (
                " WHERE image_id IN (SELECT image_id FROM files WHERE source_id = ?)",
                vec![id],
            ),
            None => ("", vec![]),
        };
        let mut stages: BTreeMap<&str, StageProgress> = BTreeMap::new();
        for stage in ["derive", "ai", "faces"] {
            stages.insert(stage, StageProgress::default());
        }
        let mut stmt = conn.prepare(&format!(
            "SELECT stage, status, COUNT(*) AS c FROM processing_state{img_in} GROUP BY stage, status"
        ))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(&params), |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (stage, status, c) = row?;
            let Some(sp) = stage.as_deref().and_then(|s| stages.get_mut(s)) else {
                continue;
            };
            match status.as_deref() {
                Some("pending") => sp.pending = c,
                Some("done") => sp.done = c,
                Some("error") => sp.error = c,
                _ => {}
            }
        }
        let (images_total, faces_total, persons_total) = match source_id {
            None => (
                count(&conn, "SELECT COUNT(*) FROM images", &[])?,
                count(&conn, "SELECT COUNT(*) FROM faces", &[])?,
                count(&conn, "SELECT COUNT(*) FROM persons", &[])?,
            ),
            Some(id) => (
                count(
                    &conn,
                    "SELECT COUNT(DISTINCT image_id) FROM files WHERE source_id = ?",
                    &[id],
                )?,
                count(&conn, &format!("SELECT COUNT(*) FROM faces{img_in}"), &params)?,
                count(
                    &conn,
                    "SELECT COUNT(DISTINCT cluster_id) FROM faces \
                     WHERE cluster_id IS NOT NULL AND image_id IN \
                     (SELECT image_id FROM files WHERE source_id = ?)",
                    &[id],
                )?,
            ),
        };
        let mut take = |k: &str| stages.remove(k).unwrap_or_default();
        Ok(JsonResponse(Progress {
            derive: take("derive"),
            ai: take("ai"),
            faces: take("faces"),
            images_total,
            faces_total,
            persons_total,
            confirm_threshold: 200,
        }))
    })
    .await
}

/// Faktisk AI-kost (USD) fra `ai_tags.cost`: total + snitt per bilde + antall.
///
/// Snittet er estimeringsgrunnlag for kostnadsgaten; bilder uten kjent kost (eldre,
/// eller modell uten `usage.cost`) teller ikke med.
async fn cost(req: DbReq<(), SourceIdQuery>) -> ApiResult<JsonResponse<CostSummary>> {
    let DbReq { conn, query, .. } = req;
    blocking(move || {
        let mut sql = String::from(
            "SELECT COALESCE(SUM(cost), 0.0) AS total, COUNT(*) AS n FROM ai_tags WHERE cost IS NOT NULL",
        );
        let mut params = Vec::new();
        if let Some(id) = query.source_id {
            sql.push_str(" AND image_id IN (SELECT image_id FROM files WHERE source_id = ?)");
            params.push(id);
        }
        let (total, n): (f64, i64) =
            conn.query_row(&sql, rusqlite::params_from_iter(params), |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
        Ok(JsonResponse(CostSummary {
            total,
            avg: if n != 0 { total / n as f64 } else { 0.0 },
            count: n,
        }))
    })
    .await
}
