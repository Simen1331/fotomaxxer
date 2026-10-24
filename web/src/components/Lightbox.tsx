import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type Face } from "../api/client";
import { useStore } from "../store";
import FaceBoxPopover from "./FaceBoxPopover";

// Lightbox: 1024-bilde + metadata. Ansikts-bokser er klikkbare korrigerings-
// innganger; popoveren er en stubb her — selve mutasjonene (endre person /
// ignorer) kommer i Sesjon 4. Pil-navigasjon går innenfor gjeldende filter.
export default function Lightbox() {
  const lightboxId = useStore((s) => s.lightboxId);
  const close = useStore((s) => s.closeLightbox);
  const step = useStore((s) => s.stepLightbox);
  const orderedIds = useStore((s) => s.orderedIds);
  const qc = useQueryClient();
  const [activeFace, setActiveFace] = useState<number | null>(null);

  const { data, isLoading } = useQuery({
    queryKey: ["image", lightboxId],
    queryFn: () => api.image(lightboxId!),
    enabled: lightboxId !== null,
  });

  useEffect(() => {
    setActiveFace(null);
  }, [lightboxId]);

  useEffect(() => {
    if (lightboxId === null) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") close();
      else if (e.key === "ArrowLeft") step(-1);
      else if (e.key === "ArrowRight") step(1);
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [lightboxId, close, step]);

  if (lightboxId === null) return null;

  const idx = orderedIds.indexOf(lightboxId);
  const hasPrev = idx > 0;
  const hasNext = idx >= 0 && idx < orderedIds.length - 1;

  async function toggleExcluded() {
    if (!data) return;
    await api.setExcluded(data.id, !data.excluded);
    qc.invalidateQueries({ queryKey: ["images"] });
    qc.invalidateQueries({ queryKey: ["facets"] });
    qc.invalidateQueries({ queryKey: ["image", data.id] });
  }

  function faceStyle(f: Face): React.CSSProperties {
    // bbox er i originalkoordinater; prosent mot original = prosent mot 1024-derivatet.
    if (!data?.width || !data?.height) return { display: "none" };
    const [x, y, w, h] = f.bbox;
    return {
      left: `${(x / data.width) * 100}%`,
      top: `${(y / data.height) * 100}%`,
      width: `${(w / data.width) * 100}%`,
      height: `${(h / data.height) * 100}%`,
    };
  }

  return (
    <div className="lightbox show" onClick={close}>
      <div className="lb-stage" onClick={(e) => e.stopPropagation()}>
        <button className="lb-close" onClick={close} aria-label="Lukk">
          ✕
        </button>
        {hasPrev && (
          <button className="lb-nav prev" onClick={() => step(-1)} aria-label="Forrige">
            ‹
          </button>
        )}
        {hasNext && (
          <button className="lb-nav next" onClick={() => step(1)} aria-label="Neste">
            ›
          </button>
        )}
        {data && (
          <div className="lb-imgwrap">
            <img src={data.image_url} alt="" />
            {data.faces.map((f) => (
              <button
                key={f.id}
                className={`face ${f.person_name ? "" : "unknown"}`}
                style={faceStyle(f)}
                onClick={() => setActiveFace(activeFace === f.id ? null : f.id)}
              >
                {f.person_name && <span>{f.person_name}</span>}
                {activeFace === f.id && (
                  <FaceBoxPopover
                    face={f}
                    imageId={data.id}
                    onDone={() => setActiveFace(null)}
                  />
                )}
              </button>
            ))}
          </div>
        )}
      </div>

      <aside className="lb-panel" onClick={(e) => e.stopPropagation()}>
        {isLoading || !data ? (
          <div className="state">Laster …</div>
        ) : (
          <>
            <div className="date">{data.taken_at ? data.taken_at.slice(0, 10) : "Uten dato"}</div>
            {data.place_name && <div className="loc">◆ {data.place_name}</div>}
            {data.ai?.description && <div className="lb-desc">{data.ai.description}</div>}

            {data.tags.length > 0 && (
              <div className="lb-sec">
                <div className="lb-chips">
                  {data.tags.map((t) => (
                    <span key={t} className="lb-chip">
                      {t}
                    </span>
                  ))}
                </div>
              </div>
            )}

            <div className="lb-sec lb-meta">
              {data.year != null && (
                <div>
                  <b>År</b>
                  {data.year}
                </div>
              )}
              {data.season && (
                <div>
                  <b>Sesong</b>
                  {data.season}
                </div>
              )}
              {(data.camera_make || data.camera_model) && (
                <div>
                  <b>Kamera</b>
                  {[data.camera_make, data.camera_model].filter(Boolean).join(" ")}
                </div>
              )}
              {data.faces.length > 0 && (
                <div>
                  <b>Personer</b>
                  {data.faces.length}
                </div>
              )}
            </div>

            <div className="lb-actions">
              <button className={data.excluded ? "btn-amber" : "btn-ghost"} onClick={toggleExcluded}>
                {data.excluded ? "Gjeninnsett bilde" : "Ekskludér bilde"}
              </button>
            </div>
          </>
        )}
      </aside>
    </div>
  );
}
