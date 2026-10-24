import { useEffect, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";
import { useStore } from "../store";
import { arrayMove } from "../lib/reorder";

// Album-detalj: drag-omorganisering + «Eksporter til mappe» (eneste fil-op) +
// «Lag PDF» (peker til kommende eksport-TB — gjør intet her).
export default function AlbumDetail() {
  const { id } = useParams();
  const albumId = Number(id);
  const navigate = useNavigate();
  const qc = useQueryClient();
  const setActiveAlbum = useStore((s) => s.setActiveAlbum);
  const activeAlbumId = useStore((s) => s.activeAlbumId);

  const [order, setOrder] = useState<number[]>([]);
  const [dragIdx, setDragIdx] = useState<number | null>(null);
  const [dest, setDest] = useState("");
  const [keepNames, setKeepNames] = useState(false);
  const [exportMsg, setExportMsg] = useState<string | null>(null);

  const { data, isError } = useQuery({
    queryKey: ["album", albumId],
    queryFn: () => api.album(albumId),
  });

  useEffect(() => {
    if (data) setOrder(data.items.map((i) => i.image_id));
  }, [data]);

  function invalidate() {
    qc.invalidateQueries({ queryKey: ["album", albumId] });
    qc.invalidateQueries({ queryKey: ["albums"] });
  }

  async function commitOrder(next: number[]) {
    setOrder(next);
    await api.reorderAlbum(albumId, next);
    invalidate();
  }

  async function removeItem(imageId: number) {
    await api.removeFromAlbum(albumId, imageId);
    invalidate();
  }

  async function doExport() {
    setExportMsg(null);
    try {
      await api.exportAlbum({
        album_id: albumId,
        dest_path: dest.trim() || undefined,
        keep_original_names: keepNames,
      });
      setExportMsg("Eksport startet — kopierer originaler til mappe.");
    } catch (e) {
      setExportMsg((e as Error).message);
    }
  }

  async function removeAlbum() {
    await api.deleteAlbum(albumId);
    if (activeAlbumId === albumId) setActiveAlbum(null);
    qc.invalidateQueries({ queryKey: ["albums"] });
    navigate("/albums");
  }

  if (isError) return <div className="state">Fant ikke albumet.</div>;
  if (!data) return <div className="state">Laster …</div>;

  const itemsById = new Map(data.items.map((i) => [i.image_id, i]));

  return (
    <div className="gridwrap">
      <div className="gridhead">
        <Link to="/albums" className="back-link">
          ← Album
        </Link>
        <h1>{data.name}</h1>
        <span className="sub">{data.items.length} bilder</span>
      </div>

      <div className="album-actions">
        <button
          className={activeAlbumId === albumId ? "btn-amber" : "btn-ghost"}
          onClick={() => setActiveAlbum(activeAlbumId === albumId ? null : albumId)}
        >
          {activeAlbumId === albumId ? "Aktivt album ✓" : "Sett som aktivt"}
        </button>
        <button className="btn-ghost" onClick={removeAlbum}>
          Slett album
        </button>
      </div>

      <div className="album-export">
        <input
          name="dest_path"
          value={dest}
          placeholder="Mål-mappe (tom = standard fra Innstillinger)"
          onChange={(e) => setDest(e.target.value)}
        />
        <label className="export-keep">
          <input type="checkbox" checked={keepNames} onChange={(e) => setKeepNames(e.target.checked)} />
          Behold originalnavn
        </label>
        <button className="btn-amber" onClick={doExport} disabled={data.items.length === 0}>
          Eksporter til mappe
        </button>
        <button className="btn-ghost" disabled title="Kommer som egen funksjon">
          Lag PDF
        </button>
      </div>
      {exportMsg && <div className="export-msg">{exportMsg}</div>}

      <div className="album-grid">
        {order.map((imageId, idx) => {
          const item = itemsById.get(imageId);
          if (!item) return null;
          return (
            <div
              key={imageId}
              className={`album-cell ${dragIdx === idx ? "dragging" : ""}`}
              draggable
              onDragStart={() => setDragIdx(idx)}
              onDragOver={(e) => {
                e.preventDefault();
                if (dragIdx === null || dragIdx === idx) return;
                setOrder((o) => arrayMove(o, dragIdx, idx));
                setDragIdx(idx);
              }}
              onDragEnd={() => {
                setDragIdx(null);
                commitOrder(order);
              }}
            >
              <span className="album-num">{idx + 1}</span>
              <img src={item.thumb_url} alt="" />
              <button className="album-rm" onClick={() => removeItem(imageId)} title="Fjern">
                ✕
              </button>
            </div>
          );
        })}
        {data.items.length === 0 && <div className="state">Tomt album. Legg til bilder fra biblioteket.</div>}
      </div>
    </div>
  );
}
