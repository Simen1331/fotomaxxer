import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";
import ProfilesSettings from "../components/ProfilesSettings";

// Innstillinger: nøkkel (maskert + lim inn), modell, ansikts-terskler, std.
// eksport-mappe, datamappe-flytt (bootstrap-peker), klargjør-for-deling.
const MODELS = [
  "google/gemini-2.5-flash",
  "google/gemini-2.5-pro",
  "anthropic/claude-haiku-4.5",
];

export default function Settings() {
  const qc = useQueryClient();
  const { data: s } = useQuery({ queryKey: ["settings"], queryFn: api.settings });
  const { data: dd } = useQuery({ queryKey: ["dataDir"], queryFn: api.dataDir });

  const [keyDraft, setKeyDraft] = useState("");
  const [pasting, setPasting] = useState(false);
  const [thr, setThr] = useState(50);
  const [minClu, setMinClu] = useState(3);
  const [exportDir, setExportDir] = useState("");
  const [delSrc, setDelSrc] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);

  useEffect(() => {
    if (!s) return;
    setThr(Math.round(Number(s.face_match_threshold ?? "0.5") * 100));
    setMinClu(Number(s.face_min_cluster_size ?? "3"));
    setExportDir(s.default_export_dir ?? "");
  }, [s]);

  async function save(key: string, value: string, note?: string) {
    await api.putSetting(key, value);
    qc.invalidateQueries({ queryKey: ["settings"] });
    if (note) setMsg(note);
  }

  // «Lim inn» leser faktisk utklippstavla og fyller feltet (localhost = sikker
  // kontekst). Slår det feil, åpnes et tomt felt så brukeren kan Ctrl+V manuelt.
  async function startPaste() {
    setPasting(true);
    try {
      const text = await navigator.clipboard.readText();
      if (text && text.trim()) setKeyDraft(text.trim());
    } catch {
      /* utklippstavle utilgjengelig — manuell innliming i feltet */
    }
  }

  async function saveKey() {
    const k = keyDraft.trim();
    if (!k) {
      setMsg("Feltet er tomt — lim inn nøkkelen først.");
      return;
    }
    setKeyDraft("");
    setPasting(false);
    try {
      await save("openrouter_api_key", k, "Nøkkel lagret (vises maskert).");
    } catch (e) {
      setMsg(`Kunne ikke lagre nøkkelen: ${String(e)}`);
    }
  }

  const models = s?.ai_model && !MODELS.includes(s.ai_model) ? [s.ai_model, ...MODELS] : MODELS;

  return (
    <div className="page" style={{ maxWidth: 720 }}>
      <h1>Innstillinger</h1>
      <p className="lead">
        Legges inn én gang. Nøkkelen lagres lokalt i pakka, ikke i en miljøfil — så hele mappa kan
        flyttes eller deles.
      </p>
      {msg && <div className="jobmsg">{msg}</div>}

      <div className="setgroup">
        <h3>AI-merking</h3>
        <div className="field">
          <label>OpenRouter API-nøkkel</label>
          {pasting ? (
            <div className="inprow">
              <input
                className="inp mono"
                autoFocus
                placeholder="sk-or-…"
                value={keyDraft}
                onChange={(e) => setKeyDraft(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && saveKey()}
              />
              <button className="pastebtn" onClick={saveKey}>
                Lagre
              </button>
              <button className="pastebtn" onClick={() => setPasting(false)}>
                Avbryt
              </button>
            </div>
          ) : (
            <div className="inprow">
              <input className="inp mono" type="text" value={s?.openrouter_api_key ?? ""} readOnly />
              <button className="pastebtn" onClick={startPaste}>
                Lim inn
              </button>
            </div>
          )}
          <div className="desc">Brukes til vision-merking. Får du en ny? Lim inn over. Vises maskert.</div>
        </div>
        <div className="field">
          <label>Modell</label>
          <select
            className="inp"
            value={s?.ai_model ?? ""}
            onChange={(e) => save("ai_model", e.target.value)}
          >
            {models.map((m) => (
              <option key={m} value={m}>
                {m}
              </option>
            ))}
          </select>
          <div className="desc">Billig og sterk på vision er standard. Bytt fritt — det logges per bilde.</div>
        </div>
        <ProfilesSettings defaultProfileId={s?.default_profile_id ?? null} />
      </div>

      <div className="setgroup">
        <h3>Personer &amp; ansikter</h3>
        <div className="field">
          <label>Hvor strengt skal ansikter grupperes til samme person?</label>
          <div className="slider">
            <span style={{ fontSize: 11.5, color: "var(--ink-3)" }}>løsere</span>
            <input
              type="range"
              min={30}
              max={70}
              value={thr}
              onChange={(e) => setThr(Number(e.target.value))}
              onPointerUp={() => save("face_match_threshold", (thr / 100).toFixed(2))}
            />
            <span style={{ fontSize: 11.5, color: "var(--ink-3)" }}>strengere</span>
            <span className="val">{(thr / 100).toFixed(2)}</span>
          </div>
          <div className="desc">
            Strengere = færre feil, men samme person kan bli delt i flere grupper du må slå sammen.
          </div>
        </div>
        <div className="field">
          <label>Hvor mange ansikter før en ny person foreslås?</label>
          <div className="slider">
            <input
              type="range"
              min={2}
              max={8}
              value={minClu}
              onChange={(e) => setMinClu(Number(e.target.value))}
              onPointerUp={() => save("face_min_cluster_size", String(minClu))}
            />
            <span className="val">{minClu}</span>
          </div>
          <div className="desc">Hindrer at ett tilfeldig ansikt blir en egen person.</div>
        </div>
      </div>

      <div className="setgroup">
        <h3>Eksport</h3>
        <div className="field">
          <label>Standard eksport-mappe</label>
          <div className="inprow">
            <input
              className="inp mono"
              type="text"
              placeholder="~/Bilder/Album"
              value={exportDir}
              onChange={(e) => setExportDir(e.target.value)}
              onBlur={() => exportDir !== (s?.default_export_dir ?? "") && save("default_export_dir", exportDir)}
            />
          </div>
          <div className="desc">Hit eksporteres album. Filer nummereres etter rekkefølge: 001_…, 002_…</div>
        </div>
      </div>

      <div className="setgroup">
        <h3>Datamappe</h3>
        <div className="field">
          <label>Hvor databasen og arbeidskopiene lagres</label>
          <div className="inprow">
            <input className="inp mono" type="text" value={dd?.data_dir ?? ""} readOnly />
            <button
              className="pastebtn"
              onClick={async () => {
                const np = window.prompt("Flytt datamappa til (full sti):", dd?.data_dir ?? "");
                if (!np || !np.trim()) return;
                if (delSrc && !window.confirm(`Slette den gamle mappa etter flytt?\n\n${dd?.data_dir ?? ""}\n\nKopien lages først; den gamle slettes bare hvis kopien lyktes.`)) return;
                try {
                  await api.moveDataDir(np.trim(), delSrc);
                  qc.invalidateQueries({ queryKey: ["dataDir"] });
                  setMsg(delSrc ? "Datamappa er flyttet, gammel mappe slettet." : "Datamappa er flyttet (gammel mappe beholdt).");
                } catch (e) {
                  setMsg(String(e));
                }
              }}
            >
              Flytt…
            </button>
          </div>
          <label className="checkrow">
            <input type="checkbox" checked={delSrc} onChange={(e) => setDelSrc(e.target.checked)} />
            <span>Slett gammel mappe etter flytt</span>
          </label>
          <div className="desc">
            Inneholder <span className="mono">fotomaxxer.db</span> + <span className="mono">derived/</span>. Det
            lages en kopi på den nye plassen — originalbildene dine røres ikke. Den gamle mappa beholdes
            som standard; kryss av over for å slette den når kopien er trygt på plass.
          </div>
        </div>
      </div>

      <div className="setgroup danger">
        <h3>Deling</h3>
        <div className="field">
          <label>Klargjør for deling</label>
          <div className="desc" style={{ margin: "0 0 13px" }}>
            Tømmer alle kilder + nøkkel/innstillinger så pakka kan gis videre ren. Bildene dine på
            disk røres ikke.
          </div>
          <button
            className="dangerbtn"
            onClick={async () => {
              if (!window.confirm("Tømme kilder + nøkkel/innstillinger? Bildene på disk røres ikke."))
                return;
              const r = await api.prepareHandoff();
              qc.invalidateQueries({ queryKey: ["settings"] });
              qc.invalidateQueries({ queryKey: ["sources"] });
              setMsg(r.message);
            }}
          >
            Klargjør for deling…
          </button>
        </div>
      </div>
    </div>
  );
}
