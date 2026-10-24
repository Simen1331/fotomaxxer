import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../api/client";
import { faceCropStyle } from "../lib/facecrop";

// Person-detalj: ansikts-rutenett for én gruppe. Multiselect → flytt til en annen
// person, lag ny person (splitt), eller ignorer søppel-ansikt. Navngi øverst.
export default function PersonDetail() {
  const params = useParams();
  const clusterId = Number(params.clusterId);
  const navigate = useNavigate();
  const qc = useQueryClient();

  const { data: persons } = useQuery({ queryKey: ["persons"], queryFn: api.persons });
  const { data, isLoading } = useQuery({
    queryKey: ["personFaces", clusterId],
    queryFn: () => api.personFaces(clusterId),
    enabled: Number.isFinite(clusterId),
  });

  const person = persons?.items.find((p) => p.cluster_id === clusterId);
  const [name, setName] = useState("");
  const [sel, setSel] = useState<Set<number>>(new Set());

  useEffect(() => {
    setName(person?.name ?? "");
  }, [person?.name]);

  useEffect(() => {
    setSel(new Set()); // tøm seleksjon når vi bytter person
  }, [clusterId]);

  function invalidate() {
    qc.invalidateQueries({ queryKey: ["personFaces", clusterId] });
    qc.invalidateQueries({ queryKey: ["persons"] });
    qc.invalidateQueries({ queryKey: ["facets"] });
    qc.invalidateQueries({ queryKey: ["images"] });
  }

  function toggle(faceId: number) {
    setSel((prev) => {
      const next = new Set(prev);
      if (next.has(faceId)) next.delete(faceId);
      else next.add(faceId);
      return next;
    });
  }

  async function commitName() {
    const n = name.trim() || null;
    await api.renamePerson(clusterId, n);
    invalidate();
  }

  async function moveTo(intoId: number) {
    await Promise.all([...sel].map((fid) => api.patchFace(fid, { cluster_id: intoId })));
    setSel(new Set());
    invalidate();
  }

  async function newPerson() {
    const r = await api.createPerson([...sel]);
    setSel(new Set());
    invalidate();
    navigate(`/people/${r.cluster_id}`);
  }

  async function ignore() {
    await Promise.all(
      [...sel].map((fid) => api.patchFace(fid, { cluster_id: null, locked: true })),
    );
    setSel(new Set());
    invalidate();
  }

  const faces = data?.items ?? [];
  const others = (persons?.items ?? []).filter((p) => p.cluster_id !== clusterId);

  return (
    <div className="page">
      <button className="backlink" onClick={() => navigate("/people")}>
        ‹ Alle personer
      </button>

      <div className="pd-head">
        <div
          className="cov"
          style={
            person?.cover
              ? faceCropStyle(
                  person.cover.hash,
                  person.cover.bbox,
                  person.cover.width,
                  person.cover.height,
                )
              : undefined
          }
        />
        <div style={{ flex: 1 }}>
          <input
            className="nameField"
            value={name}
            placeholder="Navngi person…"
            onChange={(e) => setName(e.target.value)}
            onBlur={commitName}
            onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
          />
          <div className="lead" style={{ margin: "8px 0 0" }}>
            {faces.length} ansikter i denne gruppen. Velg ansikter som ikke hører hjemme → flytt til
            en annen person, lag ny, eller ignorer.
          </div>
        </div>
      </div>

      {isLoading ? (
        <div className="state">Laster …</div>
      ) : (
        <div className="facegrid">
          {faces.map((f) => (
            <div
              key={f.face_id}
              className={`facecell ${sel.has(f.face_id) ? "sel" : ""}`}
              style={faceCropStyle(f.hash, f.bbox, f.width, f.height)}
              onClick={() => toggle(f.face_id)}
              title="Velg ansikt"
            >
              <span className="pick">✓</span>
              {f.locked && <span className="lock">ignorert</span>}
            </div>
          ))}
          {faces.length === 0 && <div className="state">Ingen ansikter i denne gruppen.</div>}
        </div>
      )}

      <div className={`facetoolbar ${sel.size > 0 ? "show" : ""}`}>
        <span className="lbl">
          <b>{sel.size}</b> ansikter valgt
        </span>
        <div className="sep" />
        <select
          className="btn-ghost"
          value=""
          onChange={(e) => e.target.value && moveTo(Number(e.target.value))}
          disabled={others.length === 0}
        >
          <option value="" disabled>
            Flytt til… ▾
          </option>
          {others.map((p) => (
            <option key={p.cluster_id} value={p.cluster_id}>
              {p.name ?? `Uten navn (${p.face_count})`}
            </option>
          ))}
        </select>
        <button className="btn-ghost" onClick={newPerson}>
          Ny person
        </button>
        <button className="btn-amber" onClick={ignore}>
          Ignorer
        </button>
        <button className="x" onClick={() => setSel(new Set())} aria-label="Tøm valg">
          ✕
        </button>
      </div>
    </div>
  );
}
