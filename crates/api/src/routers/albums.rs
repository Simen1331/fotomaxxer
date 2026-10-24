//! Album: rene databasekoblinger (`/albums…`).
//!
//! Motparten til `api/routers/albums.py`.
//!
//! **Jernregel: å legge til et bilde kopierer ALDRI en fil og lager ingen mappe.**
//! Tillegg, fjerning og omorganisering er ren database (`album_items`). Den eneste
//! filoperasjonen er «Eksporter til mappe» (`POST /api/export`, egen ruter). Et bilde kan
//! ligge i flere album. Album viser alle medlemmer — også ekskluderte — siden brukeren la dem
//! inn bevisst.
//!
//! Posisjonene følger Python nøyaktig, også i hjørnene: nye bilder får `MAX(position) + 1`
//! og oppover i rekkefølgen de ble sendt (duplikater, bilder som allerede er med og ukjente
//! id-er hoppes over uten å bruke en posisjon); omorganisering setter posisjonen til
//! indeksen i lista for hvert bilde som er med (ukjente id-er bruker opp en indeks, og ved
//! duplikater vinner den siste), og bilder som ikke er nevnt, beholder sin gamle posisjon.
//! Detaljvisningen sorterer på `position, image_id`.

use rusqlite::{Connection, OptionalExtension, params};

use super::Routes;
use super::image_query::thumb_url;
use super::persons::py_strip;
use crate::error::{ApiError, ApiResult, DictResponse, JsonResponse};
use crate::schemas::{
    AlbumAddItems, AlbumCreate, AlbumDetail, AlbumIdPath, AlbumItem, AlbumItemPath, AlbumList,
    AlbumReorder, AlbumSummary, Deleted,
};
use crate::state::blocking;
use crate::validate::DbReq;

pub fn routes() -> Routes {
    Routes::new()
        .get("/albums", list_albums)
        .post("/albums", create_album)
        .get("/albums/{album_id}", get_album)
        .post("/albums/{album_id}/items", add_items)
        .delete("/albums/{album_id}/items/{image_id}", remove_item)
        .patch("/albums/{album_id}/order", reorder)
        .delete("/albums/{album_id}", delete_album)
}

// --- Databaselogikk ------------------------------------------------------------------

/// `_require_album`: `(id, name)`, eller 404 med fast tekst.
fn require_album(conn: &Connection, album_id: i64) -> ApiResult<(i64, String)> {
    conn.query_row(
        "SELECT id, name FROM albums WHERE id = ?",
        [album_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()?
    .ok_or_else(|| ApiError::http(404, "not_found", "album finnes ikke"))
}

/// `get_album`: albumet med elementene i visningsrekkefølge.
fn album_detail(conn: &Connection, album_id: i64) -> ApiResult<AlbumDetail> {
    let (id, name) = require_album(conn, album_id)?;
    let mut stmt = conn.prepare(
        "SELECT ai.image_id, ai.position, i.content_hash AS hash
         FROM album_items ai JOIN images i ON i.id = ai.image_id
         WHERE ai.album_id = ?
         ORDER BY ai.position, ai.image_id",
    )?;
    let items = stmt
        .query_map([album_id], |r| {
            let hash: String = r.get(2)?;
            Ok(AlbumItem {
                image_id: r.get(0)?,
                thumb_url: thumb_url(&hash, "300"),
                hash,
                position: r.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(AlbumDetail { id, name, items })
}

/// Legg bildene til etter siste posisjon (idempotent). Bilder som allerede er med, og
/// id-er som ikke finnes i `images`, hoppes over. Alt i én transaksjon, som Pythons
/// implisitte transaksjon før `commit()`.
fn append_items(conn: &Connection, album_id: i64, image_ids: &[i64]) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    let last: i64 = tx.query_row(
        "SELECT COALESCE(MAX(position), -1) FROM album_items WHERE album_id = ?",
        [album_id],
        |r| r.get(0),
    )?;
    let mut pos = last + 1;
    for &image_id in image_ids {
        let already = tx
            .query_row(
                "SELECT 1 FROM album_items WHERE album_id = ? AND image_id = ?",
                [album_id, image_id],
                |_| Ok(()),
            )
            .optional()?;
        if already.is_some() {
            continue; // idempotent — allerede i albumet
        }
        // Bare bilder som finnes (FK ville feilet; Python hopper pent over).
        let exists = tx
            .query_row("SELECT 1 FROM images WHERE id = ?", [image_id], |_| Ok(()))
            .optional()?;
        if exists.is_none() {
            continue;
        }
        tx.execute(
            "INSERT INTO album_items (album_id, image_id, position) VALUES (?, ?, ?)",
            params![album_id, image_id, pos],
        )?;
        pos += 1;
    }
    tx.commit()
}

/// Ny rekkefølge fra indeksene i `image_ids` (se moduldokumentasjonen for hjørnene).
fn set_order(conn: &Connection, album_id: i64, image_ids: &[i64]) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    for (position, &image_id) in image_ids.iter().enumerate() {
        tx.execute(
            "UPDATE album_items SET position = ? WHERE album_id = ? AND image_id = ?",
            params![position as i64, album_id, image_id],
        )?;
    }
    tx.commit()
}

// --- Endepunkter ---------------------------------------------------------------------

/// `GET /api/albums`: nyeste først.
async fn list_albums(req: DbReq) -> ApiResult<JsonResponse<AlbumList>> {
    let conn = req.conn;
    blocking(move || {
        let mut stmt = conn.prepare(
            "SELECT a.id, a.name, a.created_at,
                    (SELECT COUNT(*) FROM album_items ai WHERE ai.album_id = a.id) AS item_count
             FROM albums a
             ORDER BY a.created_at DESC, a.id DESC",
        )?;
        let items = stmt
            .query_map([], |r| {
                Ok(AlbumSummary {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    created_at: r.get(2)?,
                    item_count: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(JsonResponse(AlbumList { items }))
    })
    .await
}

/// `POST /api/albums`: nytt, tomt album (navnet trimmes; tomt navn er 400).
async fn create_album(req: DbReq<(), (), AlbumCreate>) -> ApiResult<JsonResponse<AlbumSummary>> {
    let DbReq { conn, body, .. } = req;
    blocking(move || {
        let name = py_strip(&body.name);
        if name.is_empty() {
            return Err(ApiError::http(
                400,
                "bad_request",
                "navn kan ikke være tomt",
            ));
        }
        conn.execute("INSERT INTO albums (name) VALUES (?)", [name])?;
        let id = conn.last_insert_rowid();
        let summary = conn.query_row(
            "SELECT id, name, created_at FROM albums WHERE id = ?",
            [id],
            |r| {
                Ok(AlbumSummary {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    item_count: 0,
                    created_at: r.get(2)?,
                })
            },
        )?;
        Ok(JsonResponse(summary))
    })
    .await
}

/// `GET /api/albums/{album_id}`.
async fn get_album(req: DbReq<AlbumIdPath>) -> ApiResult<JsonResponse<AlbumDetail>> {
    let DbReq { conn, path, .. } = req;
    blocking(move || Ok(JsonResponse(album_detail(&conn, path.album_id)?))).await
}

/// `POST /api/albums/{album_id}/items`: legg til bilder (idempotent; etter siste posisjon).
async fn add_items(
    req: DbReq<AlbumIdPath, (), AlbumAddItems>,
) -> ApiResult<JsonResponse<AlbumDetail>> {
    let DbReq {
        conn, path, body, ..
    } = req;
    blocking(move || {
        require_album(&conn, path.album_id)?;
        append_items(&conn, path.album_id, &body.image_ids)?;
        Ok(JsonResponse(album_detail(&conn, path.album_id)?))
    })
    .await
}

/// `DELETE /api/albums/{album_id}/items/{image_id}`: fjern ett bilde (bare koblingen).
/// Et bilde som ikke er med, gir ingen feil.
async fn remove_item(req: DbReq<AlbumItemPath>) -> ApiResult<JsonResponse<AlbumDetail>> {
    let DbReq { conn, path, .. } = req;
    blocking(move || {
        require_album(&conn, path.album_id)?;
        conn.execute(
            "DELETE FROM album_items WHERE album_id = ? AND image_id = ?",
            [path.album_id, path.image_id],
        )?;
        Ok(JsonResponse(album_detail(&conn, path.album_id)?))
    })
    .await
}

/// `PATCH /api/albums/{album_id}/order`: ny rekkefølge fra rekkefølgen i `image_ids`.
async fn reorder(
    req: DbReq<AlbumIdPath, (), AlbumReorder>,
) -> ApiResult<JsonResponse<AlbumDetail>> {
    let DbReq {
        conn, path, body, ..
    } = req;
    blocking(move || {
        require_album(&conn, path.album_id)?;
        set_order(&conn, path.album_id, &body.image_ids)?;
        Ok(JsonResponse(album_detail(&conn, path.album_id)?))
    })
    .await
}

/// `DELETE /api/albums/{album_id}`: slett albumet (ikke bildene; `album_items` slettes
/// via `ON DELETE CASCADE`).
async fn delete_album(req: DbReq<AlbumIdPath>) -> ApiResult<DictResponse<Deleted>> {
    let DbReq { conn, path, .. } = req;
    blocking(move || {
        require_album(&conn, path.album_id)?;
        conn.execute("DELETE FROM albums WHERE id = ?", [path.album_id])?;
        Ok(DictResponse(Deleted {
            deleted: path.album_id,
        }))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn setup(images: i64) -> (Connection, i64) {
        let c = fotomaxxer_core::db::connect(Path::new(":memory:")).unwrap();
        fotomaxxer_core::db::init_db(&c).unwrap();
        for i in 1..=images {
            c.execute(
                "INSERT INTO images (id, content_hash) VALUES (?, ?)",
                params![i, format!("h{i}")],
            )
            .unwrap();
        }
        c.execute("INSERT INTO albums (name) VALUES ('Sommer')", [])
            .unwrap();
        let album = c.last_insert_rowid();
        (c, album)
    }

    /// `(image_id, position)` i visningsrekkefølge.
    fn items(c: &Connection, album: i64) -> Vec<(i64, i64)> {
        album_detail(c, album)
            .unwrap()
            .items
            .iter()
            .map(|i| (i.image_id, i.position))
            .collect()
    }

    #[test]
    fn legg_til_hopper_over_duplikater_og_ukjente() {
        let (c, a) = setup(5);
        append_items(&c, a, &[3, 1, 3, 99, 2]).unwrap();
        assert_eq!(items(&c, a), [(3, 0), (1, 1), (2, 2)]);
        // Idempotent: bilder som allerede er med, får ikke ny posisjon.
        append_items(&c, a, &[1, 5, 2, 4]).unwrap();
        assert_eq!(items(&c, a), [(3, 0), (1, 1), (2, 2), (5, 3), (4, 4)]);
        append_items(&c, a, &[]).unwrap();
        assert_eq!(items(&c, a).len(), 5);
        let d = album_detail(&c, a).unwrap();
        assert_eq!(d.items[0].thumb_url, "/derived/h3_300.jpg");
        assert_eq!((d.id, d.name.as_str()), (a, "Sommer"));
    }

    #[test]
    fn nye_bilder_etter_hoyeste_posisjon_ogsa_med_hull() {
        let (c, a) = setup(4);
        append_items(&c, a, &[1, 2, 3]).unwrap();
        c.execute(
            "DELETE FROM album_items WHERE album_id = ? AND image_id = 3",
            [a],
        )
        .unwrap();
        // MAX(position) er 1 nå, så neste får 2 — ikke antall + 1.
        append_items(&c, a, &[4, 3]).unwrap();
        assert_eq!(items(&c, a), [(1, 0), (2, 1), (4, 2), (3, 3)]);
        set_order(&c, a, &[3, 1]).unwrap();
        // 3 → 0, 1 → 1; 2 og 4 beholder 1 og 2. Like posisjoner sorteres på image_id.
        assert_eq!(items(&c, a), [(3, 0), (1, 1), (2, 1), (4, 2)]);
        append_items(&c, a, &[5, 2]).unwrap();
        assert_eq!(items(&c, a), [(3, 0), (1, 1), (2, 1), (4, 2)]);
    }

    #[test]
    fn omorganisering_som_python() {
        let (c, a) = setup(4);
        append_items(&c, a, &[1, 2, 3, 4]).unwrap();
        // Ukjent id bruker opp en indeks; ved duplikat vinner siste forekomst.
        // 4 → 0 (overskrives), 99 → ingenting, 2 → 2, 4 → 3; 1 og 3 er ikke nevnt.
        set_order(&c, a, &[4, 99, 2, 4]).unwrap();
        assert_eq!(items(&c, a), [(1, 0), (2, 2), (3, 2), (4, 3)]);
        set_order(&c, a, &[]).unwrap();
        assert_eq!(items(&c, a), [(1, 0), (2, 2), (3, 2), (4, 3)]);
        set_order(&c, a, &[3, 4, 1, 2]).unwrap();
        assert_eq!(items(&c, a), [(3, 0), (4, 1), (1, 2), (2, 3)]);
    }

    #[test]
    fn tomt_album_og_ukjent_album() {
        let (c, a) = setup(1);
        let d = album_detail(&c, a).unwrap();
        assert!(d.items.is_empty());
        set_order(&c, a, &[1]).unwrap();
        assert!(items(&c, a).is_empty());
        let err = album_detail(&c, 99).unwrap_err();
        assert_eq!(err.status().as_u16(), 404);
    }
}
