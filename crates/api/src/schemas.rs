//! Forespørsels- og svarskjemaer — motparten til `api/schemas.py`, for HELE API-et.
//!
//! Svarmodellene serialiseres med samme feltnavn og -rekkefølge som pydantic-modellene,
//! alle felt alltid med (`None` → `null`, aldri utelatt), heltall som heltall og
//! flyttall (`f64`) i pydantics format (via [`crate::error::JsonResponse`]). De uformelle
//! `dict`-svarene nederst sendes med [`crate::error::DictResponse`] (`json.dumps`).
//! Frontend-typene i `web/src/api/client.ts` speiler dette.
//!
//! Forespørselsmodellene lages med [`crate::model!`] og valideres pydantic-likt
//! (se [`crate::validate`]). Sti- og spørreparametre er også modeller her, så feltnavn
//! og rekkefølge i 422-feil (`loc`) blir som i FastAPI.

use serde::Serialize;
use serde_json::{Map, Value};

use crate::model;

// --- Bilder / browse ---------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImageListItem {
    pub id: i64,
    pub hash: String,
    pub thumb_url: String,
    pub taken_at: Option<String>,
    pub year: Option<i64>,
    pub season: Option<String>,
    pub place_name: Option<String>,
    pub person_count: i64,
    pub tag_count: i64,
    pub excluded: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImageList {
    pub items: Vec<ImageListItem>,
    pub total: i64,
}

model! {
    /// `PATCH /api/images/{image_id}`.
    pub struct ImagePatch {
        pub excluded: bool,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImagePatchResult {
    pub id: i64,
    pub excluded: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImageIds {
    pub ids: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FaceOut {
    pub id: i64,
    pub bbox: Vec<i64>,
    pub cluster_id: Option<i64>,
    pub person_name: Option<String>,
    pub locked: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct AiOut {
    pub scene: Option<String>,
    pub event_type: Option<String>,
    pub season_ai: Option<String>,
    pub indoor_outdoor: Option<String>,
    pub person_count: Option<i64>,
    pub description: Option<String>,
    pub provider: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FileOut {
    pub path: String,
    pub source_id: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImageDetail {
    pub id: i64,
    pub hash: String,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub taken_at: Option<String>,
    pub year: Option<i64>,
    pub season: Option<String>,
    pub place_name: Option<String>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub gps_lat: Option<f64>,
    pub gps_lon: Option<f64>,
    pub excluded: bool,
    pub image_url: String,
    pub ai: Option<AiOut>,
    pub tags: Vec<String>,
    pub faces: Vec<FaceOut>,
    pub files: Vec<FileOut>,
}

// --- Facets --------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FacetCount {
    pub name: String,
    pub count: i64,
    /// Tagging-profil-kategori; `None` = ukategorisert.
    pub category: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FacetPerson {
    pub cluster_id: i64,
    pub name: Option<String>,
    pub count: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Facets {
    pub years: Vec<i64>,
    pub seasons: Vec<String>,
    pub places: Vec<String>,
    pub events: Vec<String>,
    pub tags: Vec<FacetCount>,
    pub persons: Vec<FacetPerson>,
}

// --- Album -----------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AlbumSummary {
    pub id: i64,
    pub name: String,
    pub item_count: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AlbumList {
    pub items: Vec<AlbumSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AlbumItem {
    pub image_id: i64,
    pub hash: String,
    pub thumb_url: String,
    pub position: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AlbumDetail {
    pub id: i64,
    pub name: String,
    pub items: Vec<AlbumItem>,
}

model! {
    /// `POST /api/albums`.
    pub struct AlbumCreate {
        pub name: String,
    }
}

model! {
    /// `POST /api/albums/{album_id}/items`.
    pub struct AlbumAddItems {
        pub image_ids: Vec<i64>,
    }
}

model! {
    /// `PATCH /api/albums/{album_id}/order`.
    pub struct AlbumReorder {
        pub image_ids: Vec<i64>,
    }
}

// --- Personer / ansikter -----------------------------------------------------------
//
// `width`/`height` er originalbildets dimensjoner — frontend trenger dem for å
// CSS-croppe ansiktet (bbox er i originalkoordinater) mot 1024-derivatet.

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PersonCover {
    pub image_id: i64,
    pub hash: String,
    pub bbox: Vec<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PersonSummary {
    pub cluster_id: i64,
    pub name: Option<String>,
    pub face_count: i64,
    pub cover: Option<PersonCover>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PersonList {
    pub items: Vec<PersonSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PersonFaceItem {
    pub face_id: i64,
    pub image_id: i64,
    pub hash: String,
    pub bbox: Vec<i64>,
    pub locked: bool,
    pub width: Option<i64>,
    pub height: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PersonFaceList {
    pub items: Vec<PersonFaceItem>,
}

model! {
    /// `PUT /api/persons/{cluster_id}`. `name=null` fjerner navnet.
    pub struct PersonRename {
        pub name: Option<String> = null,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PersonRenameResult {
    pub cluster_id: i64,
    pub name: Option<String>,
}

model! {
    /// `POST /api/persons` (ny person fra et utvalg ansikter).
    pub struct PersonCreate {
        pub name: Option<String> = null,
        pub face_ids: Vec<i64>,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PersonCreateResult {
    pub cluster_id: i64,
}

model! {
    /// `POST /api/persons/{cluster_id}/merge`.
    pub struct PersonMerge {
        pub into_cluster_id: i64,
    }
}

model! {
    /// `PUT /api/faces/{face_id}`. Bare feltene klienten sendte anvendes — les
    /// `body_fields` fra [`crate::validate::DbReq`] (pydantics `model_fields_set`).
    pub struct FacePatch {
        pub cluster_id: Option<i64> = null,
        pub locked: Option<bool> = null,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FacePatchResult {
    pub id: i64,
    pub cluster_id: Option<i64>,
    pub locked: bool,
}

// --- Jobber / eksport --------------------------------------------------------------

model! {
    /// `POST /api/export`.
    pub struct ExportRequest {
        pub album_id: Option<i64> = null,
        pub image_ids: Option<Vec<i64>> = null,
        pub dest_path: Option<String> = null,
        pub keep_original_names: bool = false,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobAccepted {
    pub job_id: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobCurrent {
    pub running: bool,
    pub kind: Option<String>,
    pub step: Option<String>,
    pub started_at: Option<String>,
    pub message: Option<String>,
    pub progress: Map<String, Value>,
    /// Kilden jobben er avgrenset til (`None` = alle).
    pub source_id: Option<i64>,
}

// --- Kilder ------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SourceItem {
    pub id: i64,
    pub root_path: String,
    pub source_type: String,
    pub last_scanned_at: Option<String>,
    pub file_count: i64,
    /// Tagging-profil; `None` = arv globalt.
    pub profile_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SourceList {
    pub items: Vec<SourceItem>,
}

model! {
    /// `POST /api/sources`.
    pub struct SourceCreate {
        pub path: String,
        /// Valgfri tagging-profil.
        pub profile_id: Option<i64> = null,
    }
}

model! {
    /// `PATCH /api/sources/{source_id}`. `None` fjerner profilen (arv globalt).
    pub struct SourceProfileUpdate {
        pub profile_id: Option<i64> = null,
    }
}

/// `DELETE /api/sources/{source_id}` (dict uten `response_model` i Python).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SourceRemoved {
    pub removed: i64,
    pub summary: String,
}

// --- Tagging-profiler ----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProfileItem {
    pub id: i64,
    pub name: String,
    pub focus: Option<String>,
    pub categories: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProfileList {
    pub items: Vec<ProfileItem>,
}

model! {
    /// `POST /api/profiles`.
    pub struct ProfileCreate {
        pub name: String,
        pub focus: Option<String> = null,
        pub categories: Vec<String> = [],
    }
}

model! {
    /// `PUT /api/profiles/{profile_id}`.
    pub struct ProfileUpdate {
        pub name: String,
        pub focus: Option<String> = null,
        pub categories: Vec<String> = [],
    }
}

// --- Filsystem-browse (mappe-velger i «Legg til mappe») ---------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FsEntry {
    pub name: String,
    /// Absolutt.
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FsList {
    /// Resolvet absolutt sti for gjeldende mappe.
    pub path: String,
    /// Foreldermappe, `None` når vi er på rota.
    pub parent: Option<String>,
    /// Undermapper (sortert, skjulte utelatt).
    pub entries: Vec<FsEntry>,
}

// --- Pipeline-jobber (oppdater-kjede + enkelt-steg) ------------------------------------

model! {
    /// `POST /api/jobs/update`.
    pub struct UpdateRequest {
        pub source_id: Option<i64> = null,
        pub all: bool = false,
    }
}

model! {
    /// `POST /api/jobs/scan`.
    pub struct ScanRequest {
        pub source_id: Option<i64> = null,
        pub all: bool = false,
    }
}

model! {
    /// `POST /api/jobs/derive`.
    pub struct DeriveRequest {
        pub workers: Option<i64> = null,
        /// `None` = alle pending.
        pub source_id: Option<i64> = null,
    }
}

model! {
    /// `POST /api/jobs/tag`.
    pub struct TagRequest {
        pub limit: Option<i64> = null,
        pub workers: Option<i64> = null,
        pub retag: bool = false,
        pub source_id: Option<i64> = null,
    }
}

model! {
    /// `POST /api/jobs/faces-detect`.
    pub struct FacesDetectRequest {
        pub workers: Option<i64> = null,
        pub retry_errors: bool = false,
        pub source_id: Option<i64> = null,
    }
}

model! {
    /// `POST /api/jobs/faces-cluster`.
    pub struct FacesClusterRequest {
        pub full: bool = false,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct StageProgress {
    pub pending: i64,
    pub done: i64,
    pub error: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Progress {
    pub derive: StageProgress,
    pub ai: StageProgress,
    pub faces: StageProgress,
    pub images_total: i64,
    pub faces_total: i64,
    pub persons_total: i64,
    pub confirm_threshold: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct CostSummary {
    /// Sum faktisk AI-kost (USD) for merkede bilder med kjent kost.
    pub total: f64,
    /// Snitt per bilde — estimeringsgrunnlag for kostnadsgaten.
    pub avg: f64,
    /// Antall merkede bilder med kjent kost.
    pub count: i64,
}

// --- Innstillinger + vedlikehold ---------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Settings {
    pub ai_model: Option<String>,
    pub face_match_threshold: Option<String>,
    pub face_min_cluster_size: Option<String>,
    pub default_export_dir: Option<String>,
    /// Globalt standard tagging-profil; `None` = generisk.
    pub default_profile_id: Option<i64>,
    /// Alltid maskert.
    pub openrouter_api_key: String,
}

model! {
    /// `PUT /api/settings/{key}`.
    pub struct SettingPut {
        pub value: String,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DataDir {
    pub data_dir: String,
}

model! {
    /// `POST /api/maintenance/data-dir`.
    pub struct DataDirMove {
        pub path: String,
        pub delete_source: bool = false,
    }
}

// --- Uformelle svar (dict uten `response_model` i Python) ---------------------------------

/// `GET /api/health`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Health {
    pub status: String,
}

/// `{"deleted": id}` (profiler, album, personer).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Deleted {
    pub deleted: i64,
}

/// `{"message": …}` (`POST /api/maintenance/prepare-handoff`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Message {
    pub message: String,
}

/// `POST /api/persons/{cluster_id}/ignore`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PersonIgnored {
    pub ignored: i64,
    pub faces: i64,
}

// --- Sti- og spørreparametre ----------------------------------------------------------------

model! {
    /// `{source_id}` i stien.
    pub struct SourceIdPath {
        pub source_id: i64,
    }
}

model! {
    /// `{profile_id}` i stien.
    pub struct ProfileIdPath {
        pub profile_id: i64,
    }
}

model! {
    /// `{key}` i `PUT /api/settings/{key}`.
    pub struct SettingKeyPath {
        pub key: String,
    }
}

model! {
    /// `{image_id}` i stien.
    pub struct ImageIdPath {
        pub image_id: i64,
    }
}

model! {
    /// `{cluster_id}` i stien.
    pub struct ClusterIdPath {
        pub cluster_id: i64,
    }
}

model! {
    /// `{face_id}` i stien.
    pub struct FaceIdPath {
        pub face_id: i64,
    }
}

model! {
    /// `{album_id}` i stien.
    pub struct AlbumIdPath {
        pub album_id: i64,
    }
}

model! {
    /// `{album_id}/items/{image_id}` i stien.
    pub struct AlbumItemPath {
        pub album_id: i64,
        pub image_id: i64,
    }
}

model! {
    /// `?source_id=` for `/api/progress` og `/api/cost`.
    pub struct SourceIdQuery {
        pub source_id: Option<i64> = null,
    }
}

model! {
    /// `?path=` for `/api/fs/list`.
    pub struct FsListQuery {
        pub path: Option<String> = null,
    }
}

model! {
    /// Filtrene i `GET /api/images` (rekkefølgen er funksjonssignaturen i Python).
    pub struct ImagesQuery {
        pub year: Option<i64> = null,
        pub season: Option<String> = null,
        pub place: Option<String> = null,
        pub event: Option<String> = null,
        pub indoor_outdoor: Option<String> = null,
        pub tag: Option<String> = null,
        pub person: Option<i64> = null,
        pub q: Option<String> = null,
        pub include_excluded: bool = false,
        pub limit: i64 = 200 => ge(1) le(1000),
        pub offset: i64 = 0 => ge(0),
    }
}

model! {
    /// Filtrene i `GET /api/images/ids` (uten paginering).
    pub struct ImageIdsQuery {
        pub year: Option<i64> = null,
        pub season: Option<String> = null,
        pub place: Option<String> = null,
        pub event: Option<String> = null,
        pub indoor_outdoor: Option<String> = null,
        pub tag: Option<String> = null,
        pub person: Option<i64> = null,
        pub q: Option<String> = null,
        pub include_excluded: bool = false,
    }
}

model! {
    /// `?limit=&offset=` for `GET /api/persons/{cluster_id}/images`.
    pub struct PageQuery {
        pub limit: i64 = 200 => ge(1) le(1000),
        pub offset: i64 = 0 => ge(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validate::{Model, validate_object};
    use fotomaxxer_core::pyjson::{dumps_compact, dumps_pydantic};
    use serde_json::json;

    #[test]
    fn svar_har_pydantics_feltrekkefolge_og_null() {
        let s = Settings {
            ai_model: Some("google/gemini-2.5-flash".into()),
            face_match_threshold: Some("0.5".into()),
            face_min_cluster_size: Some("3".into()),
            default_export_dir: None,
            default_profile_id: None,
            openrouter_api_key: "(ikke satt)".into(),
        };
        // Fasit: FastAPI-svaret fra GET /api/settings på en tom pakke.
        assert_eq!(
            dumps_compact(&s),
            r#"{"ai_model":"google/gemini-2.5-flash","face_match_threshold":"0.5","face_min_cluster_size":"3","default_export_dir":null,"default_profile_id":null,"openrouter_api_key":"(ikke satt)"}"#
        );
        let p = Progress {
            derive: StageProgress::default(),
            ai: StageProgress::default(),
            faces: StageProgress::default(),
            images_total: 0,
            faces_total: 0,
            persons_total: 0,
            confirm_threshold: 200,
        };
        assert_eq!(
            dumps_compact(&p),
            r#"{"derive":{"pending":0,"done":0,"error":0},"ai":{"pending":0,"done":0,"error":0},"faces":{"pending":0,"done":0,"error":0},"images_total":0,"faces_total":0,"persons_total":0,"confirm_threshold":200}"#
        );
    }

    #[test]
    fn flyttall_som_pydantic() {
        let c = CostSummary::default();
        assert_eq!(dumps_pydantic(&c), r#"{"total":0.0,"avg":0.0,"count":0}"#);
        // Fasit: GET /api/cost fra FastAPI etter to merkede bilder.
        let c = CostSummary {
            total: 0.000_107,
            avg: 5.35e-5,
            count: 2,
        };
        assert_eq!(
            dumps_pydantic(&c),
            r#"{"total":0.000107,"avg":0.0000535,"count":2}"#
        );
        let d = ImageDetail {
            id: 1,
            hash: "h".into(),
            width: Some(10),
            height: None,
            taken_at: None,
            year: None,
            season: None,
            place_name: Some("Bø i Telemark".into()),
            camera_make: None,
            camera_model: None,
            gps_lat: Some(59.0),
            gps_lon: Some(9.07),
            excluded: false,
            image_url: "/derived/h_1024.jpg".into(),
            ai: None,
            tags: vec![],
            faces: vec![],
            files: vec![],
        };
        assert!(dumps_pydantic(&d).contains(r#""gps_lat":59.0,"gps_lon":9.07,"excluded":false"#));
    }

    #[test]
    fn jobb_status_med_progress_i_rekkefolge() {
        let mut progress = Map::new();
        progress.insert("scan_seen".into(), json!(50));
        progress.insert("scan_new".into(), json!(3));
        let j = JobCurrent {
            running: false,
            kind: Some("scan".into()),
            step: Some("scan".into()),
            started_at: Some("2026-10-23T19:24:26+00:00".into()),
            message: Some("Skanning ferdig.".into()),
            progress,
            source_id: None,
        };
        assert_eq!(
            dumps_compact(&j),
            r#"{"running":false,"kind":"scan","step":"scan","started_at":"2026-10-23T19:24:26+00:00","message":"Skanning ferdig.","progress":{"scan_seen":50,"scan_new":3},"source_id":null}"#
        );
    }

    /// Hver forespørselsmodell må kunne bygges fra sine standardverdier pluss de
    /// påkrevde feltene (feltlisten og serde-structen er i takt).
    #[test]
    fn foresporselsmodeller_er_i_takt() {
        fn sample<T: Model>(required: Value) {
            let mut errors = Vec::new();
            let Value::Object(map) = &required else {
                panic!()
            };
            assert!(
                validate_object::<T>(map, "body", &required, &mut errors).is_some(),
                "{}: {errors:?}",
                std::any::type_name::<T>()
            );
        }
        sample::<ImagePatch>(json!({"excluded": true}));
        sample::<AlbumCreate>(json!({"name": "a"}));
        sample::<AlbumAddItems>(json!({"image_ids": [1]}));
        sample::<AlbumReorder>(json!({"image_ids": []}));
        sample::<PersonRename>(json!({}));
        sample::<PersonCreate>(json!({"face_ids": [1, 2]}));
        sample::<PersonMerge>(json!({"into_cluster_id": 1}));
        sample::<FacePatch>(json!({}));
        sample::<ExportRequest>(json!({}));
        sample::<SourceCreate>(json!({"path": "/x"}));
        sample::<SourceProfileUpdate>(json!({}));
        sample::<ProfileCreate>(json!({"name": "p"}));
        sample::<ProfileUpdate>(json!({"name": "p"}));
        sample::<UpdateRequest>(json!({}));
        sample::<ScanRequest>(json!({}));
        sample::<DeriveRequest>(json!({}));
        sample::<TagRequest>(json!({}));
        sample::<FacesDetectRequest>(json!({}));
        sample::<FacesClusterRequest>(json!({}));
        sample::<SettingPut>(json!({"value": "v"}));
        sample::<DataDirMove>(json!({"path": "/x"}));
        sample::<SourceIdPath>(json!({"source_id": "1"}));
        sample::<ProfileIdPath>(json!({"profile_id": "1"}));
        sample::<SettingKeyPath>(json!({"key": "ai_model"}));
        sample::<ImageIdPath>(json!({"image_id": "1"}));
        sample::<ClusterIdPath>(json!({"cluster_id": "1"}));
        sample::<FaceIdPath>(json!({"face_id": "1"}));
        sample::<AlbumIdPath>(json!({"album_id": "1"}));
        sample::<AlbumItemPath>(json!({"album_id": "1", "image_id": "2"}));
        sample::<SourceIdQuery>(json!({}));
        sample::<FsListQuery>(json!({}));
        sample::<ImagesQuery>(json!({"limit": "5", "include_excluded": "true"}));
        sample::<ImageIdsQuery>(json!({}));
        sample::<PageQuery>(json!({}));
    }

    #[test]
    fn spørring_med_grenser_som_fastapi() {
        let mut errors = Vec::new();
        let q = json!({"limit": "5000", "offset": "-1"});
        let Value::Object(map) = &q else { panic!() };
        assert!(validate_object::<ImagesQuery>(map, "query", &Value::Null, &mut errors).is_none());
        // Fasit: GET /api/images?limit=5000&offset=-1 mot FastAPI.
        assert_eq!(
            dumps_compact(&json!({"detail": errors})),
            r#"{"detail":[{"type":"less_than_equal","loc":["query","limit"],"msg":"Input should be less than or equal to 1000","input":"5000","ctx":{"le":1000}},{"type":"greater_than_equal","loc":["query","offset"],"msg":"Input should be greater than or equal to 0","input":"-1","ctx":{"ge":0}}]}"#
        );
    }
}
