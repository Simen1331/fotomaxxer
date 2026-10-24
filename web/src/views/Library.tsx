import { useEffect } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { VirtuosoGrid } from "react-virtuoso";
import { api, type ImageListItem } from "../api/client";
import { useStore } from "../store";
import FilterSidebar from "../components/FilterSidebar";
import Lightbox from "../components/Lightbox";
import ActiveAlbumDrawer from "../components/ActiveAlbumDrawer";

const PAGE = 200;

function formatDate(taken: string | null): string {
  return taken ? taken.slice(0, 10) : "";
}

function Cell({ img }: { img: ImageListItem }) {
  const selected = useStore((s) => s.selected.has(img.id));
  const toggleSelect = useStore((s) => s.toggleSelect);
  const openLightbox = useStore((s) => s.openLightbox);
  const qc = useQueryClient();

  async function exclude(e: React.MouseEvent) {
    e.stopPropagation();
    await api.setExcluded(img.id, !img.excluded);
    qc.invalidateQueries({ queryKey: ["images"] });
    qc.invalidateQueries({ queryKey: ["facets"] });
  }

  return (
    <div
      className={`cell ${selected ? "sel" : ""}`}
      title={img.place_name ?? ""}
      onClick={() => openLightbox(img.id)}
    >
      <img src={img.thumb_url} loading="lazy" alt="" />
      <div className="grad" />
      <button
        className="pick"
        aria-label="Velg"
        onClick={(e) => {
          e.stopPropagation();
          toggleSelect(img.id);
        }}
      >
        ✓
      </button>
      <button className="excl-btn" title={img.excluded ? "Gjeninnsett" : "Ekskludér"} onClick={exclude}>
        {img.excluded ? "↺" : "⊘"}
      </button>
      {img.person_count > 0 && <div className="pers-badge">◐ {img.person_count}</div>}
      <div className="meta">
        {img.place_name && <div className="pl">{img.place_name}</div>}
        <div className="dt">{formatDate(img.taken_at)}</div>
      </div>
    </div>
  );
}

function SelectionBar() {
  const selected = useStore((s) => s.selected);
  const clearSelection = useStore((s) => s.clearSelection);
  const activeAlbumId = useStore((s) => s.activeAlbumId);
  const qc = useQueryClient();
  const count = selected.size;
  if (count === 0) return null;

  async function addToActive() {
    if (activeAlbumId === null) return;
    await api.addToAlbum(activeAlbumId, [...selected]);
    clearSelection();
    qc.invalidateQueries({ queryKey: ["albums"] });
    qc.invalidateQueries({ queryKey: ["album", activeAlbumId] });
  }

  return (
    <div className="selbar show">
      <div className="lbl">
        <b>{count}</b> valgt
      </div>
      <div className="sep" />
      <button
        className="btn-amber"
        onClick={addToActive}
        disabled={activeAlbumId === null}
        title={activeAlbumId === null ? "Sett et aktivt album først" : "Legg i aktivt album"}
      >
        Legg i album
      </button>
      <button className="btn-ghost" onClick={clearSelection}>
        Tøm
      </button>
    </div>
  );
}

export default function Library() {
  const filter = useStore((s) => s.filter);
  const includeExcluded = useStore((s) => s.includeExcluded);
  const setOrderedIds = useStore((s) => s.setOrderedIds);

  const query = { ...filter, include_excluded: includeExcluded };
  const { data, isLoading, isError, error } = useQuery({
    queryKey: ["images", query, PAGE, 0],
    queryFn: () => api.images(query, PAGE, 0),
  });

  const items = data?.items ?? [];
  const total = data?.total ?? 0;

  useEffect(() => {
    setOrderedIds(items.map((i) => i.id));
  }, [items, setOrderedIds]);

  return (
    <div className="library">
      <FilterSidebar />
      <div className="gridwrap">
        <div className="gridhead">
          <h1>Biblioteket</h1>
          <span className="sub">{total} bilder</span>
        </div>
        {isLoading ? (
          <div className="state">Laster biblioteket …</div>
        ) : isError ? (
          <div className="state">Kunne ikke laste biblioteket: {(error as Error).message}</div>
        ) : items.length === 0 ? (
          <div className="state">Ingen bilder her. Juster filteret eller oppdater biblioteket.</div>
        ) : (
          <VirtuosoGrid
            useWindowScroll
            data={items}
            listClassName="grid"
            itemContent={(_index, img) => <Cell img={img} />}
          />
        )}
      </div>
      <ActiveAlbumDrawer />
      <SelectionBar />
      <Lightbox />
    </div>
  );
}
