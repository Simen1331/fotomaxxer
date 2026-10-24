import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { api } from "../api/client";

// Mappe-velger for «Legg til mappe». To måter:
//  1) Velg gjeldende mappa som én kilde (bla deg inn, «Velg denne mappa»).
//  2) Hak av flere undermapper og legg dem til som hver sin kilde — med valgfri
//     tagging-profil per rad. Da registreres løvmappene, IKKE forelderen.
export default function FolderPicker({
  onPick,
  onPickMany,
  onClose,
}: {
  onPick: (path: string) => void;
  onPickMany?: (rows: { path: string; profile_id: number | null }[]) => void;
  onClose: () => void;
}) {
  const [path, setPath] = useState<string | undefined>(undefined);
  const [manual, setManual] = useState("");
  // path → valgt profil-id for avhukede undermapper. null = standard-profil.
  const [picked, setPicked] = useState<Map<string, number | null>>(new Map());
  const { data, isLoading, error } = useQuery({
    queryKey: ["fs", path ?? "~"],
    queryFn: () => api.browse(path),
  });
  const { data: profilesData } = useQuery({ queryKey: ["profiles"], queryFn: api.profiles });
  const profiles = profilesData?.items ?? [];

  function useManual() {
    const p = manual.trim();
    if (p) onPick(p);
  }

  function toggle(p: string) {
    setPicked((prev) => {
      const next = new Map(prev);
      if (next.has(p)) next.delete(p);
      else next.set(p, null);
      return next;
    });
  }

  function setProfile(p: string, profileId: number | null) {
    setPicked((prev) => new Map(prev).set(p, profileId));
  }

  const multi = picked.size > 0 && onPickMany;

  function addPicked() {
    if (!onPickMany) return;
    const rows = [...picked].map(([p, pid]) => ({ path: p, profile_id: pid }));
    if (rows.length) onPickMany(rows);
  }

  return (
    <div className="fsoverlay" onClick={onClose}>
      <div className="fspick" onClick={(e) => e.stopPropagation()}>
        <div className="fshead">
          <b>Velg mappe</b>
          <span className="mono">{data?.path ?? "…"}</span>
        </div>

        {onPickMany && (
          <div className="fshint">
            Bla deg inn i en mappe, så kan du hake av undermappene du vil ha — hver blir en egen
            kilde. Vil du gi en mappe en egen tagging-profil, velg den i nedtrekket som dukker opp.
          </div>
        )}

        <div className="fslist">
          {data?.parent && (
            <button className="fsrow up" onClick={() => setPath(data.parent ?? undefined)}>
              ↑ opp ett nivå
            </button>
          )}
          {isLoading && <div className="state">Laster …</div>}
          {error && <div className="jobmsg err">{String(error)}</div>}
          {data?.entries.map((e) => {
            const isPicked = picked.has(e.path);
            return (
              <div key={e.path} className={`fsrow tree${isPicked ? " picked" : ""}`}>
                {onPickMany && (
                  <input
                    type="checkbox"
                    className="fscheck"
                    checked={isPicked}
                    onChange={() => toggle(e.path)}
                    onClick={(ev) => ev.stopPropagation()}
                    title="Legg til som egen kilde"
                  />
                )}
                <button className="fsnav" onClick={() => setPath(e.path)}>
                  <span className="ic">📁</span> {e.name}
                </button>
                {onPickMany && isPicked && (
                  <select
                    className="inp fsprof"
                    value={picked.get(e.path) ?? ""}
                    onChange={(ev) =>
                      setProfile(e.path, ev.target.value ? Number(ev.target.value) : null)
                    }
                    onClick={(ev) => ev.stopPropagation()}
                  >
                    <option value="">Standard-profil</option>
                    {profiles.map((p) => (
                      <option key={p.id} value={p.id}>
                        {p.name}
                      </option>
                    ))}
                  </select>
                )}
              </div>
            );
          })}
          {data && data.entries.length === 0 && !isLoading && (
            <div className="state">Ingen undermapper her.</div>
          )}
        </div>

        <div className="inprow">
          <input
            className="inp mono"
            placeholder="eller lim inn sti"
            value={manual}
            onChange={(e) => setManual(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && useManual()}
          />
          <button className="pastebtn" onClick={useManual} disabled={!manual.trim()}>
            Bruk sti
          </button>
        </div>

        <div className="fsfoot">
          {multi ? (
            <button className="bigbtn" onClick={addPicked}>
              Legg til {picked.size} valgte {picked.size === 1 ? "mappe" : "mapper"}
            </button>
          ) : (
            <button
              className="bigbtn"
              disabled={!data?.path}
              onClick={() => data?.path && onPick(data.path)}
            >
              Velg denne mappa
            </button>
          )}
          <button className="pastebtn" onClick={onClose}>
            Avbryt
          </button>
        </div>
      </div>
    </div>
  );
}
