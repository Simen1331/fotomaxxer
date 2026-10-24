import { useStore } from "../store";

// Fritekst-søk i topbar. Søker beskrivelse + stikkord + sted (LIKE, server-side).
export default function SearchBar() {
  const q = useStore((s) => s.filter.q ?? "");
  const setSearch = useStore((s) => s.setSearch);

  return (
    <div className="search">
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
        <circle cx="11" cy="11" r="7" />
        <path d="m21 21-4.3-4.3" />
      </svg>
      <input
        type="search"
        name="q"
        value={q}
        placeholder="Søk i bilder, steder, stikkord …"
        onChange={(e) => setSearch(e.target.value)}
      />
    </div>
  );
}
