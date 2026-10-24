//! Felles bildefiltre og -lister for `images.py` og `persons.py`.
//!
//! I Python importerer `persons.py` `_build_filters` og `_thumb_url` fra `images.py`,
//! og `GET /api/persons/{id}/images` bruker samme SQL som `GET /api/images`. Her ligger
//! de i en egen modul, så de to trinn B-ruterne kan bygges parallelt uten å dele fil.
//!
//! **Jernregel:** alle lister filtrerer `excluded = 0` med mindre `include_excluded`.
//! Filtrene AND-kombineres; `q` søker beskrivelse + tagger + sted med `LIKE`.

use rusqlite::Connection;
use rusqlite::types::Value;

use crate::schemas::{ImageIdsQuery, ImageList, ImageListItem, ImagesQuery};

/// `/derived/<hash>_<size>.jpg` (`_thumb_url`).
pub fn thumb_url(content_hash: &str, size: &str) -> String {
    format!("/derived/{content_hash}_{size}.jpg")
}

/// Filterdimensjonene (`_build_filters`-argumentene).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImageFilter {
    pub year: Option<i64>,
    pub season: Option<String>,
    pub place: Option<String>,
    pub event: Option<String>,
    pub indoor_outdoor: Option<String>,
    pub tag: Option<String>,
    pub person: Option<i64>,
    pub q: Option<String>,
    pub include_excluded: bool,
}

impl From<&ImagesQuery> for ImageFilter {
    fn from(q: &ImagesQuery) -> Self {
        Self {
            year: q.year,
            season: q.season.clone(),
            place: q.place.clone(),
            event: q.event.clone(),
            indoor_outdoor: q.indoor_outdoor.clone(),
            tag: q.tag.clone(),
            person: q.person,
            q: q.q.clone(),
            include_excluded: q.include_excluded,
        }
    }
}

impl From<&ImageIdsQuery> for ImageFilter {
    fn from(q: &ImageIdsQuery) -> Self {
        Self {
            year: q.year,
            season: q.season.clone(),
            place: q.place.clone(),
            event: q.event.clone(),
            indoor_outdoor: q.indoor_outdoor.clone(),
            tag: q.tag.clone(),
            person: q.person,
            q: q.q.clone(),
            include_excluded: q.include_excluded,
        }
    }
}

/// Pythons `if season:` — `None` og tom streng er «ikke satt».
fn truthy(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|v| !v.is_empty())
}

/// `_build_filters`: WHERE-ledd + parametre, i samme rekkefølge som Python.
pub fn build_filters(f: &ImageFilter) -> (Vec<&'static str>, Vec<Value>) {
    let mut wh: Vec<&'static str> = Vec::new();
    let mut params: Vec<Value> = Vec::new();
    if !f.include_excluded {
        wh.push("i.excluded = 0");
    }
    if let Some(year) = f.year {
        wh.push("i.year = ?");
        params.push(Value::Integer(year));
    }
    if let Some(season) = truthy(&f.season) {
        wh.push("i.season = ?");
        params.push(Value::Text(season.into()));
    }
    if let Some(place) = truthy(&f.place) {
        wh.push("i.place_name = ?");
        params.push(Value::Text(place.into()));
    }
    if let Some(event) = truthy(&f.event) {
        wh.push("EXISTS (SELECT 1 FROM ai_tags a WHERE a.image_id = i.id AND a.event_type = ?)");
        params.push(Value::Text(event.into()));
    }
    if let Some(io) = truthy(&f.indoor_outdoor) {
        wh.push(
            "EXISTS (SELECT 1 FROM ai_tags a WHERE a.image_id = i.id AND a.indoor_outdoor = ?)",
        );
        params.push(Value::Text(io.into()));
    }
    if let Some(tag) = truthy(&f.tag) {
        wh.push(
            "EXISTS (SELECT 1 FROM image_tags it JOIN tags t ON t.id = it.tag_id \
             WHERE it.image_id = i.id AND t.name = ?)",
        );
        params.push(Value::Text(tag.into()));
    }
    if let Some(person) = f.person {
        wh.push("EXISTS (SELECT 1 FROM faces f WHERE f.image_id = i.id AND f.cluster_id = ?)");
        params.push(Value::Integer(person));
    }
    if let Some(q) = truthy(&f.q) {
        let like = format!("%{q}%");
        wh.push(
            "(\
             EXISTS (SELECT 1 FROM ai_tags a WHERE a.image_id = i.id AND a.description LIKE ?) \
             OR EXISTS (SELECT 1 FROM image_tags it JOIN tags t ON t.id = it.tag_id \
                        WHERE it.image_id = i.id AND t.name LIKE ?) \
             OR i.place_name LIKE ?\
             )",
        );
        params.extend([
            Value::Text(like.clone()),
            Value::Text(like.clone()),
            Value::Text(like),
        ]);
    }
    (wh, params)
}

/// `"WHERE a AND b"` eller tom streng.
pub fn where_sql(wh: &[&str]) -> String {
    if wh.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", wh.join(" AND "))
    }
}

/// Rekkefølgen alle bildelister bruker.
pub const ORDER_BY: &str = "ORDER BY i.taken_at IS NULL, i.taken_at DESC, i.id DESC";

/// Én side av bildelista (`GET /api/images` og `GET /api/persons/{id}/images`).
pub fn list_page(
    conn: &Connection,
    filter: &ImageFilter,
    limit: i64,
    offset: i64,
) -> rusqlite::Result<ImageList> {
    let (wh, params) = build_filters(filter);
    let where_sql = where_sql(&wh);
    let total: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM images i {where_sql}"),
        rusqlite::params_from_iter(&params),
        |r| r.get(0),
    )?;
    let sql = format!(
        "SELECT i.id, i.content_hash, i.taken_at, i.year, i.season, i.place_name, i.excluded,
                (SELECT COUNT(*) FROM faces f WHERE f.image_id = i.id)          AS person_count,
                (SELECT COUNT(*) FROM image_tags it WHERE it.image_id = i.id)   AS tag_count
         FROM images i
         {where_sql}
         {ORDER_BY}
         LIMIT ? OFFSET ?"
    );
    let mut all = params;
    all.push(Value::Integer(limit));
    all.push(Value::Integer(offset));
    let mut stmt = conn.prepare(&sql)?;
    let items = stmt
        .query_map(rusqlite::params_from_iter(&all), |r| {
            let hash: String = r.get(1)?;
            Ok(ImageListItem {
                id: r.get(0)?,
                thumb_url: thumb_url(&hash, "300"),
                hash,
                taken_at: r.get(2)?,
                year: r.get(3)?,
                season: r.get(4)?,
                place_name: r.get(5)?,
                excluded: r.get::<_, i64>(6)? != 0,
                person_count: r.get(7)?,
                tag_count: r.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ImageList { items, total })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filtre_i_python_rekkefolge() {
        let (wh, params) = build_filters(&ImageFilter::default());
        assert_eq!(wh, ["i.excluded = 0"]);
        assert!(params.is_empty());
        let f = ImageFilter {
            year: Some(2011),
            season: Some(String::new()),
            person: Some(3),
            q: Some("strand".into()),
            include_excluded: true,
            ..Default::default()
        };
        let (wh, params) = build_filters(&f);
        assert_eq!(wh.len(), 3);
        assert_eq!(wh[0], "i.year = ?");
        assert!(wh[1].contains("f.cluster_id = ?"));
        assert_eq!(params.len(), 5);
        assert_eq!(params[4], Value::Text("%strand%".into()));
    }

    #[test]
    fn side_fra_tom_database() {
        let conn = fotomaxxer_core::db::connect(std::path::Path::new(":memory:")).unwrap();
        fotomaxxer_core::db::init_db(&conn).unwrap();
        conn.execute(
            "INSERT INTO images (content_hash, taken_at, year) VALUES ('h1', '2011-07-01 12:00:00', 2011)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO images (content_hash, excluded) VALUES ('h2', 1)",
            [],
        )
        .unwrap();
        let page = list_page(&conn, &ImageFilter::default(), 200, 0).unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].thumb_url, "/derived/h1_300.jpg");
        let all = ImageFilter {
            include_excluded: true,
            ..Default::default()
        };
        let page = list_page(&conn, &all, 200, 0).unwrap();
        assert_eq!(
            page.items
                .iter()
                .map(|i| i.hash.as_str())
                .collect::<Vec<_>>(),
            ["h1", "h2"]
        );
    }
}
