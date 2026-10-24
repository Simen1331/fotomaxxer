import { useState } from "react";
import { Link } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";
import { useStore } from "../store";

// Album-oversikt: liste + opprett. Album er vedvarende prosjekter; flere parallelt.
export default function Albums() {
  const qc = useQueryClient();
  const setActiveAlbum = useStore((s) => s.setActiveAlbum);
  const activeAlbumId = useStore((s) => s.activeAlbumId);
  const [newName, setNewName] = useState("");

  const { data } = useQuery({ queryKey: ["albums"], queryFn: api.albums });

  async function create() {
    const name = newName.trim();
    if (!name) return;
    const a = await api.createAlbum(name);
    setNewName("");
    setActiveAlbum(a.id);
    qc.invalidateQueries({ queryKey: ["albums"] });
  }

  return (
    <div className="gridwrap">
      <div className="gridhead">
        <h1>Album</h1>
        <span className="sub">{data?.items.length ?? 0} album</span>
      </div>

      <div className="album-new">
        <input
          name="album_name"
          value={newName}
          placeholder="Navn på nytt album …"
          onChange={(e) => setNewName(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && create()}
        />
        <button className="btn-amber" onClick={create} disabled={!newName.trim()}>
          ＋ Nytt album
        </button>
      </div>

      <div className="album-list">
        {(data?.items ?? []).map((a) => (
          <div key={a.id} className={`album-card ${activeAlbumId === a.id ? "active" : ""}`}>
            <Link to={`/albums/${a.id}`} className="album-card-main">
              <div className="album-name">{a.name}</div>
              <div className="album-count">{a.item_count} bilder</div>
            </Link>
            <button
              className="album-setactive"
              onClick={() => setActiveAlbum(activeAlbumId === a.id ? null : a.id)}
            >
              {activeAlbumId === a.id ? "Aktivt ✓" : "Sett aktivt"}
            </button>
          </div>
        ))}
        {data && data.items.length === 0 && (
          <div className="state">Ingen album ennå. Lag ett over.</div>
        )}
      </div>
    </div>
  );
}
