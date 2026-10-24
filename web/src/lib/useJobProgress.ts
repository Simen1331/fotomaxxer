import { useEffect, useRef } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";

// Avledede queries som en fullført pipeline-jobb kan ha endret.
const INVALIDATE_ON_DONE = ["sources", "images", "facets", "persons", "progress", "cost"];

// Poller jobb-status + fremdrift ~1,5 s KUN mens en jobb kjører (TB-krav).
// Når ingen jobb kjører står begge stille til neste manuelle invalidasjon.
// Når en jobb går fra kjørende → ferdig, friskes avledede data automatisk, så
// f.eks. kilde-kortet ikke står igjen med stale «0 filer · aldri skannet».
export function useJobProgress() {
  const qc = useQueryClient();
  const current = useQuery({
    queryKey: ["jobCurrent"],
    queryFn: api.jobCurrent,
    refetchInterval: (q) => (q.state.data?.running ? 1500 : false),
  });
  const running = current.data?.running ?? false;
  const jobSource = current.data?.source_id ?? null;

  const wasRunning = useRef(running);
  useEffect(() => {
    if (wasRunning.current && !running) {
      for (const key of INVALIDATE_ON_DONE) qc.invalidateQueries({ queryKey: [key] });
    }
    wasRunning.current = running;
  }, [running, qc]);

  // Fremdriften scopes til kilden jobben kjører på, så baren reflekterer kun den
  // mappa (ikke hele bibliotekets pending).
  const progress = useQuery({
    queryKey: ["progress", jobSource],
    queryFn: () => api.progress(jobSource ?? undefined),
    refetchInterval: running ? 1500 : false,
  });
  return { current: current.data, progress: progress.data, running };
}
