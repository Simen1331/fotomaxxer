"""Pipeline-jobber: «Oppdater biblioteket»-kjede, enkelt-steg (Avansert) + status.

Alle tunge/skrivende operasjoner går via den delte jobb-kjøreren (``api.jobs``):
**én** jobb om gangen, hver i egen tråd med sin **egen** SQLite-connection.
``/jobs/update`` kjeder scan→derive→tag→faces-detect→cluster og oppdaterer
``current.step`` per ledd. Enkelt-steg-endepunktene bruker samme kjører.

**Kostnadsgate:** GUI sjekker ai-pending via ``/progress`` og bekrefter med bruker
før ``update``/``tag`` sendes — API-et kjører derfor ``tag`` med ``assume_yes=True``.
Mangler OpenRouter-nøkkel: kjeden hopper ``tag`` med en melding i ``current.message``
og fortsetter med ansikter.
"""

from __future__ import annotations

import sqlite3

from fastapi import APIRouter, Depends

from api.deps import get_conn, make_storage
from api.jobs import JobBusy, JobRunner, runner
from api.schemas import (
    CostSummary,
    DeriveRequest,
    FacesClusterRequest,
    FacesDetectRequest,
    JobAccepted,
    JobCurrent,
    Progress,
    ScanRequest,
    StageProgress,
    TagRequest,
    UpdateRequest,
)
from fastapi import HTTPException

from fotomaxxer import db, sources as sources_mod
from fotomaxxer.derive import derive as derive_images
from fotomaxxer.exif import ExifReader
from fotomaxxer.faces import cluster as cluster_faces
from fotomaxxer.faces import detect as detect_faces
from fotomaxxer.geocode import Geocoder
from fotomaxxer.scan import scan_source
from fotomaxxer.tagging import TaggingError
from fotomaxxer.tagging import tag as tag_images

router = APIRouter(tags=["jobs"])


# --- Steg-implementasjoner (kjøres inne i en jobb, egen connection) --------


def _step_scan(jconn: sqlite3.Connection, run: JobRunner, source_id: int | None, all_: bool) -> None:
    run.update(step="scan")
    if source_id is not None:
        row = jconn.execute("SELECT * FROM sources WHERE id = ?", (source_id,)).fetchone()
        targets = [row] if row else []
    else:
        targets = sources_mod.list_sources(jconn)
    if not targets:
        return
    geocoder = Geocoder()

    def report(st) -> None:
        run.update(progress={"scan_seen": st.seen, "scan_new": st.new_images})

    with ExifReader() as exif:
        for row in targets:
            scan_source(jconn, row, exif, geocoder, on_progress=report)


def _step_derive(
    jconn: sqlite3.Connection, run: JobRunner, workers: int | None = None, source_id: int | None = None
) -> None:
    run.update(step="derive")
    derive_images(jconn, make_storage(), workers=workers, source_id=source_id)


def _step_tag(
    jconn: sqlite3.Connection, run: JobRunner, *, soft: bool = True, source_id: int | None = None
) -> None:
    """AI-merking. ``assume_yes=True`` — kostnadsgaten håndteres i GUI før kall.

    ``soft``: mangler nøkkel → hopp med melding (brukes i kjeden). Ellers reises feilen.
    ``source_id`` begrenser til én kilde (per-mappe-kjøring).
    """
    run.update(step="tag")
    try:
        tag_images(jconn, make_storage(), assume_yes=True, source_id=source_id)
    except TaggingError as e:
        if not soft:
            raise
        run.update(message=f"Hoppet over AI-merking: {e}")


def _step_faces_detect(
    jconn: sqlite3.Connection,
    run: JobRunner,
    *,
    retry_errors: bool = False,
    workers: int | None = None,
    source_id: int | None = None,
) -> None:
    run.update(step="faces-detect")
    detect_faces(
        jconn, make_storage(), workers=workers, retry_errors=retry_errors, source_id=source_id
    )


def _step_faces_cluster(jconn: sqlite3.Connection, run: JobRunner, *, full: bool = False) -> None:
    run.update(step="cluster")
    cluster_faces(jconn, full=full)


def _accept(kind: str, target) -> JobAccepted:
    try:
        job_id = runner.start(kind, target)
    except JobBusy:
        raise HTTPException(
            status_code=409,
            detail={"error": "busy", "message": "en jobb kjører allerede — vent til den er ferdig"},
        )
    return JobAccepted(job_id=job_id, kind=kind)


# --- Oppdater-kjeden -------------------------------------------------------


@router.post("/jobs/update", response_model=JobAccepted, status_code=202)
def update_library(body: UpdateRequest) -> JobAccepted:
    """scan → derive → tag → faces-detect → cluster på alle (eller én kilde)."""
    source_id = body.source_id

    def job(run: JobRunner) -> None:
        run.update(source_id=source_id)  # /progress?source_id kan da scope fremdriften
        jconn = db.connect(make_storage().db_path)
        try:
            _step_scan(jconn, run, source_id, body.all or source_id is None)
            _step_derive(jconn, run, source_id=source_id)
            _step_tag(jconn, run, soft=True, source_id=source_id)
            _step_faces_detect(jconn, run, source_id=source_id)
            _step_faces_cluster(jconn, run)
            if run.current().get("message") is None:
                run.update(message="Biblioteket er oppdatert.")
        finally:
            jconn.close()

    return _accept("update", job)


# --- Avansert: enkelt-steg -------------------------------------------------


@router.post("/jobs/scan", response_model=JobAccepted, status_code=202)
def job_scan(body: ScanRequest) -> JobAccepted:
    def job(run: JobRunner) -> None:
        run.update(source_id=body.source_id)
        jconn = db.connect(make_storage().db_path)
        try:
            _step_scan(jconn, run, body.source_id, body.all or body.source_id is None)
            run.update(message="Skanning ferdig.")
        finally:
            jconn.close()

    return _accept("scan", job)


@router.post("/jobs/derive", response_model=JobAccepted, status_code=202)
def job_derive(body: DeriveRequest) -> JobAccepted:
    def job(run: JobRunner) -> None:
        run.update(source_id=body.source_id)
        jconn = db.connect(make_storage().db_path)
        try:
            _step_derive(jconn, run, workers=body.workers, source_id=body.source_id)
            run.update(message="Arbeidskopier ferdig.")
        finally:
            jconn.close()

    return _accept("derive", job)


@router.post("/jobs/tag", response_model=JobAccepted, status_code=202)
def job_tag(body: TagRequest) -> JobAccepted:
    def job(run: JobRunner) -> None:
        run.update(source_id=body.source_id)
        jconn = db.connect(make_storage().db_path)
        try:
            run.update(step="tag")
            tag_images(
                jconn,
                make_storage(),
                limit=body.limit,
                workers=body.workers,
                retag=body.retag,
                assume_yes=True,
                source_id=body.source_id,
            )
            run.update(message="AI-merking ferdig.")
        finally:
            jconn.close()

    return _accept("tag", job)


@router.post("/jobs/faces-detect", response_model=JobAccepted, status_code=202)
def job_faces_detect(body: FacesDetectRequest) -> JobAccepted:
    def job(run: JobRunner) -> None:
        run.update(source_id=body.source_id)
        jconn = db.connect(make_storage().db_path)
        try:
            _step_faces_detect(
                jconn, run, retry_errors=body.retry_errors, workers=body.workers,
                source_id=body.source_id,
            )
            run.update(message="Ansikts-deteksjon ferdig.")
        finally:
            jconn.close()

    return _accept("faces-detect", job)


@router.post("/jobs/faces-cluster", response_model=JobAccepted, status_code=202)
def job_faces_cluster(body: FacesClusterRequest) -> JobAccepted:
    def job(run: JobRunner) -> None:
        jconn = db.connect(make_storage().db_path)
        try:
            _step_faces_cluster(jconn, run, full=body.full)
            run.update(message="Klynging ferdig.")
        finally:
            jconn.close()

    return _accept("faces-cluster", job)


# --- Status + fremdrift ----------------------------------------------------


@router.get("/jobs/current", response_model=JobCurrent)
def current() -> JobCurrent:
    s = runner.current()
    return JobCurrent(
        running=s.get("running", False),
        kind=s.get("kind"),
        step=s.get("step"),
        started_at=s.get("started_at"),
        message=s.get("message"),
        progress=s.get("progress", {}),
        source_id=s.get("source_id"),
    )


@router.get("/progress", response_model=Progress)
def progress(
    source_id: int | None = None, conn: sqlite3.Connection = Depends(get_conn)
) -> Progress:
    """Aggregat over ``processing_state`` (GROUP BY stage,status) + telle-tall.

    ``source_id`` scoper alt til bilder med en fil i den kilden — så kostnadsgaten
    og per-mappe-fremdriften reflekterer kun den mappa, ikke hele biblioteket.
    """
    # Felles bilde-filter: en korrelert IN-subquery når en kilde er valgt.
    img_in = ""
    params: list[object] = []
    if source_id is not None:
        img_in = " WHERE image_id IN (SELECT image_id FROM files WHERE source_id = ?)"
        params = [source_id]

    stages: dict[str, StageProgress] = {"derive": StageProgress(), "ai": StageProgress(), "faces": StageProgress()}
    for r in conn.execute(
        f"SELECT stage, status, COUNT(*) AS c FROM processing_state{img_in} GROUP BY stage, status",
        params,
    ):
        sp = stages.get(r["stage"])
        if sp is None:
            continue
        if r["status"] in ("pending", "done", "error"):
            setattr(sp, r["status"], r["c"])

    if source_id is None:
        images_total = conn.execute("SELECT COUNT(*) FROM images").fetchone()[0]
        faces_total = conn.execute("SELECT COUNT(*) FROM faces").fetchone()[0]
        persons_total = conn.execute("SELECT COUNT(*) FROM persons").fetchone()[0]
    else:
        images_total = conn.execute(
            "SELECT COUNT(DISTINCT image_id) FROM files WHERE source_id = ?", (source_id,)
        ).fetchone()[0]
        faces_total = conn.execute(f"SELECT COUNT(*) FROM faces{img_in}", params).fetchone()[0]
        persons_total = conn.execute(
            "SELECT COUNT(DISTINCT cluster_id) FROM faces "
            "WHERE cluster_id IS NOT NULL AND image_id IN "
            "(SELECT image_id FROM files WHERE source_id = ?)",
            (source_id,),
        ).fetchone()[0]

    return Progress(
        derive=stages["derive"],
        ai=stages["ai"],
        faces=stages["faces"],
        images_total=images_total,
        faces_total=faces_total,
        persons_total=persons_total,
    )


@router.get("/cost", response_model=CostSummary)
def cost(
    source_id: int | None = None, conn: sqlite3.Connection = Depends(get_conn)
) -> CostSummary:
    """Faktisk AI-kost (USD) fra ``ai_tags.cost``: total + snitt/bilde + antall.

    ``source_id`` scoper til én kilde. Snittet er estimeringsgrunnlag for
    kostnadsgaten; bilder uten kjent kost (eldre, eller modell uten usage.cost)
    teller ikke med."""
    where = "WHERE cost IS NOT NULL"
    params: list[object] = []
    if source_id is not None:
        where += " AND image_id IN (SELECT image_id FROM files WHERE source_id = ?)"
        params.append(source_id)
    row = conn.execute(
        f"SELECT COALESCE(SUM(cost), 0.0) AS total, COUNT(*) AS n FROM ai_tags {where}", params
    ).fetchone()
    total = float(row["total"])
    n = int(row["n"])
    return CostSummary(total=total, avg=(total / n if n else 0.0), count=n)
