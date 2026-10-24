"""Album: rene DB-koblinger (``album_items``).

**Jernregel: å legge til et bilde kopierer ALDRI en fil og lager ingen mappe.**
Tillegg/fjerning/omorganisering er ren database. Den eneste fil-operasjonen er
«Eksporter til mappe» (``POST /api/export``, egen router). Et bilde kan ligge i
flere album. Album viser alle medlemmer — også ekskluderte — siden brukeren la dem
inn bevisst.
"""

from __future__ import annotations

import sqlite3

from fastapi import APIRouter, Depends, HTTPException

from api.deps import get_conn
from api.schemas import (
    AlbumAddItems,
    AlbumCreate,
    AlbumDetail,
    AlbumItem,
    AlbumList,
    AlbumReorder,
    AlbumSummary,
)

router = APIRouter(tags=["albums"])


def _thumb_url(content_hash: str) -> str:
    return f"/derived/{content_hash}_300.jpg"


def _require_album(conn: sqlite3.Connection, album_id: int) -> sqlite3.Row:
    row = conn.execute("SELECT id, name FROM albums WHERE id = ?", (album_id,)).fetchone()
    if row is None:
        raise HTTPException(status_code=404, detail={"error": "not_found", "message": "album finnes ikke"})
    return row


@router.get("/albums", response_model=AlbumList)
def list_albums(conn: sqlite3.Connection = Depends(get_conn)) -> AlbumList:
    rows = conn.execute(
        """
        SELECT a.id, a.name, a.created_at,
               (SELECT COUNT(*) FROM album_items ai WHERE ai.album_id = a.id) AS item_count
        FROM albums a
        ORDER BY a.created_at DESC, a.id DESC
        """
    ).fetchall()
    return AlbumList(
        items=[
            AlbumSummary(id=r["id"], name=r["name"], item_count=r["item_count"], created_at=r["created_at"])
            for r in rows
        ]
    )


@router.post("/albums", response_model=AlbumSummary)
def create_album(body: AlbumCreate, conn: sqlite3.Connection = Depends(get_conn)) -> AlbumSummary:
    name = body.name.strip()
    if not name:
        raise HTTPException(status_code=400, detail={"error": "bad_request", "message": "navn kan ikke være tomt"})
    cur = conn.execute("INSERT INTO albums (name) VALUES (?)", (name,))
    conn.commit()
    row = conn.execute(
        "SELECT id, name, created_at FROM albums WHERE id = ?", (cur.lastrowid,)
    ).fetchone()
    return AlbumSummary(id=row["id"], name=row["name"], item_count=0, created_at=row["created_at"])


@router.get("/albums/{album_id}", response_model=AlbumDetail)
def get_album(album_id: int, conn: sqlite3.Connection = Depends(get_conn)) -> AlbumDetail:
    album = _require_album(conn, album_id)
    rows = conn.execute(
        """
        SELECT ai.image_id, ai.position, i.content_hash AS hash
        FROM album_items ai JOIN images i ON i.id = ai.image_id
        WHERE ai.album_id = ?
        ORDER BY ai.position, ai.image_id
        """,
        (album_id,),
    ).fetchall()
    items = [
        AlbumItem(
            image_id=r["image_id"],
            hash=r["hash"],
            thumb_url=_thumb_url(r["hash"]),
            position=r["position"],
        )
        for r in rows
    ]
    return AlbumDetail(id=album["id"], name=album["name"], items=items)


@router.post("/albums/{album_id}/items", response_model=AlbumDetail)
def add_items(
    album_id: int, body: AlbumAddItems, conn: sqlite3.Connection = Depends(get_conn)
) -> AlbumDetail:
    """Legg til bilder (idempotent; appender etter siste position)."""
    _require_album(conn, album_id)
    last = conn.execute(
        "SELECT COALESCE(MAX(position), -1) FROM album_items WHERE album_id = ?", (album_id,)
    ).fetchone()[0]
    pos = last + 1
    for image_id in body.image_ids:
        already = conn.execute(
            "SELECT 1 FROM album_items WHERE album_id = ? AND image_id = ?", (album_id, image_id)
        ).fetchone()
        if already:
            continue  # idempotent — alt i albumet, hopp over
        # Valider at bildet finnes (FK ville feilet, men gi pen feil).
        exists = conn.execute("SELECT 1 FROM images WHERE id = ?", (image_id,)).fetchone()
        if not exists:
            continue
        conn.execute(
            "INSERT INTO album_items (album_id, image_id, position) VALUES (?, ?, ?)",
            (album_id, image_id, pos),
        )
        pos += 1
    conn.commit()
    return get_album(album_id, conn)


@router.delete("/albums/{album_id}/items/{image_id}", response_model=AlbumDetail)
def remove_item(
    album_id: int, image_id: int, conn: sqlite3.Connection = Depends(get_conn)
) -> AlbumDetail:
    _require_album(conn, album_id)
    conn.execute(
        "DELETE FROM album_items WHERE album_id = ? AND image_id = ?", (album_id, image_id)
    )
    conn.commit()
    return get_album(album_id, conn)


@router.patch("/albums/{album_id}/order", response_model=AlbumDetail)
def reorder(
    album_id: int, body: AlbumReorder, conn: sqlite3.Connection = Depends(get_conn)
) -> AlbumDetail:
    """Sett ny rekkefølge fra rekkefølgen i ``image_ids``."""
    _require_album(conn, album_id)
    for position, image_id in enumerate(body.image_ids):
        conn.execute(
            "UPDATE album_items SET position = ? WHERE album_id = ? AND image_id = ?",
            (position, album_id, image_id),
        )
    conn.commit()
    return get_album(album_id, conn)


@router.delete("/albums/{album_id}")
def delete_album(album_id: int, conn: sqlite3.Connection = Depends(get_conn)) -> dict[str, int]:
    """Slett album (ikke bildene; ``album_items`` cascade-slettes)."""
    _require_album(conn, album_id)
    conn.execute("DELETE FROM albums WHERE id = ?", (album_id,))
    conn.commit()
    return {"deleted": album_id}
