import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { api, type Facets } from "../api/client";
import { useStore } from "../store";

type TagFacet = Facets["tags"][number];

// Topp-N stikkord per kategori-gruppe før «vis flere» slår inn.
const TAG_SHOW_N = 12;

interface TagGroupData {
  category: string | null;
  tags: TagFacet[];
}

// Grupper stikkord på kategori. Stikkordene kommer count-sortert fra backend;
// kategori-rekkefølgen følger første forekomst (= høyeste antall), «Andre» sist.
function groupTags(tags: TagFacet[]): TagGroupData[] {
  const order: (string | null)[] = [];
  const byCat = new Map<string | null, TagFacet[]>();
  for (const t of tags) {
    const cat = t.category ?? null;
    if (!byCat.has(cat)) {
      byCat.set(cat, []);
      order.push(cat);
    }
    byCat.get(cat)!.push(t);
  }
  order.sort((a, b) => (a === null ? 1 : 0) - (b === null ? 1 : 0)); // null («Andre») sist
  return order.map((category) => ({ category, tags: byCat.get(category)! }));
}

const SEASON_DOT: Record<string, string> = {
  vinter: "var(--vinter)",
  vår: "var(--var)",
  sommer: "var(--sommer)",
  høst: "var(--host)",
};

// Filter-sidebar: facets som chips/rader. Klikk toggler filteret av/på.
// «Vis ekskluderte» styrer om mykt-skjulte bilder vises. Bruker-vendt språk —
// «person»/«gruppe», ikke «klynge».
export default function FilterSidebar() {
  const { data: facets } = useQuery({ queryKey: ["facets"], queryFn: api.facets });
  const filter = useStore((s) => s.filter);
  const toggle = useStore((s) => s.toggleFilterKey);
  const clearFilter = useStore((s) => s.clearFilter);
  const includeExcluded = useStore((s) => s.includeExcluded);
  const toggleIncludeExcluded = useStore((s) => s.toggleIncludeExcluded);

  const activeCount = Object.keys(filter).filter((k) => k !== "q").length;

  return (
    <aside className="sidebar">
      <h2>Filter</h2>
      <div className="count">
        {activeCount > 0 ? (
          <button className="clear-link" onClick={clearFilter}>
            Nullstill ({activeCount})
          </button>
        ) : (
          "Ingen aktive filtre"
        )}
      </div>

      {facets && facets.years.length > 0 && (
        <div className="fgroup">
          <h3>År</h3>
          <div className="chips">
            {facets.years.map((y) => (
              <button
                key={y}
                className={`chip ${filter.year === y ? "on" : ""}`}
                onClick={() => toggle("year", y)}
              >
                {y}
              </button>
            ))}
          </div>
        </div>
      )}

      {facets && facets.seasons.length > 0 && (
        <div className="fgroup">
          <h3>Sesong</h3>
          <div className="flist">
            {facets.seasons.map((s) => (
              <div
                key={s}
                className={`frow ${filter.season === s ? "on" : ""}`}
                onClick={() => toggle("season", s)}
              >
                <span className="dot" style={{ background: SEASON_DOT[s] ?? "var(--ink-3)" }} />
                {s}
              </div>
            ))}
          </div>
        </div>
      )}

      {facets && facets.places.length > 0 && (
        <div className="fgroup">
          <h3>Sted</h3>
          <div className="flist">
            {facets.places.map((p) => (
              <div
                key={p}
                className={`frow ${filter.place === p ? "on" : ""}`}
                onClick={() => toggle("place", p)}
              >
                {p}
              </div>
            ))}
          </div>
        </div>
      )}

      {facets && facets.events.length > 0 && (
        <div className="fgroup">
          <h3>Hva</h3>
          <div className="chips">
            {facets.events.map((e) => (
              <button
                key={e}
                className={`chip ${filter.event === e ? "on" : ""}`}
                onClick={() => toggle("event", e)}
              >
                {e}
              </button>
            ))}
          </div>
        </div>
      )}

      {facets && facets.persons.length > 0 && (
        <div className="fgroup">
          <h3>Personer</h3>
          <div className="flist">
            {facets.persons.map((p) => (
              <div
                key={p.cluster_id}
                className={`frow ${filter.person === p.cluster_id ? "on" : ""}`}
                onClick={() => toggle("person", p.cluster_id)}
              >
                {p.name ?? "Uten navn"}
                <span className="n">{p.count}</span>
              </div>
            ))}
          </div>
        </div>
      )}

      {facets && facets.tags.length > 0 && (
        <div className="fgroup">
          <h3>Stikkord</h3>
          {(() => {
            const groups = groupTags(facets.tags);
            const lone = groups.length === 1; // ingen kategorier → skjul «Andre»-overskrift
            return groups.map((g) => (
              <TagGroup
                key={g.category ?? "__andre"}
                group={g}
                showHeader={!lone}
                activeTag={filter.tag}
                onToggle={(name) => toggle("tag", name)}
              />
            ));
          })()}
        </div>
      )}

      <div className="fgroup">
        <label className="excl-toggle">
          <input type="checkbox" checked={includeExcluded} onChange={toggleIncludeExcluded} />
          Vis ekskluderte
        </label>
      </div>
    </aside>
  );
}

// Én kategori-gruppe stikkord med topp-N + «vis flere»-utvider. Ukategoriserte
// kommer under «Andre»; har biblioteket ingen kategorier vises ingen overskrift.
function TagGroup({
  group,
  showHeader,
  activeTag,
  onToggle,
}: {
  group: TagGroupData;
  showHeader: boolean;
  activeTag: string | undefined;
  onToggle: (name: string) => void;
}) {
  const [expanded, setExpanded] = useState(false);
  const shown = expanded ? group.tags : group.tags.slice(0, TAG_SHOW_N);
  const hidden = group.tags.length - shown.length;
  return (
    <div className="tagcatgroup">
      {showHeader && <div className="tagcat">{group.category ?? "Andre"}</div>}
      <div className="chips">
        {shown.map((t) => (
          <button
            key={t.name}
            className={`chip ${activeTag === t.name ? "on" : ""}`}
            onClick={() => onToggle(t.name)}
          >
            {t.name}
            <span className="n">{t.count}</span>
          </button>
        ))}
      </div>
      {group.tags.length > TAG_SHOW_N && (
        <button className="morelink" onClick={() => setExpanded((v) => !v)}>
          {expanded ? "vis færre" : `vis ${hidden} til`}
        </button>
      )}
    </div>
  );
}
