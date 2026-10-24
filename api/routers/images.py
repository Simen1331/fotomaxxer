"""Bilder / browse: liste med filtre + søk, id-liste, detalj, facets.

Lese-laget over ``images``/``ai_tags``/``tags``/``faces``. **Jernregel: alle
lister filtrerer ``excluded=0`` med mindre ``include_excluded=true``.** Filtrene
AND-kombineres. ``q`` søker beskrivelse + tags + sted med ``LIKE`` (v1).
Thumbnails/bilder serveres som statiske filer (``/derived/<hash>_<size>.jpg``),
aldri base64 i JSON.
"""

from __future__ import annotations

import json
import sqlite3

from fastapi import APIRouter, Depends, HTTPException, Query

from api.deps import get_conn
from api.schemas import (
    AiOut,
    FaceOut,
    Facets,
    FacetCount,
    FacetPerson,
    FileOut,
    ImageDetail,
    ImageIds,
    ImageList,
    ImageListItem,
    ImagePatch,
    ImagePatchResult,
)

router = APIRouter(tags=["images"])


def _thumb_url(content_hash: str, size: str) -> str:
    return f"/derived/{content_hash}_{size}.jpg"


def _build_filters(
    year: int | None,
    season: str | None,
    place: str | None,
    event: str | None,
    indoor_outdoor: str | None,
    tag: str | None,
    person: int | None,
    q: str | None,
    include_excluded: bool,
) -> tuple[list[str], list[object]]:
    """Bygg WHERE-ledd + parametre. Alle ledd AND-kombineres."""
    where: list[str] = []
    params: list[object] = []

    if not include_excluded:
        where.append("i.excluded = 0")
    if year is not None:
        where.append("i.year = ?")
        params.append(year)
    if season:
        where.append("i.season = ?")
        params.append(season)
    if place:
        where.append("i.place_name = ?")
        params.append(place)
    if event:
        where.append("EXISTS (SELECT 1 FROM ai_tags a WHERE a.image_id = i.id AND a.event_type = ?)")
        params.append(event)
    if indoor_outdoor:
        where.append("EXISTS (SELECT 1 FROM ai_tags a WHERE a.image_id = i.id AND a.indoor_outdoor = ?)")
        params.append(indoor_outdoor)
    if tag:
        where.append(
            "EXISTS (SELECT 1 FROM image_tags it JOIN tags t ON t.id = it.tag_id "
            "WHERE it.image_id = i.id AND t.name = ?)"
        )
        params.append(tag)
    if person is not None:
        where.append("EXISTS (SELECT 1 FROM faces f WHERE f.image_id = i.id AND f.cluster_id = ?)")
        params.append(person)
    if q:
        like = f"%{q}%"
        where.append(
            "("
            "EXISTS (SELECT 1 FROM ai_tags a WHERE a.image_id = i.id AND a.description LIKE ?) "
            "OR EXISTS (SELECT 1 FROM image_tags it JOIN tags t ON t.id = it.tag_id "
            "           WHERE it.image_id = i.id AND t.name LIKE ?) "
            "OR i.place_name LIKE ?"
            ")"
        )
        params.extend([like, like, like])

    return where, params


@router.get("/images", response_model=ImageList)
def list_images(
    conn: sqlite3.Connection = Depends(get_conn),
    year: int | None = None,
    season: str | None = None,
    place: str | None = None,
    event: str | None = None,
    indoor_outdoor: str | None = None,
    tag: str | None = None,
    person: int | None = None,
    q: str | None = None,
    include_excluded: bool = False,
    limit: int = Query(200, ge=1, le=1000),
    offset: int = Query(0, ge=0),
) -> ImageList:
    where, params = _build_filters(
        year, season, place, event, indoor_outdoor, tag, person, q, include_excluded
    )
    where_sql = ("WHERE " + " AND ".join(where)) if where else ""

    total = conn.execute(f"SELECT COUNT(*) FROM images i {where_sql}", params).fetchone()[0]

    rows = conn.execute(
        f"""
        SELECT i.id, i.content_hash, i.taken_at, i.year, i.season, i.place_name, i.excluded,
               (SELECT COUNT(*) FROM faces f WHERE f.image_id = i.id)          AS person_count,
               (SELECT COUNT(*) FROM image_tags it WHERE it.image_id = i.id)   AS tag_count
        FROM images i
        {where_sql}
        ORDER BY i.taken_at IS NULL, i.taken_at DESC, i.id DESC
        LIMIT ? OFFSET ?
        """,
        [*params, limit, offset],
    ).fetchall()

    items = [
        ImageListItem(
            id=r["id"],
            hash=r["content_hash"],
            thumb_url=_thumb_url(r["content_hash"], "300"),
            taken_at=r["taken_at"],
            year=r["year"],
            season=r["season"],
            place_name=r["place_name"],
            person_count=r["person_count"],
            tag_count=r["tag_count"],
            excluded=bool(r["excluded"]),
        )
        for r in rows
    ]
    return ImageList(items=items, total=total)


@router.get("/images/ids", response_model=ImageIds)
def list_image_ids(
    conn: sqlite3.Connection = Depends(get_conn),
    year: int | None = None,
    season: str | None = None,
    place: str | None = None,
    event: str | None = None,
    indoor_outdoor: str | None = None,
    tag: str | None = None,
    person: int | None = None,
    q: str | None = None,
    include_excluded: bool = False,
) -> ImageIds:
    """Alle id-er i gjeldende filter (for «velg alle i filter»). Ingen paginering."""
    where, params = _build_filters(
        year, season, place, event, indoor_outdoor, tag, person, q, include_excluded
    )
    where_sql = ("WHERE " + " AND ".join(where)) if where else ""
    rows = conn.execute(
        f"SELECT i.id FROM images i {where_sql} ORDER BY i.taken_at IS NULL, i.taken_at DESC, i.id DESC",
        params,
    ).fetchall()
    return ImageIds(ids=[r["id"] for r in rows])


@router.get("/images/{image_id}", response_model=ImageDetail)
def get_image(image_id: int, conn: sqlite3.Connection = Depends(get_conn)) -> ImageDetail:
    img = conn.execute("SELECT * FROM images WHERE id = ?", (image_id,)).fetchone()
    if img is None:
        raise HTTPException(status_code=404, detail={"error": "not_found", "message": "bilde finnes ikke"})

    ai_row = conn.execute(
        "SELECT scene, event_type, season_ai, indoor_outdoor, person_count, description, provider "
        "FROM ai_tags WHERE image_id = ? ORDER BY tagged_at DESC LIMIT 1",
        (image_id,),
    ).fetchone()
    ai = AiOut(**dict(ai_row)) if ai_row is not None else None

    tags = [
        r["name"]
        for r in conn.execute(
            "SELECT t.name FROM tags t JOIN image_tags it ON it.tag_id = t.id "
            "WHERE it.image_id = ? ORDER BY t.name",
            (image_id,),
        )
    ]

    faces = [
        FaceOut(
            id=r["id"],
            bbox=json.loads(r["bbox"]),
            cluster_id=r["cluster_id"],
            person_name=r["person_name"],
            locked=bool(r["locked"]),
        )
        for r in conn.execute(
            "SELECT f.id, f.bbox, f.cluster_id, f.locked, p.name AS person_name "
            "FROM faces f LEFT JOIN persons p ON p.cluster_id = f.cluster_id "
            "WHERE f.image_id = ? ORDER BY f.id",
            (image_id,),
        )
    ]

    files = [
        FileOut(path=r["path"], source_id=r["source_id"])
        for r in conn.execute(
            "SELECT path, source_id FROM files WHERE image_id = ? ORDER BY id", (image_id,)
        )
    ]

    return ImageDetail(
        id=img["id"],
        hash=img["content_hash"],
        width=img["width"],
        height=img["height"],
        taken_at=img["taken_at"],
        year=img["year"],
        season=img["season"],
        place_name=img["place_name"],
        camera_make=img["camera_make"],
        camera_model=img["camera_model"],
        gps_lat=img["gps_lat"],
        gps_lon=img["gps_lon"],
        excluded=bool(img["excluded"]),
        image_url=_thumb_url(img["content_hash"], "1024"),
        ai=ai,
        tags=tags,
        faces=faces,
        files=files,
    )


@router.patch("/images/{image_id}", response_model=ImagePatchResult)
def patch_image(
    image_id: int, body: ImagePatch, conn: sqlite3.Connection = Depends(get_conn)
) -> ImagePatchResult:
    """Ekskludér/gjeninnsett et bilde (mykt ``images.excluded``). Originalen røres aldri."""
    exists = conn.execute("SELECT 1 FROM images WHERE id = ?", (image_id,)).fetchone()
    if exists is None:
        raise HTTPException(status_code=404, detail={"error": "not_found", "message": "bilde finnes ikke"})
    conn.execute("UPDATE images SET excluded = ? WHERE id = ?", (1 if body.excluded else 0, image_id))
    conn.commit()
    return ImagePatchResult(id=image_id, excluded=body.excluded)


@router.get("/facets", response_model=Facets)
def get_facets(conn: sqlite3.Connection = Depends(get_conn)) -> Facets:
    """Filter-dimensjoner for ikke-ekskluderte bilder."""
    years = [
        r[0]
        for r in conn.execute(
            "SELECT DISTINCT year FROM images WHERE excluded = 0 AND year IS NOT NULL ORDER BY year DESC"
        )
    ]
    seasons = [
        r[0]
        for r in conn.execute(
            "SELECT DISTINCT season FROM images WHERE excluded = 0 AND season IS NOT NULL ORDER BY season"
        )
    ]
    places = [
        r[0]
        for r in conn.execute(
            "SELECT DISTINCT place_name FROM images WHERE excluded = 0 AND place_name IS NOT NULL "
            "ORDER BY place_name"
        )
    ]
    events = [
        r[0]
        for r in conn.execute(
            "SELECT DISTINCT a.event_type FROM ai_tags a JOIN images i ON i.id = a.image_id "
            "WHERE i.excluded = 0 AND a.event_type IS NOT NULL ORDER BY a.event_type"
        )
    ]
    tags = [
        FacetCount(name=r["name"], count=r["c"], category=r["category"])
        for r in conn.execute(
            "SELECT t.name AS name, t.category AS category, COUNT(*) AS c FROM tags t "
            "JOIN image_tags it ON it.tag_id = t.id "
            "JOIN images i ON i.id = it.image_id "
            "WHERE i.excluded = 0 GROUP BY t.id ORDER BY c DESC, t.name"
        )
    ]
    persons = [
        FacetPerson(cluster_id=r["cluster_id"], name=r["name"], count=r["c"])
        for r in conn.execute(
            "SELECT p.cluster_id AS cluster_id, p.name AS name, COUNT(f.id) AS c "
            "FROM persons p JOIN faces f ON f.cluster_id = p.cluster_id "
            "JOIN images i ON i.id = f.image_id "
            "WHERE i.excluded = 0 GROUP BY p.cluster_id ORDER BY c DESC, p.name IS NULL, p.name"
        )
    ]
    return Facets(
        years=years, seasons=seasons, places=places, events=events, tags=tags, persons=persons
    )
