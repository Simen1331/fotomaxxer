//! Bilder / browse: liste med filtre + søk, id-liste, detalj, facets (`/images`, `/images/ids`, `/images/{image_id}`, `/facets`).
//!
//! Motparten til `api/routers/images.py`: lese-laget over `images`/`ai_tags`/`tags`/`faces`.
//! **Jernregel:** alle lister filtrerer `excluded = 0` med mindre `include_excluded=true`.
//! Filtrene AND-kombineres; `q` søker beskrivelse + tagger + sted med `LIKE`. Filtrene og
//! selve lista ligger i [`super::image_query`], som `persons.rs` også bruker.
//! Thumbnails/bilder serveres som statiske filer (`/derived/<hash>_<size>.jpg`), aldri
//! base64 i JSON.
//!
//! **Rutingen er Starlettes:** `PATCH /api/images/ids` treffer ikke `GET /images/ids`
//! (bare stien stemmer), men går videre til `PATCH /images/{image_id}` med
//! `image_id="ids"` og gir 422 — se [`patch_image_ids`].

use axum::extract::{FromRequest, Request, State};
use axum::response::{IntoResponse, Response};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{Map, Value};

use super::Routes;
use super::image_query::{ImageFilter, ORDER_BY, build_filters, list_page, thumb_url, where_sql};
use crate::error::{ApiError, ApiResult, JsonResponse};
use crate::schemas::{
    AiOut, FaceOut, FacetCount, FacetPerson, Facets, FileOut, ImageDetail, ImageIdPath, ImageIds,
    ImageIdsQuery, ImageList, ImagePatch, ImagePatchResult, ImagesQuery,
};
use crate::state::{AppState, blocking};
use crate::validate::{DbReq, parse_int_str, validate_object};

pub fn routes() -> Routes {
    Routes::new()
        .get("/images", list_images)
        .get("/images/ids", list_image_ids)
        .patch("/images/ids", patch_image_ids)
        .get("/images/{image_id}", get_image)
        .patch("/images/{image_id}", patch_image)
        .get("/facets", get_facets)
}

fn not_found() -> ApiError {
    ApiError::http(404, "not_found", "bilde finnes ikke")
}

async fn list_images(req: DbReq<(), ImagesQuery>) -> ApiResult<JsonResponse<ImageList>> {
    let DbReq { conn, query, .. } = req;
    blocking(move || {
        let filter = ImageFilter::from(&query);
        Ok(JsonResponse(list_page(
            &conn,
            &filter,
            query.limit,
            query.offset,
        )?))
    })
    .await
}

/// Alle id-er i gjeldende filter (for «velg alle i filter»). Ingen paginering.
async fn list_image_ids(req: DbReq<(), ImageIdsQuery>) -> ApiResult<JsonResponse<ImageIds>> {
    let DbReq { conn, query, .. } = req;
    blocking(move || {
        let (wh, params) = build_filters(&ImageFilter::from(&query));
        let sql = format!("SELECT i.id FROM images i {} {ORDER_BY}", where_sql(&wh));
        let mut stmt = conn.prepare(&sql)?;
        let ids = stmt
            .query_map(rusqlite::params_from_iter(&params), |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        Ok(JsonResponse(ImageIds { ids }))
    })
    .await
}

/// `json.loads(bbox)` validert som pydantics `list[int]` (lax: `1.0`, `true` og `"1"`
/// godtas). Noe annet er en ufanget valideringsfeil i Python → 500.
pub(crate) fn parse_bbox(text: &str) -> ApiResult<Vec<i64>> {
    let bad = || ApiError::internal(format!("bbox er ikke list[int]: {text}"));
    let value: Value = serde_json::from_str(text).map_err(|_| bad())?;
    let Value::Array(items) = value else {
        return Err(bad());
    };
    items
        .iter()
        .map(|v| match v {
            Value::Number(n) => n.as_i64().or_else(|| {
                n.as_f64()
                    .filter(|f| f.fract() == 0.0 && f.abs() < 9.2e18)
                    .map(|f| f as i64)
            }),
            Value::Bool(b) => Some(i64::from(*b)),
            Value::String(s) => parse_int_str(s).ok(),
            _ => None,
        })
        .collect::<Option<Vec<i64>>>()
        .ok_or_else(bad)
}

fn image_detail(conn: &Connection, image_id: i64) -> ApiResult<ImageDetail> {
    let img = conn
        .query_row(
            "SELECT id, content_hash, width, height, taken_at, year, season, place_name,
                    camera_make, camera_model, gps_lat, gps_lon, excluded
             FROM images WHERE id = ?",
            [image_id],
            |r| {
                let hash: String = r.get(1)?;
                Ok(ImageDetail {
                    id: r.get(0)?,
                    image_url: thumb_url(&hash, "1024"),
                    hash,
                    width: r.get(2)?,
                    height: r.get(3)?,
                    taken_at: r.get(4)?,
                    year: r.get(5)?,
                    season: r.get(6)?,
                    place_name: r.get(7)?,
                    camera_make: r.get(8)?,
                    camera_model: r.get(9)?,
                    gps_lat: r.get(10)?,
                    gps_lon: r.get(11)?,
                    excluded: r.get::<_, i64>(12)? != 0,
                    ai: None,
                    tags: Vec::new(),
                    faces: Vec::new(),
                    files: Vec::new(),
                })
            },
        )
        .optional()?;
    let Some(mut detail) = img else {
        return Err(not_found());
    };

    detail.ai = conn
        .query_row(
            "SELECT scene, event_type, season_ai, indoor_outdoor, person_count, description, provider \
             FROM ai_tags WHERE image_id = ? ORDER BY tagged_at DESC LIMIT 1",
            [image_id],
            |r| {
                Ok(AiOut {
                    scene: r.get(0)?,
                    event_type: r.get(1)?,
                    season_ai: r.get(2)?,
                    indoor_outdoor: r.get(3)?,
                    person_count: r.get(4)?,
                    description: r.get(5)?,
                    provider: r.get(6)?,
                })
            },
        )
        .optional()?;

    let mut stmt = conn.prepare(
        "SELECT t.name FROM tags t JOIN image_tags it ON it.tag_id = t.id \
         WHERE it.image_id = ? ORDER BY t.name",
    )?;
    detail.tags = stmt
        .query_map([image_id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;

    let mut stmt = conn.prepare(
        "SELECT f.id, f.bbox, f.cluster_id, f.locked, p.name AS person_name \
         FROM faces f LEFT JOIN persons p ON p.cluster_id = f.cluster_id \
         WHERE f.image_id = ? ORDER BY f.id",
    )?;
    let rows = stmt
        .query_map([image_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (id, bbox, cluster_id, locked, person_name) in rows {
        detail.faces.push(FaceOut {
            id,
            bbox: parse_bbox(&bbox)?,
            cluster_id,
            person_name,
            locked: locked != 0,
        });
    }

    let mut stmt =
        conn.prepare("SELECT path, source_id FROM files WHERE image_id = ? ORDER BY id")?;
    detail.files = stmt
        .query_map([image_id], |r| {
            Ok(FileOut {
                path: r.get(0)?,
                source_id: r.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(detail)
}

async fn get_image(req: DbReq<ImageIdPath>) -> ApiResult<JsonResponse<ImageDetail>> {
    let DbReq { conn, path, .. } = req;
    blocking(move || Ok(JsonResponse(image_detail(&conn, path.image_id)?))).await
}

/// Ekskludér/gjeninnsett et bilde (mykt `images.excluded`). Originalen røres aldri.
async fn patch_image(
    req: DbReq<ImageIdPath, (), ImagePatch>,
) -> ApiResult<JsonResponse<ImagePatchResult>> {
    let DbReq {
        conn, path, body, ..
    } = req;
    blocking(move || {
        let image_id = path.image_id;
        let exists = conn
            .query_row("SELECT 1 FROM images WHERE id = ?", [image_id], |_| Ok(()))
            .optional()?;
        if exists.is_none() {
            return Err(not_found());
        }
        conn.execute(
            "UPDATE images SET excluded = ? WHERE id = ?",
            [i64::from(body.excluded), image_id],
        )?;
        Ok(JsonResponse(ImagePatchResult {
            id: image_id,
            excluded: body.excluded,
        }))
    })
    .await
}

/// `PATCH /api/images/ids`. I Starlette er `GET /images/ids` bare en delvis treff
/// (feil metode); `PATCH /images/{image_id}` lenger ned er et fullt treff og vinner, så
/// FastAPI validerer `image_id="ids"` (422 `int_parsing`) sammen med kroppen. axum
/// velger den statiske stien, så vi gjenskaper svaret her: stifeilen først, så
/// eventuelle kroppsfeil — eller bare kroppens JSON-/400-feil, som avbryter før
/// valideringen i FastAPI.
async fn patch_image_ids(State(state): State<AppState>, req: Request) -> Response {
    let mut path = Map::new();
    path.insert("image_id".into(), Value::String("ids".into()));
    let mut errors = Vec::new();
    // Gir alltid feil: "ids" er ikke et heltall.
    let _ = validate_object::<ImageIdPath>(&path, "path", &Value::Null, &mut errors);
    match DbReq::<(), (), ImagePatch>::from_request(req, &state).await {
        Ok(_) => ApiError::Validation(errors).into_response(),
        Err(ApiError::Validation(body_errors))
            if !body_errors.iter().any(|e| e["type"] == "json_invalid") =>
        {
            errors.extend(body_errors);
            ApiError::Validation(errors).into_response()
        }
        Err(other) => other.into_response(),
    }
}

/// Filter-dimensjoner for ikke-ekskluderte bilder.
fn facets(conn: &Connection) -> rusqlite::Result<Facets> {
    fn column<T: rusqlite::types::FromSql>(
        conn: &Connection,
        sql: &str,
    ) -> rusqlite::Result<Vec<T>> {
        let mut stmt = conn.prepare(sql)?;
        stmt.query_map([], |r| r.get(0))?.collect()
    }
    let years = column(
        conn,
        "SELECT DISTINCT year FROM images WHERE excluded = 0 AND year IS NOT NULL ORDER BY year DESC",
    )?;
    let seasons = column(
        conn,
        "SELECT DISTINCT season FROM images WHERE excluded = 0 AND season IS NOT NULL ORDER BY season",
    )?;
    let places = column(
        conn,
        "SELECT DISTINCT place_name FROM images WHERE excluded = 0 AND place_name IS NOT NULL \
         ORDER BY place_name",
    )?;
    let events = column(
        conn,
        "SELECT DISTINCT a.event_type FROM ai_tags a JOIN images i ON i.id = a.image_id \
         WHERE i.excluded = 0 AND a.event_type IS NOT NULL ORDER BY a.event_type",
    )?;
    let mut stmt = conn.prepare(
        "SELECT t.name AS name, t.category AS category, COUNT(*) AS c FROM tags t \
         JOIN image_tags it ON it.tag_id = t.id \
         JOIN images i ON i.id = it.image_id \
         WHERE i.excluded = 0 GROUP BY t.id ORDER BY c DESC, t.name",
    )?;
    let tags = stmt
        .query_map([], |r| {
            Ok(FacetCount {
                name: r.get(0)?,
                category: r.get(1)?,
                count: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut stmt = conn.prepare(
        "SELECT p.cluster_id AS cluster_id, p.name AS name, COUNT(f.id) AS c \
         FROM persons p JOIN faces f ON f.cluster_id = p.cluster_id \
         JOIN images i ON i.id = f.image_id \
         WHERE i.excluded = 0 GROUP BY p.cluster_id ORDER BY c DESC, p.name IS NULL, p.name",
    )?;
    let persons = stmt
        .query_map([], |r| {
            Ok(FacetPerson {
                cluster_id: r.get(0)?,
                name: r.get(1)?,
                count: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Facets {
        years,
        seasons,
        places,
        events,
        tags,
        persons,
    })
}

async fn get_facets(req: DbReq) -> ApiResult<JsonResponse<Facets>> {
    let conn = req.conn;
    blocking(move || Ok(JsonResponse(facets(&conn)?))).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bbox_som_pydantic_list_int() {
        assert_eq!(
            parse_bbox("[1380, -471, 1274, 1423]").unwrap(),
            [1380, -471, 1274, 1423]
        );
        assert_eq!(parse_bbox("[1.0, true, \" 7 \"]").unwrap(), [1, 1, 7]);
        assert_eq!(parse_bbox("[]").unwrap(), Vec::<i64>::new());
        for bad in ["[1.5]", "{\"x\": 1}", "[null]", "ikke json", "[[1]]"] {
            assert!(parse_bbox(bad).is_err(), "{bad}");
        }
    }

    fn db() -> Connection {
        let conn = fotomaxxer_core::db::connect(std::path::Path::new(":memory:")).unwrap();
        fotomaxxer_core::db::init_db(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO sources (id, root_path) VALUES (1, '/a'), (2, '/b');
             INSERT INTO images (id, content_hash, taken_at, year, season, place_name, gps_lat, gps_lon, excluded)
               VALUES (1, 'h1', '2011-07-01 12:00:00', 2011, 'sommer', 'Bø i Telemark', 59.0, 9.07, 0),
                      (2, 'h2', NULL, NULL, NULL, NULL, NULL, NULL, 0),
                      (3, 'h3', '2012-01-02 08:00:00', 2012, 'vinter', 'Ål', NULL, NULL, 1);
             INSERT INTO files (image_id, source_id, path) VALUES (1, 2, '/b/x.jpg'), (1, 1, '/a/x.jpg');
             INSERT INTO persons (cluster_id, name) VALUES (1, 'Åse'), (2, NULL);
             INSERT INTO faces (image_id, bbox, embedding, cluster_id, locked)
               VALUES (1, '[1, 2, 3, 4]', x'00', 2, 1), (1, '[5, 6, 7, 8]', x'00', 1, 0),
                      (3, '[0, 0, 1, 1]', x'00', 1, 0);
             INSERT INTO ai_tags (image_id, provider, event_type, indoor_outdoor, description, tagged_at)
               VALUES (1, 'p', 'ferie', 'ute', 'gammel', '2026-01-01 00:00:00'),
                      (1, 'p', 'bursdag', 'inne', 'ny', '2026-01-02 00:00:00'),
                      (3, 'p', 'jul', 'inne', 'snø', '2026-01-01 00:00:00');
             INSERT INTO tags (id, name, category) VALUES (1, 'strand', NULL), (2, 'Ørret', 'mat');
             INSERT INTO image_tags VALUES (1, 2), (1, 1), (3, 1);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn detalj_med_ai_tagger_ansikter_og_filer() {
        let conn = db();
        let d = image_detail(&conn, 1).unwrap();
        assert_eq!(d.image_url, "/derived/h1_1024.jpg");
        assert_eq!(d.ai.as_ref().unwrap().description.as_deref(), Some("ny"));
        assert_eq!(d.tags, ["strand", "Ørret"]);
        assert_eq!(
            d.faces
                .iter()
                .map(|f| (f.id, f.person_name.as_deref(), f.locked))
                .collect::<Vec<_>>(),
            [(1, None, true), (2, Some("Åse"), false)]
        );
        assert_eq!(
            d.files.iter().map(|f| f.source_id).collect::<Vec<_>>(),
            [2, 1]
        );
        let tom = image_detail(&conn, 2).unwrap();
        assert!(tom.ai.is_none() && tom.tags.is_empty() && tom.faces.is_empty());
        assert!(matches!(
            image_detail(&conn, 99),
            Err(ApiError::Http { status, .. }) if status == 404
        ));
    }

    #[test]
    fn fasetter_uten_ekskluderte() {
        let f = facets(&db()).unwrap();
        assert_eq!(f.years, [2011]);
        assert_eq!(f.seasons, ["sommer"]);
        assert_eq!(f.places, ["Bø i Telemark"]);
        assert_eq!(f.events, ["bursdag", "ferie"]);
        assert_eq!(
            f.tags
                .iter()
                .map(|t| (t.name.as_str(), t.count))
                .collect::<Vec<_>>(),
            [("strand", 1), ("Ørret", 1)]
        );
        assert_eq!(
            f.persons
                .iter()
                .map(|p| (p.cluster_id, p.count))
                .collect::<Vec<_>>(),
            [(1, 1), (2, 1)]
        );
    }
}
