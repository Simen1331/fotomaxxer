//! Personer / ansikter: oversikt, ansikts-rutenett og korrigering (`/persons…`, `/faces/{face_id}`).
//!
//! Motparten til `api/routers/persons.py`. Lese-laget over `persons`/`faces`/`images`;
//! mutasjonene er de rene kurateringsfunksjonene i [`fotomaxxer_core::faces`]
//! (`reassign_face`, `set_face_locked`, `create_person_from_faces`, `merge_persons`,
//! `delete_person`, `ignore_person`, `set_person_name`), kalt uendret. Hver committer selv
//! og gir [`Error::Faces`] (Pythons `ValueError`) på ukjent ansikt/person — det blir 404
//! eller 400 her, som i Python.
//!
//! Ansikter på ekskluderte bilder skjules (samme jernregel som bildelistene): cover og
//! rutenett joiner `images` med `excluded = 0`.

use fotomaxxer_core::{Error, faces};
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;

use super::Routes;
use super::image_query::{self, ImageFilter};
use crate::error::{ApiError, ApiResult, DictResponse, JsonResponse};
use crate::schemas::{
    ClusterIdPath, Deleted, FaceIdPath, FacePatch, FacePatchResult, ImageList, PageQuery,
    PersonCover, PersonCreate, PersonCreateResult, PersonFaceItem, PersonFaceList, PersonIgnored,
    PersonList, PersonMerge, PersonRename, PersonRenameResult, PersonSummary,
};
use crate::state::blocking;
use crate::validate::DbReq;

pub fn routes() -> Routes {
    Routes::new()
        .get("/persons", list_persons)
        .get("/persons/{cluster_id}/faces", person_faces)
        .get("/persons/{cluster_id}/images", person_images)
        .put("/persons/{cluster_id}", rename_person)
        .post("/persons", create_person)
        .post("/persons/{cluster_id}/merge", merge_person)
        .delete("/persons/{cluster_id}", remove_person)
        .post("/persons/{cluster_id}/ignore", ignore_person)
        .put("/faces/{face_id}", patch_face)
}

// --- Hjelpere ------------------------------------------------------------------------

/// Pythons `str.strip()`: Unicode-mellomrom pluss `\x1c`–`\x1f` (som Rusts
/// `char::is_whitespace` ikke regner med).
pub(crate) fn py_strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\x1c'..='\x1f').contains(&c))
}

/// `body.name.strip() if body.name and body.name.strip() else None`.
fn clean_name(name: Option<&str>) -> Option<String> {
    name.map(py_strip)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// `ValueError` fra `fotomaxxer.faces` → `HTTPException(status, {"error", "message"})`;
/// andre feil (database) → 500, som et ufanget unntak i Python.
fn faces_error(status: u16, error: &'static str) -> impl Fn(Error) -> ApiError {
    move |e| match e {
        Error::Faces(message) => ApiError::http(status, error, message),
        other => ApiError::from(other),
    }
}

/// `_require_person`: 404 med fast tekst (uten id) når klyngen ikke finnes.
fn require_person(conn: &Connection, cluster_id: i64) -> ApiResult<()> {
    let found = conn
        .query_row(
            "SELECT 1 FROM persons WHERE cluster_id = ?",
            [cluster_id],
            |_| Ok(()),
        )
        .optional()?;
    match found {
        Some(()) => Ok(()),
        None => Err(ApiError::http(404, "not_found", "person finnes ikke")),
    }
}

/// `json.loads(bbox)` validert som pydantics `list[int]` (lax: `3.0` → 3). Ugyldig
/// JSON eller en annen form gir 500, som i Python.
fn parse_bbox(text: &str) -> ApiResult<Vec<i64>> {
    let value: Value = serde_json::from_str(text)
        .map_err(|e| ApiError::internal(format!("ugyldig bbox {text:?}: {e}")))?;
    let items = value
        .as_array()
        .ok_or_else(|| ApiError::internal(format!("bbox er ikke en liste: {text:?}")))?;
    items
        .iter()
        .map(|v| match v {
            Value::Number(n) => n
                .as_i64()
                .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64)),
            _ => None,
        })
        .collect::<Option<Vec<i64>>>()
        .ok_or_else(|| ApiError::internal(format!("bbox er ikke heltall: {text:?}")))
}

/// `_cover`: ansiktet som representerer personen — et låst (manuelt bekreftet) ansikt
/// foran de andre, ellers det eldste; bare ansikter på ikke-ekskluderte bilder.
fn cover(conn: &Connection, cluster_id: i64) -> ApiResult<Option<PersonCover>> {
    let row = conn
        .query_row(
            "SELECT f.image_id, i.content_hash AS hash, f.bbox, i.width, i.height
             FROM faces f JOIN images i ON i.id = f.image_id
             WHERE f.cluster_id = ? AND i.excluded = 0
             ORDER BY f.locked DESC, f.id
             LIMIT 1",
            [cluster_id],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<i64>>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                ))
            },
        )
        .optional()?;
    row.map(|(image_id, hash, bbox, width, height)| {
        Ok(PersonCover {
            image_id,
            hash,
            bbox: parse_bbox(&bbox)?,
            width,
            height,
        })
    })
    .transpose()
}

/// Klyngekortene: alle personer med minst ett ikke-ekskludert ansikt, flest ansikter
/// først, så navngitte foran unavngitte og alfabetisk.
fn person_list(conn: &Connection) -> ApiResult<PersonList> {
    let mut stmt = conn.prepare(
        "SELECT p.cluster_id AS cluster_id, p.name AS name, COUNT(f.id) AS face_count
         FROM persons p
         JOIN faces f ON f.cluster_id = p.cluster_id
         JOIN images i ON i.id = f.image_id
         WHERE i.excluded = 0
         GROUP BY p.cluster_id
         ORDER BY face_count DESC, p.name IS NULL, p.name",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let items = rows
        .into_iter()
        .map(|(cluster_id, name, face_count)| {
            Ok(PersonSummary {
                cluster_id,
                name,
                face_count,
                cover: cover(conn, cluster_id)?,
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(PersonList { items })
}

// --- Endepunkter ---------------------------------------------------------------------

/// `GET /api/persons`.
async fn list_persons(req: DbReq) -> ApiResult<JsonResponse<PersonList>> {
    let conn = req.conn;
    blocking(move || Ok(JsonResponse(person_list(&conn)?))).await
}

/// `GET /api/persons/{cluster_id}/faces`: ansikts-rutenettet i person-detaljen.
async fn person_faces(req: DbReq<ClusterIdPath>) -> ApiResult<JsonResponse<PersonFaceList>> {
    let DbReq { conn, path, .. } = req;
    blocking(move || {
        require_person(&conn, path.cluster_id)?;
        let mut stmt = conn.prepare(
            "SELECT f.id AS face_id, f.image_id, i.content_hash AS hash, f.bbox, f.locked,
                    i.width, i.height
             FROM faces f JOIN images i ON i.id = f.image_id
             WHERE f.cluster_id = ? AND i.excluded = 0
             ORDER BY f.id",
        )?;
        let rows = stmt
            .query_map([path.cluster_id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, Option<i64>>(5)?,
                    r.get::<_, Option<i64>>(6)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let items = rows
            .into_iter()
            .map(|(face_id, image_id, hash, bbox, locked, width, height)| {
                Ok(PersonFaceItem {
                    face_id,
                    image_id,
                    hash,
                    bbox: parse_bbox(&bbox)?,
                    locked: locked != 0,
                    width,
                    height,
                })
            })
            .collect::<ApiResult<Vec<_>>>()?;
        Ok(JsonResponse(PersonFaceList { items }))
    })
    .await
}

/// `GET /api/persons/{cluster_id}/images`: bildene personen finnes i (samme form og
/// rekkefølge som `GET /api/images?person=…`, uten de andre filtrene).
async fn person_images(req: DbReq<ClusterIdPath, PageQuery>) -> ApiResult<JsonResponse<ImageList>> {
    let DbReq {
        conn, path, query, ..
    } = req;
    blocking(move || {
        require_person(&conn, path.cluster_id)?;
        let filter = ImageFilter {
            person: Some(path.cluster_id),
            ..Default::default()
        };
        let page = image_query::list_page(&conn, &filter, query.limit, query.offset)?;
        Ok(JsonResponse(page))
    })
    .await
}

/// `PUT /api/persons/{cluster_id}`: navngi/omdøp. Tomt navn eller `null` fjerner navnet
/// (klyngen består).
async fn rename_person(
    req: DbReq<ClusterIdPath, (), PersonRename>,
) -> ApiResult<JsonResponse<PersonRenameResult>> {
    let DbReq {
        conn, path, body, ..
    } = req;
    blocking(move || {
        let name = clean_name(body.name.as_deref());
        faces::set_person_name(&conn, path.cluster_id, name.as_deref())
            .map_err(faces_error(404, "not_found"))?;
        Ok(JsonResponse(PersonRenameResult {
            cluster_id: path.cluster_id,
            name,
        }))
    })
    .await
}

/// `POST /api/persons`: ny person fra et utvalg ansikter (splitt).
async fn create_person(
    req: DbReq<(), (), PersonCreate>,
) -> ApiResult<JsonResponse<PersonCreateResult>> {
    let DbReq { conn, body, .. } = req;
    blocking(move || {
        let name = clean_name(body.name.as_deref());
        let cluster_id = faces::create_person_from_faces(&conn, &body.face_ids, name.as_deref())
            .map_err(faces_error(400, "bad_request"))?;
        Ok(JsonResponse(PersonCreateResult { cluster_id }))
    })
    .await
}

/// `POST /api/persons/{cluster_id}/merge`: flytt alle ansikter til `into_cluster_id` og
/// slett denne klyngen. Alle feil (også ukjent person) er 400 her, som i Python.
async fn merge_person(
    req: DbReq<ClusterIdPath, (), PersonMerge>,
) -> ApiResult<JsonResponse<PersonRenameResult>> {
    let DbReq {
        conn, path, body, ..
    } = req;
    blocking(move || {
        let into = body.into_cluster_id;
        faces::merge_persons(&conn, path.cluster_id, into)
            .map_err(faces_error(400, "bad_request"))?;
        let name = conn
            .query_row(
                "SELECT name FROM persons WHERE cluster_id = ?",
                [into],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        Ok(JsonResponse(PersonRenameResult {
            cluster_id: into,
            name,
        }))
    })
    .await
}

/// `DELETE /api/persons/{cluster_id}`: slett klyngen; ansiktene løsnes (`cluster_id → NULL`)
/// og kan bli klynget på nytt.
async fn remove_person(req: DbReq<ClusterIdPath>) -> ApiResult<DictResponse<Deleted>> {
    let DbReq { conn, path, .. } = req;
    blocking(move || {
        faces::delete_person(&conn, path.cluster_id).map_err(faces_error(404, "not_found"))?;
        Ok(DictResponse(Deleted {
            deleted: path.cluster_id,
        }))
    })
    .await
}

/// `POST /api/persons/{cluster_id}/ignore`: kryss bort en søppel-person permanent (lås +
/// løsne ansiktene + slett klyngen). Ansiktene blir aldri klynget på nytt.
async fn ignore_person(req: DbReq<ClusterIdPath>) -> ApiResult<DictResponse<PersonIgnored>> {
    let DbReq { conn, path, .. } = req;
    blocking(move || {
        let n =
            faces::ignore_person(&conn, path.cluster_id).map_err(faces_error(404, "not_found"))?;
        Ok(DictResponse(PersonIgnored {
            ignored: path.cluster_id,
            faces: i64::try_from(n).map_err(ApiError::internal)?,
        }))
    })
    .await
}

/// `PUT /api/faces/{face_id}`: flytt ansiktet (`cluster_id`) og/eller frys/ignorér det
/// (`locked`). Bare feltene klienten sendte anvendes (`model_fields_set`);
/// `locked=true` + `cluster_id=null` = ignorér et søppel-ansikt.
async fn patch_face(
    req: DbReq<FaceIdPath, (), FacePatch>,
) -> ApiResult<JsonResponse<FacePatchResult>> {
    let DbReq {
        conn,
        path,
        body,
        body_fields,
        ..
    } = req;
    blocking(move || {
        if body_fields.is_empty() {
            return Err(ApiError::http(400, "bad_request", "ingen felt å oppdatere"));
        }
        let face_id = path.face_id;
        let not_found = faces_error(404, "not_found");
        if body_fields.contains(&"cluster_id") {
            faces::reassign_face(&conn, face_id, body.cluster_id).map_err(&not_found)?;
        }
        if body_fields.contains(&"locked") {
            // `bool(body.locked)`: `null` tiner opp.
            faces::set_face_locked(&conn, face_id, body.locked.unwrap_or(false))
                .map_err(&not_found)?;
        }
        let (cluster_id, locked) = conn.query_row(
            "SELECT cluster_id, locked FROM faces WHERE id = ?",
            [face_id],
            |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, i64>(1)?)),
        )?;
        Ok(JsonResponse(FacePatchResult {
            id: face_id,
            cluster_id,
            locked: locked != 0,
        }))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn conn() -> Connection {
        let c = fotomaxxer_core::db::connect(Path::new(":memory:")).unwrap();
        fotomaxxer_core::db::init_db(&c).unwrap();
        c
    }

    fn image(c: &Connection, hash: &str, excluded: bool) -> i64 {
        c.execute(
            "INSERT INTO images (content_hash, width, height, excluded) VALUES (?, 4000, 3000, ?)",
            rusqlite::params![hash, i64::from(excluded)],
        )
        .unwrap();
        c.last_insert_rowid()
    }

    fn face(c: &Connection, image_id: i64, cluster: Option<i64>, locked: bool) -> i64 {
        c.execute(
            "INSERT INTO faces (image_id, bbox, embedding, cluster_id, locked) \
             VALUES (?, '[10, 20, 30, 40]', x'00', ?, ?)",
            rusqlite::params![image_id, cluster, i64::from(locked)],
        )
        .unwrap();
        c.last_insert_rowid()
    }

    fn person(c: &Connection, name: Option<&str>) -> i64 {
        c.execute("INSERT INTO persons (name) VALUES (?)", [name])
            .unwrap();
        c.last_insert_rowid()
    }

    #[test]
    fn strip_som_python() {
        assert_eq!(py_strip("  Åse \u{a0}\t"), "Åse");
        assert_eq!(py_strip("\x1fØ\x1c"), "Ø");
        assert_eq!(py_strip("\u{200b}x"), "\u{200b}x"); // ZWSP er ikke mellomrom i Python
        assert_eq!(clean_name(Some("  ")), None);
        assert_eq!(clean_name(Some("")), None);
        assert_eq!(clean_name(None), None);
        assert_eq!(clean_name(Some(" Bjørn ")).as_deref(), Some("Bjørn"));
    }

    #[test]
    fn bbox_som_pydantic_liste_av_heltall() {
        assert_eq!(parse_bbox("[1, 2, 3, 4]").unwrap(), [1, 2, 3, 4]);
        assert_eq!(parse_bbox("[1.0, -2]").unwrap(), [1, -2]);
        assert!(parse_bbox("[1.5]").is_err());
        assert!(parse_bbox("{}").is_err());
        assert!(parse_bbox("ikke json").is_err());
    }

    #[test]
    fn cover_foretrekker_laste_og_hopper_over_ekskluderte() {
        let c = conn();
        let p = person(&c, None);
        let ex = image(&c, "ekskludert", true);
        let a = image(&c, "a", false);
        let b = image(&c, "b", false);
        // Laveste ansikts-id er på et ekskludert bilde og skal ikke bli cover.
        face(&c, ex, Some(p), true);
        face(&c, a, Some(p), false);
        let fb = face(&c, b, Some(p), false);
        let cov = cover(&c, p).unwrap().unwrap();
        assert_eq!((cov.image_id, cov.hash.as_str()), (a, "a"));
        assert_eq!(cov.bbox, [10, 20, 30, 40]);
        assert_eq!((cov.width, cov.height), (Some(4000), Some(3000)));
        // Et låst ansikt går foran, selv med høyere id.
        c.execute("UPDATE faces SET locked = 1 WHERE id = ?", [fb])
            .unwrap();
        assert_eq!(cover(&c, p).unwrap().unwrap().image_id, b);
        // Ingen synlige ansikter → ingen cover.
        c.execute("UPDATE images SET excluded = 1", []).unwrap();
        assert_eq!(cover(&c, p).unwrap(), None);
        assert_eq!(cover(&c, 999).unwrap(), None);
    }

    #[test]
    fn personliste_sortert_og_uten_tomme_klynger() {
        let c = conn();
        let ukjent = person(&c, None);
        let bjorn = person(&c, Some("Bjørn"));
        let aase = person(&c, Some("Åse"));
        let tom = person(&c, Some("Tom"));
        let skjult = person(&c, Some("Skjult"));
        let img = image(&c, "h", false);
        let ex = image(&c, "x", true);
        for (p, n) in [(ukjent, 2), (bjorn, 1), (aase, 1), (skjult, 1)] {
            for _ in 0..n {
                face(&c, img, Some(p), false);
            }
        }
        // Ansikter på ekskluderte bilder teller ikke: `skjult` har tre, men ett synlig.
        face(&c, ex, Some(skjult), false);
        face(&c, ex, Some(skjult), false);
        let list = person_list(&c).unwrap();
        let got: Vec<(i64, Option<&str>, i64)> = list
            .items
            .iter()
            .map(|p| (p.cluster_id, p.name.as_deref(), p.face_count))
            .collect();
        // Flest ansikter først; ved likt antall navngitte foran unavngitte, så BINARY-
        // sortering (som SQLite: «Skjult» < «Åse»). `tom` har ingen ansikter.
        assert_eq!(
            got,
            [
                (ukjent, None, 2),
                (bjorn, Some("Bjørn"), 1),
                (skjult, Some("Skjult"), 1),
                (aase, Some("Åse"), 1),
            ]
        );
        assert!(list.items.iter().all(|p| p.cover.is_some()));
        assert!(!got.iter().any(|g| g.0 == tom));
    }
}
