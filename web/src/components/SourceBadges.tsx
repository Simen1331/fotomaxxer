import { useQuery } from "@tanstack/react-query";
import { api } from "../api/client";

// Per-kilde status-badges: hvilke pipeline-steg som er kjørt på mappa.
// Bruker samme queryKey som useJobProgress' scopede poll (["progress", id]), så
// badgene oppdateres live mens en jobb kjører på kilden og når den er ferdig.
type St = "none" | "partial" | "done";

function stageState(done: number, pending: number): St {
  if (done + pending === 0) return "none";
  return pending === 0 ? "done" : "partial";
}

export default function SourceBadges({ sourceId }: { sourceId: number }) {
  const { data: p } = useQuery({
    queryKey: ["progress", sourceId],
    queryFn: () => api.progress(sourceId),
  });
  if (!p) return null;

  const stages: { label: string; st: St; done: number; total: number }[] = [
    {
      label: "Arbeidskopier",
      st: stageState(p.derive.done, p.derive.pending),
      done: p.derive.done,
      total: p.derive.done + p.derive.pending,
    },
    {
      label: "AI-merket",
      st: stageState(p.ai.done, p.ai.pending),
      done: p.ai.done,
      total: p.ai.done + p.ai.pending,
    },
    {
      label: "Ansikter",
      st: stageState(p.faces.done, p.faces.pending),
      done: p.faces.done,
      total: p.faces.done + p.faces.pending,
    },
  ];

  return (
    <div className="srcbadges">
      {p.images_total > 0 && <span className="badge done">Skannet</span>}
      {stages.map((s) => (
        <span key={s.label} className={`badge ${s.st}`}>
          {s.label}
          {s.st === "partial" ? ` ${s.done}/${s.total}` : ""}
        </span>
      ))}
    </div>
  );
}
