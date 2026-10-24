"""Pydantic request/response-modeller — den autoritative formen på API-svar.

Dette er single-source for response-former; frontend-typene i
``web/src/api/client.ts`` speiler disse. Hold dem i synk.
"""

from __future__ import annotations

from pydantic import BaseModel


# --- Bilder / browse -------------------------------------------------------


class ImageListItem(BaseModel):
    id: int
    hash: str
    thumb_url: str
    taken_at: str | None = None
    year: int | None = None
    season: str | None = None
    place_name: str | None = None
    person_count: int = 0
    tag_count: int = 0
    excluded: bool = False


class ImageList(BaseModel):
    items: list[ImageListItem]
    total: int


class ImagePatch(BaseModel):
    excluded: bool


class ImagePatchResult(BaseModel):
    id: int
    excluded: bool


class ImageIds(BaseModel):
    ids: list[int]


class FaceOut(BaseModel):
    id: int
    bbox: list[int]
    cluster_id: int | None = None
    person_name: str | None = None
    locked: bool = False


class AiOut(BaseModel):
    scene: str | None = None
    event_type: str | None = None
    season_ai: str | None = None
    indoor_outdoor: str | None = None
    person_count: int | None = None
    description: str | None = None
    provider: str | None = None


class FileOut(BaseModel):
    path: str
    source_id: int


class ImageDetail(BaseModel):
    id: int
    hash: str
    width: int | None = None
    height: int | None = None
    taken_at: str | None = None
    year: int | None = None
    season: str | None = None
    place_name: str | None = None
    camera_make: str | None = None
    camera_model: str | None = None
    gps_lat: float | None = None
    gps_lon: float | None = None
    excluded: bool = False
    image_url: str
    ai: AiOut | None = None
    tags: list[str] = []
    faces: list[FaceOut] = []
    files: list[FileOut] = []


# --- Facets ----------------------------------------------------------------


class FacetCount(BaseModel):
    name: str
    count: int
    category: str | None = None  # tagging-profil-kategori; None = ukategorisert


class FacetPerson(BaseModel):
    cluster_id: int
    name: str | None = None
    count: int


class Facets(BaseModel):
    years: list[int] = []
    seasons: list[str] = []
    places: list[str] = []
    events: list[str] = []
    tags: list[FacetCount] = []
    persons: list[FacetPerson] = []


# --- Album -----------------------------------------------------------------


class AlbumSummary(BaseModel):
    id: int
    name: str
    item_count: int
    created_at: str


class AlbumList(BaseModel):
    items: list[AlbumSummary]


class AlbumItem(BaseModel):
    image_id: int
    hash: str
    thumb_url: str
    position: int


class AlbumDetail(BaseModel):
    id: int
    name: str
    items: list[AlbumItem]


class AlbumCreate(BaseModel):
    name: str


class AlbumAddItems(BaseModel):
    image_ids: list[int]


class AlbumReorder(BaseModel):
    image_ids: list[int]


# --- Personer / ansikter ---------------------------------------------------
#
# `width`/`height` er originalbildets dimensjoner — frontend trenger dem for å
# CSS-croppe ansiktet (bbox er i originalkoordinater) mot 1024-derivatet.


class PersonCover(BaseModel):
    image_id: int
    hash: str
    bbox: list[int]
    width: int | None = None
    height: int | None = None


class PersonSummary(BaseModel):
    cluster_id: int
    name: str | None = None
    face_count: int
    cover: PersonCover | None = None


class PersonList(BaseModel):
    items: list[PersonSummary]


class PersonFaceItem(BaseModel):
    face_id: int
    image_id: int
    hash: str
    bbox: list[int]
    locked: bool = False
    width: int | None = None
    height: int | None = None


class PersonFaceList(BaseModel):
    items: list[PersonFaceItem]


class PersonRename(BaseModel):
    name: str | None = None


class PersonRenameResult(BaseModel):
    cluster_id: int
    name: str | None = None


class PersonCreate(BaseModel):
    name: str | None = None
    face_ids: list[int]


class PersonCreateResult(BaseModel):
    cluster_id: int


class PersonMerge(BaseModel):
    into_cluster_id: int


class FacePatch(BaseModel):
    cluster_id: int | None = None
    locked: bool | None = None


class FacePatchResult(BaseModel):
    id: int
    cluster_id: int | None = None
    locked: bool = False


# --- Jobber / eksport ------------------------------------------------------


class ExportRequest(BaseModel):
    album_id: int | None = None
    image_ids: list[int] | None = None
    dest_path: str | None = None
    keep_original_names: bool = False


class JobAccepted(BaseModel):
    job_id: str
    kind: str


class JobCurrent(BaseModel):
    running: bool
    kind: str | None = None
    step: str | None = None
    started_at: str | None = None
    message: str | None = None
    progress: dict = {}
    source_id: int | None = None  # kilden jobben er scopet til (None = alle)


# --- Kilder ----------------------------------------------------------------


class SourceItem(BaseModel):
    id: int
    root_path: str
    source_type: str
    last_scanned_at: str | None = None
    file_count: int = 0
    profile_id: int | None = None  # tagging-profil; None = arv globalt


class SourceList(BaseModel):
    items: list[SourceItem]


class SourceCreate(BaseModel):
    path: str
    profile_id: int | None = None  # valgfri tagging-profil


class SourceProfileUpdate(BaseModel):
    profile_id: int | None = None  # None fjerner profil (arv globalt)


# --- Tagging-profiler ------------------------------------------------------


class ProfileItem(BaseModel):
    id: int
    name: str
    focus: str | None = None
    categories: list[str] = []


class ProfileList(BaseModel):
    items: list[ProfileItem]


class ProfileCreate(BaseModel):
    name: str
    focus: str | None = None
    categories: list[str] = []


class ProfileUpdate(BaseModel):
    name: str
    focus: str | None = None
    categories: list[str] = []


# --- Filsystem-browse (mappe-velger i «Legg til mappe») --------------------


class FsEntry(BaseModel):
    name: str
    path: str  # absolutt


class FsList(BaseModel):
    path: str  # resolvet absolutt sti for gjeldende mappe
    parent: str | None = None  # foreldermappe, None når vi er på rota
    entries: list[FsEntry] = []  # undermapper (sortert, skjulte utelatt)


# --- Pipeline-jobber (oppdater-kjede + enkelt-steg) ------------------------


class UpdateRequest(BaseModel):
    source_id: int | None = None
    all: bool = False


class ScanRequest(BaseModel):
    source_id: int | None = None
    all: bool = False


class DeriveRequest(BaseModel):
    workers: int | None = None
    source_id: int | None = None  # None = alle pending


class TagRequest(BaseModel):
    limit: int | None = None
    workers: int | None = None
    retag: bool = False
    source_id: int | None = None  # None = alle pending


class FacesDetectRequest(BaseModel):
    workers: int | None = None
    retry_errors: bool = False
    source_id: int | None = None  # None = alle pending


class FacesClusterRequest(BaseModel):
    full: bool = False


class StageProgress(BaseModel):
    pending: int = 0
    done: int = 0
    error: int = 0


class Progress(BaseModel):
    derive: StageProgress = StageProgress()
    ai: StageProgress = StageProgress()
    faces: StageProgress = StageProgress()
    images_total: int = 0
    faces_total: int = 0
    persons_total: int = 0
    confirm_threshold: int = 200


class CostSummary(BaseModel):
    total: float = 0.0  # sum faktisk AI-kost (USD) for merkede bilder med kjent kost
    avg: float = 0.0  # snitt per bilde — estimeringsgrunnlag for kostnadsgaten
    count: int = 0  # antall merkede bilder med kjent kost


# --- Innstillinger + vedlikehold -------------------------------------------


class Settings(BaseModel):
    ai_model: str | None = None
    face_match_threshold: str | None = None
    face_min_cluster_size: str | None = None
    default_export_dir: str | None = None
    default_profile_id: int | None = None  # globalt standard tagging-profil; None = generisk
    openrouter_api_key: str = "(ikke satt)"  # alltid maskert


class SettingPut(BaseModel):
    value: str


class DataDir(BaseModel):
    data_dir: str


class DataDirMove(BaseModel):
    path: str
    delete_source: bool = False
