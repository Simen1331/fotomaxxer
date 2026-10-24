import { create } from "zustand";
import type { ImageFilter } from "./api/client";

// Klient-state (Zustand): aktivt filter, seleksjons-sett, lightbox og «vis
// ekskluderte». Seleksjon overlever scroll/vy-bytte. Aktivt album legges til i S3.
interface AppState {
  filter: ImageFilter;
  includeExcluded: boolean;

  selected: Set<number>;
  orderedIds: number[]; // gjeldende synlige rekkefølge — for lightbox-navigasjon
  lightboxId: number | null;

  activeAlbumId: number | null; // «aktivt album»-byggemodus — overlever vy-bytte
  setActiveAlbum: (id: number | null) => void;

  setFilterKey: <K extends keyof ImageFilter>(key: K, value: ImageFilter[K]) => void;
  toggleFilterKey: <K extends keyof ImageFilter>(key: K, value: NonNullable<ImageFilter[K]>) => void;
  setSearch: (q: string) => void;
  clearFilter: () => void;
  toggleIncludeExcluded: () => void;

  toggleSelect: (id: number) => void;
  selectMany: (ids: number[]) => void;
  clearSelection: () => void;

  setOrderedIds: (ids: number[]) => void;
  openLightbox: (id: number) => void;
  closeLightbox: () => void;
  stepLightbox: (delta: number) => void;
}

export const useStore = create<AppState>((set, get) => ({
  filter: {},
  includeExcluded: false,

  selected: new Set<number>(),
  orderedIds: [],
  lightboxId: null,

  activeAlbumId: null,
  setActiveAlbum: (id) => set({ activeAlbumId: id }),

  setFilterKey: (key, value) =>
    set((s) => {
      const filter = { ...s.filter };
      if (value === undefined || value === null || value === "") delete filter[key];
      else filter[key] = value;
      return { filter };
    }),

  // Klikk på samme verdi slår filteret av igjen (toggle-semantikk som mockup-chips).
  toggleFilterKey: (key, value) =>
    set((s) => {
      const filter = { ...s.filter };
      if (filter[key] === value) delete filter[key];
      else filter[key] = value;
      return { filter };
    }),

  setSearch: (q) =>
    set((s) => {
      const filter = { ...s.filter };
      if (q.trim()) filter.q = q.trim();
      else delete filter.q;
      return { filter };
    }),

  clearFilter: () => set({ filter: {} }),
  toggleIncludeExcluded: () => set((s) => ({ includeExcluded: !s.includeExcluded })),

  toggleSelect: (id) =>
    set((s) => {
      const selected = new Set(s.selected);
      if (selected.has(id)) selected.delete(id);
      else selected.add(id);
      return { selected };
    }),

  selectMany: (ids) =>
    set((s) => {
      const selected = new Set(s.selected);
      ids.forEach((id) => selected.add(id));
      return { selected };
    }),

  clearSelection: () => set({ selected: new Set<number>() }),

  setOrderedIds: (ids) => set({ orderedIds: ids }),
  openLightbox: (id) => set({ lightboxId: id }),
  closeLightbox: () => set({ lightboxId: null }),

  stepLightbox: (delta) => {
    const { orderedIds, lightboxId } = get();
    if (lightboxId === null) return;
    const i = orderedIds.indexOf(lightboxId);
    if (i === -1) return;
    const next = i + delta;
    if (next < 0 || next >= orderedIds.length) return;
    set({ lightboxId: orderedIds[next] });
  },
}));
