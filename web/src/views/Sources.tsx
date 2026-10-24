import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type StepKind } from "../api/client";
import { useJobProgress } from "../lib/useJobProgress";
import FolderPicker from "../components/FolderPicker";
import SourceBadges from "../components/SourceBadges";

// Kilder & pipeline: mappe-liste + «Oppdater biblioteket»-kjede med samlet
// fremdrift, og en «Avansert»-skuff for enkelt-steg. Sjargong er KUN tillatt i
// Avansert (mono-etiketter) — resten er brukerens språk.
const STEP_LABELS: { kind: StepKind; label: string; mono: string; body?: Record<string, unknown> }[] = [
  { kind: "scan", label: "Skann", mono: "finn nye", body: { all: true } },
  { kind: "derive", label: "Arbeidskopier", mono: "derive" },
  { kind: "tag", label: "AI-merking", mono: "tag" },
  { kind: "faces-detect", label: "Ansikts-deteksjon", mono: "detect" },
  { kind: "faces-cluster", label: "Klyng ansikter", mono: "cluster" },
  { kind: "faces-cluster", label: "Full re-klynging", mono: "cluster --full", body: { full: true } },
];

const STEP_TEXT: Record<string, string> = {
  scan: "finner nye bilder",
  derive: "lager arbeidskopier",
  tag: "AI-merker",
  "faces-detect": "finner ansikter",
  cluster: "grupperer ansikter",
};

export default function Sources() {
  const qc = useQueryClient();
  const { data } = useQuery({ queryKey: ["sources"], queryFn: api.sources });
  const { current, progress, running } = useJobProgress();
  const { data: costData } = useQuery({
    queryKey: ["cost"],
    queryFn: () => api.cost(),
    refetchInterval: running ? 1500 : false, // klatrer live mens AI-merking kjører
  });
  const { data: profilesData } = useQuery({ queryKey: ["profiles"], queryFn: api.profiles });
  const [picking, setPicking] = useState(false);
  const [advSource, setAdvSource] = useState<number | "all">("all");
  const [err, setErr] = useState<string | null>(null);

  const profiles = profilesData?.items ?? [];

  function refresh() {
    qc.invalidateQueries({ queryKey: ["jobCurrent"] });
    qc.invalidateQueries({ queryKey: ["progress"] });
    qc.invalidateQueries({ queryKey: ["sources"] });
    qc.invalidateQueries({ queryKey: ["images"] });
    qc.invalidateQueries({ queryKey: ["facets"] });
    qc.invalidateQueries({ queryKey: ["persons"] });
  }

  // Kostnadsgate: sjekk ai-pending live før kjeden treffer det betalte API-et.
  // Scopes til kilden hvis gitt; estimerer kost fra snittet av tidligere merkinger.
  async function confirmCost(sourceId?: number): Promise<boolean> {
    const p = await api.progress(sourceId);
    if (p.ai.pending > p.confirm_threshold) {
      const c = await api.cost();
      const est =
        c.count > 0
          ? `estimert ~$${(c.avg * p.ai.pending).toFixed(2)} basert på snitt fra ${c.count} bilder`
          : "koster penger";
      return window.confirm(
        `Dette merker ${p.ai.pending} nye bilder via OpenRouter (${est}). Fortsette?`,
      );
    }
    return true;
  }

  // Re-merk-gate: ved retag nullstilles allerede-merkede bilder (done/error) til
  // pending, så den vanlige confirmCost (som ser pending=0) ville sluppet det
  // gjennom ugated. Her teller vi det som FAKTISK re-merkes og bekrefter alltid.
  async function confirmRetagCost(sourceId: number): Promise<boolean> {
    const p = await api.progress(sourceId);
    const n = p.ai.done + p.ai.error; // disse blir pending igjen ved retag
    if (n === 0) return true; // ingenting merket ennå — vanlig tag-flyt gjelder
    const c = await api.cost(sourceId);
    const est =
      c.count > 0
        ? `estimert ~$${(c.avg * n).toFixed(2)} basert på snitt fra ${c.count} bilder`
        : "koster penger";
    return window.confirm(`Merker ${n} bilder på nytt med profil via OpenRouter (${est}). Fortsette?`);
  }

  async function changeProfile(id: number, profileId: number | null) {
    setErr(null);
    try {
      await api.setSourceProfile(id, profileId);
      qc.invalidateQueries({ queryKey: ["sources"] });
    } catch (e) {
      setErr(String(e));
    }
  }

  // «Merk på nytt med profil»: kilde-scopet retag gjennom kost-gaten.
  async function retagWithProfile(id: number) {
    setErr(null);
    if (!(await confirmRetagCost(id))) return;
    try {
      await api.runStep("tag", { source_id: id, retag: true });
      refresh();
    } catch (e) {
      setErr(String(e));
    }
  }

  async function startUpdate(sourceId?: number) {
    setErr(null);
    if (!(await confirmCost(sourceId))) return;
    try {
      await api.updateLibrary(sourceId != null ? { source_id: sourceId } : { all: true });
      refresh();
    } catch (e) {
      setErr(String(e));
    }
  }

  // Slå sammen valgt Avansert-mappe inn i steg-body. source_id overstyrer «all».
  function withSource(base?: Record<string, unknown>): Record<string, unknown> {
    const b = { ...(base ?? {}) };
    if (advSource !== "all") {
      b.source_id = advSource;
      delete b.all;
    }
    return b;
  }

  async function runStep(kind: StepKind, body?: Record<string, unknown>) {
    setErr(null);
    const sid = typeof body?.source_id === "number" ? body.source_id : undefined;
    if (kind === "tag" && !(await confirmCost(sid))) return;
    try {
      await api.runStep(kind, body ?? {});
      refresh();
    } catch (e) {
      setErr(String(e));
    }
  }

  async function addSourcePath(p: string) {
    const trimmed = p.trim();
    if (!trimmed) return;
    setErr(null);
    try {
      await api.addSource(trimmed);
      setPicking(false);
      qc.invalidateQueries({ queryKey: ["sources"] });
    } catch (e) {
      setErr(String(e));
    }
  }

  // Undermappe-tre: legg til hver avhuket løvmappe som egen kilde (med valgfri
  // per-rad profil). Én POST per rad — forelderen registreres aldri.
  async function addSources(rows: { path: string; profile_id: number | null }[]) {
    setErr(null);
    try {
      for (const r of rows) {
        await api.addSource(r.path, r.profile_id);
      }
      setPicking(false);
      qc.invalidateQueries({ queryKey: ["sources"] });
    } catch (e) {
      setErr(String(e));
    }
  }

  async function removeSource(id: number) {
    if (!window.confirm("Fjerne denne kilden? Bildene på disk røres ikke.")) return;
    await api.deleteSource(id);
    refresh();
  }

  const sources = data?.items ?? [];

  return (
    <div className="page">
      <h1>Kilder &amp; pipeline</h1>
      <p className="lead">
        Legg til mappene du vil ha med. «Oppdater biblioteket» finner nye bilder, lager
        arbeidskopier, AI-merker og grupperer ansikter — i ett.
      </p>

      <div className="updatebar">
        <div className="txt">
          <b>Oppdater biblioteket</b>
          <span>
            {running && current
              ? `Jobber … ${STEP_TEXT[current.step ?? ""] ?? current.step ?? ""}`
              : "Kjører skann → arbeidskopier → AI-merking → ansikter på alle nye bilder"}
          </span>
        </div>
        <button className="bigbtn" onClick={() => startUpdate()} disabled={running}>
          {running ? "Jobber …" : "Oppdater alt"}
        </button>
      </div>

      {running && progress && (
        <div className="jobprog">
          <span className="mono">{current?.step}</span>
          {current?.step === "scan" ? (
            <>
              {" "}
              · skannet {current?.progress?.scan_seen ?? 0} bilder
              {(current?.progress?.scan_new ?? 0) > 0 ? ` (${current?.progress?.scan_new} nye)` : ""}
            </>
          ) : (
            <>
              {" "}
              · arbeidskopier {progress.derive.done}/
              {progress.derive.done + progress.derive.pending} · merket {progress.ai.done}/
              {progress.ai.done + progress.ai.pending} · ansikter {progress.faces_total} · personer{" "}
              {progress.persons_total}
            </>
          )}
        </div>
      )}
      {current?.message && !running && <div className="jobmsg">{current.message}</div>}
      {err && <div className="jobmsg err">{err}</div>}

      {costData && costData.count > 0 && (
        <div className="costline">
          AI-kost så langt: <b>${costData.total.toFixed(2)}</b> · snitt $
          {costData.avg.toFixed(4)}/bilde · {costData.count} merket
        </div>
      )}

      <div className="srclist">
        {sources.map((s) => (
          <div key={s.id} className="srcrow">
            <div className="ic">◆</div>
            <div className="info">
              <b>{s.root_path.split("/").filter(Boolean).pop() || s.root_path}</b>
              <span>{s.root_path}</span>
              <SourceBadges sourceId={s.id} />
            </div>
            <select
              className="inp profsel"
              value={s.profile_id ?? ""}
              onChange={(e) => changeProfile(s.id, e.target.value ? Number(e.target.value) : null)}
              disabled={running}
              title="Tagging-profil for denne mappa"
            >
              <option value="">Standard-profil</option>
              {profiles.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            </select>
            <div className="st">
              {s.file_count} filer ·{" "}
              {s.last_scanned_at ? s.last_scanned_at.slice(0, 10) : "aldri skannet"}
            </div>
            <button className="mini" onClick={() => startUpdate(s.id)} disabled={running}>
              Oppdater
            </button>
            <button
              className="mini"
              onClick={() => retagWithProfile(s.id)}
              disabled={running}
              title="Merk alle bilder i mappa på nytt med valgt profil (koster penger — du får bekreftelse)"
            >
              Merk på nytt
            </button>
            <button className="mini" onClick={() => removeSource(s.id)} disabled={running}>
              Fjern
            </button>
          </div>
        ))}
        {sources.length === 0 && <div className="state">Ingen kilder ennå. Legg til en mappe under.</div>}
      </div>

      <div className="addsrc" onClick={() => setPicking(true)}>
        ＋ Legg til mappe…
      </div>

      {picking && (
        <FolderPicker
          onClose={() => setPicking(false)}
          onPick={addSourcePath}
          onPickMany={addSources}
        />
      )}

      <details className="adv">
        <summary>
          <span className="chev">▸</span>Avansert — kjør enkelt-steg
        </summary>
        <div className="body">
          <div className="advsource">
            <label>Kjør på:</label>
            <select
              className="inp"
              value={advSource}
              onChange={(e) =>
                setAdvSource(e.target.value === "all" ? "all" : Number(e.target.value))
              }
            >
              <option value="all">Alle mapper</option>
              {sources.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.root_path.split("/").filter(Boolean).pop() || s.root_path}
                </option>
              ))}
            </select>
          </div>
          {STEP_LABELS.map((s) => (
            <button
              key={s.label}
              className="stepbtn"
              disabled={running}
              onClick={() => runStep(s.kind, withSource(s.body))}
            >
              {s.label} <span className="mono">{s.mono}</span>
            </button>
          ))}
        </div>
      </details>
    </div>
  );
}
