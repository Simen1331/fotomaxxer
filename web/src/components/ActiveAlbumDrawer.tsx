import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";
import { useStore } from "../store";
import { arrayMove } from "../lib/reorder";

// «Aktivt album»-byggemodus: en sammenleggbar side-skuff. Velg ett album aktivt,
// valgte bilder går dit med ett klikk, og albumet vises vokse med drag-rekkefølge.
// Inline «＋ Nytt album». Tillegg/omorganisering er ren DB — ingen fil røres.
export default function ActiveAlbumDrawer() {
  const qc = useQueryClient();
  const activeAlbumId = useStore((s) => s.activeAlbumId);
  const setActiveAlbum = useStore((s) => s.setActiveAlbum);
  const selected = useStore((s) => s.selected);
  const clearSelection = useStore((s) => s.clearSelection);

  const [collapsed, setCollapsed] = useState(false);
  const [newName, setNewName] = useState("");
  const [order, setOrder] = useState<number[]>([]);
  const [dragIdx, setDragIdx] = useState<number | null>(null);

  const { data: albums } = useQuery({ queryKey: ["albums"], queryFn: api.albums });
  const { data: active } = useQuery({
    queryKey: ["album", activeAlbumId],
    queryFn: () => api.album(activeAlbumId!),
    enabled: activeAlbumId !== null,
  });

  useEffect(() => {
    if (active) setOrder(active.items.map((i) => i.image_id));
  }, [active]);

  function invalidate() {
    qc.invalidateQueries({ queryKey: ["albums"] });
    qc.invalidateQueries({ queryKey: ["album", activeAlbumId] });
  }

  async function createAlbum() {
    const name = newName.trim();
    if (!name) return;
    const a = await api.createAlbum(name);
    setNewName("");
    setActiveAlbum(a.id);
    qc.invalidateQueries({ queryKey: ["albums"] });
  }

  async function addSelected() {
    if (activeAlbumId === null || selected.size === 0) return;
    await api.addToAlbum(activeAlbumId, [...selected]);
    clearSelection();
    invalidate();
  }

  async function removeItem(imageId: number) {
    if (activeAlbumId === null) return;
    await api.removeFromAlbum(activeAlbumId, imageId);
    invalidate();
  }

  async function commitOrder(next: number[]) {
    setOrder(next);
    if (activeAlbumId !== null) {
      await api.reorderAlbum(activeAlbumId, next);
      invalidate();
    }
  }

  if (collapsed) {
    return (
      <div className="aad collapsed">
        <button className="aad-tab" onClick={() => setCollapsed(false)} title="Vis aktivt album">
          Album ▸
        </button>
      </div>
    );
  }

  const itemsById = new Map(active?.items.map((i) => [i.image_id, i]) ?? []);

  return (
    <aside className="aad">
      <div className="aad-head">
        <h3>Aktivt album</h3>
        <button className="aad-x" onClick={() => setCollapsed(true)} title="Skjul">
          ◂
        </button>
      </div>

      <select
        className="aad-select"
        value={activeAlbumId ?? ""}
        onChange={(e) => setActiveAlbum(e.target.value ? Number(e.target.value) : null)}
      >
        <option value="">Velg album …</option>
        {albums?.items.map((a) => (
          <option key={a.id} value={a.id}>
            {a.name} ({a.item_count})
          </option>
        ))}
      </select>

      <div className="aad-new">
        <input
          name="new_album_name"
          value={newName}
          placeholder="＋ Nytt album"
          onChange={(e) => setNewName(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && createAlbum()}
        />
        <button onClick={createAlbum} disabled={!newName.trim()}>
          Lag
        </button>
      </div>

      {activeAlbumId !== null && (
        <>
          <button className="aad-add btn-amber" onClick={addSelected} disabled={selected.size === 0}>
            Legg til {selected.size} valgte
          </button>

          <div className="aad-list">
            {order.length === 0 ? (
              <div className="aad-empty">Tomt — velg bilder og legg dem til.</div>
            ) : (
              order.map((imageId, idx) => {
                const item = itemsById.get(imageId);
                if (!item) return null;
                return (
                  <div
                    key={imageId}
                    className={`aad-item ${dragIdx === idx ? "dragging" : ""}`}
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
                    <span className="aad-num">{idx + 1}</span>
                    <img src={item.thumb_url} alt="" />
                    <button className="aad-rm" onClick={() => removeItem(imageId)} title="Fjern">
                      ✕
                    </button>
                  </div>
                );
              })
            )}
          </div>

          <Link className="aad-open" to={`/albums/${activeAlbumId}`}>
            Åpne album →
          </Link>
        </>
      )}
    </aside>
  );
}
