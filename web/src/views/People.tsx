import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type PersonSummary } from "../api/client";
import { useStore } from "../store";
import { faceCropStyle } from "../lib/facecrop";

// Personer: rutenett av grupperte ansikter. Navngi inline, klikk antall for å
// filtrere biblioteket, eller slå sammen to grupper. Bruker-vendt språk —
// «person»/«gruppe», ikke «klynge».
export default function People() {
  const qc = useQueryClient();
  const navigate = useNavigate();
  const setFilterKey = useStore((s) => s.setFilterKey);
  const clearFilter = useStore((s) => s.clearFilter);

  const { data } = useQuery({ queryKey: ["persons"], queryFn: api.persons });
  const [editing, setEditing] = useState<number | null>(null);
  const [draft, setDraft] = useState("");
  const [mergeFor, setMergeFor] = useState<number | null>(null);

  function invalidate() {
    qc.invalidateQueries({ queryKey: ["persons"] });
    qc.invalidateQueries({ queryKey: ["facets"] });
  }

  function startEdit(p: PersonSummary) {
    setEditing(p.cluster_id);
    setDraft(p.name ?? "");
  }

  async function commitName(clusterId: number) {
    const name = draft.trim() || null;
    setEditing(null);
    await api.renamePerson(clusterId, name);
    invalidate();
  }

  function filterByPerson(clusterId: number) {
    clearFilter();
    setFilterKey("person", clusterId);
    navigate("/");
  }

  async function merge(sourceId: number, intoId: number) {
    setMergeFor(null);
    await api.mergePerson(sourceId, intoId);
    invalidate();
  }

  async function ignore(p: PersonSummary) {
    const label = p.name ?? `denne gruppen (${p.face_count} ansikter)`;
    if (!window.confirm(`Kryss bort ${label}? Ansiktene ignoreres og gruppen kommer ikke tilbake ved ny gruppering.`))
      return;
    await api.ignorePerson(p.cluster_id);
    invalidate();
  }

  const persons = data?.items ?? [];

  return (
    <div className="page">
      <h1>Personer</h1>
      <p className="lead">
        FotoMaxxer har gruppert ansikter den tror er samme person. Gi dem navn én gang, så følger
        navnet med på alle bilder — også nye du legger til senere. Er en gruppe feil? Slå sammen,
        del opp eller flytt et ansikt.
      </p>

      <div className="pgrid">
        {persons.map((p) => (
          <div key={p.cluster_id} className={`pcard ${p.name ? "" : "unnamed"}`}>
            <div
              className="cover"
              style={
                p.cover
                  ? faceCropStyle(p.cover.hash, p.cover.bbox, p.cover.width, p.cover.height)
                  : undefined
              }
              onClick={() => navigate(`/people/${p.cluster_id}`)}
              title="Åpne person"
            >
              <span className="cnt">{p.face_count}</span>
              <button
                className="pcard-ignore"
                title="Kryss bort — tilfeldig/feil gruppe"
                onClick={(e) => {
                  e.stopPropagation();
                  ignore(p);
                }}
              >
                ×
              </button>
            </div>
            <div className="body">
              {editing === p.cluster_id ? (
                <input
                  className="nameField"
                  autoFocus
                  value={draft}
                  placeholder="Navngi person…"
                  onChange={(e) => setDraft(e.target.value)}
                  onBlur={() => commitName(p.cluster_id)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") commitName(p.cluster_id);
                    if (e.key === "Escape") setEditing(null);
                  }}
                />
              ) : (
                <div
                  className={`name ${p.name ? "" : "unnamed"}`}
                  onClick={() => startEdit(p)}
                  title="Klikk for å navngi"
                >
                  {p.name ?? "Uten navn"}
                </div>
              )}
              <div className="sub" onClick={() => filterByPerson(p.cluster_id)}>
                {p.face_count} ansikter · vis bilder
              </div>

              {mergeFor === p.cluster_id ? (
                <select
                  className="nameField"
                  style={{ marginTop: 10, fontFamily: "inherit", fontSize: 12.5 }}
                  autoFocus
                  defaultValue=""
                  onChange={(e) => e.target.value && merge(p.cluster_id, Number(e.target.value))}
                  onBlur={() => setMergeFor(null)}
                >
                  <option value="" disabled>
                    Slå sammen inn i…
                  </option>
                  {persons
                    .filter((o) => o.cluster_id !== p.cluster_id)
                    .map((o) => (
                      <option key={o.cluster_id} value={o.cluster_id}>
                        {o.name ?? `Uten navn (${o.face_count})`}
                      </option>
                    ))}
                </select>
              ) : (
                persons.length > 1 && (
                  <button className="pcard-merge" onClick={() => setMergeFor(p.cluster_id)}>
                    Slå sammen…
                  </button>
                )
              )}
            </div>
          </div>
        ))}
        {data && persons.length === 0 && (
          <div className="state">Ingen personer ennå. Oppdater biblioteket for å finne ansikter.</div>
        )}
      </div>
    </div>
  );
}
