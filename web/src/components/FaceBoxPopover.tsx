import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type Face } from "../api/client";

// Korrigerings-popover på en klikkbar ansikts-boks i lightboxen: endre person
// eller ignorer ansiktet. Mutasjonene er de samme som person-detalj bruker.
export default function FaceBoxPopover({
  face,
  imageId,
  onDone,
}: {
  face: Face;
  imageId: number;
  onDone: () => void;
}) {
  const qc = useQueryClient();
  const [mode, setMode] = useState<"menu" | "change">("menu");
  const { data: persons } = useQuery({
    queryKey: ["persons"],
    queryFn: api.persons,
    enabled: mode === "change",
  });

  function invalidate() {
    qc.invalidateQueries({ queryKey: ["image", imageId] });
    qc.invalidateQueries({ queryKey: ["persons"] });
    qc.invalidateQueries({ queryKey: ["facets"] });
    qc.invalidateQueries({ queryKey: ["images"] });
  }

  async function changeTo(clusterId: number) {
    await api.patchFace(face.id, { cluster_id: clusterId });
    invalidate();
    onDone();
  }

  async function ignore() {
    await api.patchFace(face.id, { cluster_id: null, locked: true });
    invalidate();
    onDone();
  }

  const others = (persons?.items ?? []).filter((p) => p.cluster_id !== face.cluster_id);

  return (
    <div className="face-pop" onClick={(e) => e.stopPropagation()}>
      <div className="fp-name">{face.person_name ?? "Ukjent person"}</div>
      {mode === "menu" ? (
        <>
          <button className="fp-act" onClick={() => setMode("change")}>
            Endre person…
          </button>
          <button className="fp-act" onClick={ignore}>
            Ignorer dette ansiktet
          </button>
        </>
      ) : (
        <select
          className="fp-act"
          autoFocus
          defaultValue=""
          onChange={(e) => e.target.value && changeTo(Number(e.target.value))}
        >
          <option value="" disabled>
            Velg person…
          </option>
          {others.map((p) => (
            <option key={p.cluster_id} value={p.cluster_id}>
              {p.name ?? `Uten navn (${p.face_count})`}
            </option>
          ))}
        </select>
      )}
    </div>
  );
}
