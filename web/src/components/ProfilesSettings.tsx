import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type Profile } from "../api/client";
import { PROFILE_PRESETS } from "../lib/profilePresets";

// Tagging-profiler i Innstillinger: opprett/rediger/slett + velg standard.
// En profil = navn + fokus-direktiv + kategorier. Kategoriene styrer både
// AI-prompten (stikkord bøttes per kategori) og gruppering i filter-sidebaren.
interface Draft {
  id: number | null;
  name: string;
  focus: string;
  categories: string[];
}

export default function ProfilesSettings({
  defaultProfileId,
}: {
  defaultProfileId: number | null;
}) {
  const qc = useQueryClient();
  const { data } = useQuery({ queryKey: ["profiles"], queryFn: api.profiles });
  const [draft, setDraft] = useState<Draft | null>(null);
  const [catInput, setCatInput] = useState("");
  const [err, setErr] = useState<string | null>(null);

  const profiles = data?.items ?? [];

  function refresh() {
    qc.invalidateQueries({ queryKey: ["profiles"] });
    qc.invalidateQueries({ queryKey: ["sources"] });
    qc.invalidateQueries({ queryKey: ["facets"] });
  }

  function newDraft() {
    setErr(null);
    setCatInput("");
    setDraft({ id: null, name: "", focus: "", categories: [] });
  }

  function editProfile(p: Profile) {
    setErr(null);
    setCatInput("");
    setDraft({ id: p.id, name: p.name, focus: p.focus ?? "", categories: [...p.categories] });
  }

  function applyPreset(name: string) {
    const p = PROFILE_PRESETS.find((x) => x.name === name);
    if (p) setDraft({ id: null, name: p.name, focus: p.focus, categories: [...p.categories] });
  }

  function addCategory() {
    const c = catInput.trim();
    if (!c || !draft) return;
    if (!draft.categories.includes(c)) setDraft({ ...draft, categories: [...draft.categories, c] });
    setCatInput("");
  }

  function removeCategory(c: string) {
    if (draft) setDraft({ ...draft, categories: draft.categories.filter((x) => x !== c) });
  }

  async function save() {
    if (!draft) return;
    const body = { name: draft.name.trim(), focus: draft.focus.trim() || null, categories: draft.categories };
    if (!body.name) {
      setErr("Profilen må ha et navn.");
      return;
    }
    setErr(null);
    try {
      if (draft.id == null) await api.createProfile(body);
      else await api.updateProfile(draft.id, body);
      setDraft(null);
      refresh();
    } catch (e) {
      setErr(String(e));
    }
  }

  async function remove(p: Profile) {
    if (!window.confirm(`Slette profilen «${p.name}»? Kilder som bruker den faller tilbake til standard.`))
      return;
    try {
      await api.deleteProfile(p.id);
      if (draft?.id === p.id) setDraft(null);
      refresh();
    } catch (e) {
      setErr(String(e));
    }
  }

  async function setDefault(id: number | null) {
    try {
      await api.putSetting("default_profile_id", id == null ? "" : String(id));
      qc.invalidateQueries({ queryKey: ["settings"] });
      qc.invalidateQueries({ queryKey: ["facets"] });
    } catch (e) {
      setErr(String(e));
    }
  }

  return (
    <div className="field">
      <label>Tagging-profiler</label>
      <div className="desc" style={{ marginTop: 0, marginBottom: 10 }}>
        En profil styrer hvor detaljert AI-en merker, og grupperer stikkordene i kategorier. Mapper
        uten egen profil bruker standard-profilen. «Ingen» = vanlig, generisk merking.
      </div>

      {err && <div className="jobmsg err">{err}</div>}

      <div className="proflist">
        <label className="profdefault">
          <input
            type="radio"
            checked={defaultProfileId == null}
            onChange={() => setDefault(null)}
          />
          Ingen standard (generisk merking)
        </label>
        {profiles.map((p) => (
          <div key={p.id} className="profrow">
            <label className="profdefault">
              <input
                type="radio"
                checked={defaultProfileId === p.id}
                onChange={() => setDefault(p.id)}
                title="Bruk som standard-profil"
              />
              <b>{p.name}</b>
            </label>
            {p.categories.length > 0 && <span className="profcats">{p.categories.join(" · ")}</span>}
            <button className="mini" onClick={() => editProfile(p)}>
              Rediger
            </button>
            <button className="mini" onClick={() => remove(p)}>
              Slett
            </button>
          </div>
        ))}
        {profiles.length === 0 && <div className="desc">Ingen profiler ennå.</div>}
      </div>

      {draft ? (
        <div className="profedit">
          {draft.id == null && (
            <div className="presets">
              {PROFILE_PRESETS.map((p) => (
                <button key={p.name} className="pastebtn" onClick={() => applyPreset(p.name)}>
                  {p.name}
                </button>
              ))}
            </div>
          )}
          <input
            className="inp"
            placeholder="Navn (f.eks. Design & håndverk)"
            value={draft.name}
            onChange={(e) => setDraft({ ...draft, name: e.target.value })}
          />
          <textarea
            className="inp"
            rows={2}
            placeholder="Fokus — hva skal vektes? Tomt = generisk."
            value={draft.focus}
            onChange={(e) => setDraft({ ...draft, focus: e.target.value })}
          />
          <div className="catrow">
            {draft.categories.map((c) => (
              <span key={c} className="catchip">
                {c}
                <button onClick={() => removeCategory(c)} title="Fjern kategori">
                  ✕
                </button>
              </span>
            ))}
          </div>
          <div className="inprow">
            <input
              className="inp"
              placeholder="legg til kategori (materiale, teknikk, stil …)"
              value={catInput}
              onChange={(e) => setCatInput(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && (e.preventDefault(), addCategory())}
            />
            <button className="pastebtn" onClick={addCategory} disabled={!catInput.trim()}>
              Legg til
            </button>
          </div>
          <div className="profedit-actions">
            <button className="pastebtn" onClick={save}>
              {draft.id == null ? "Opprett profil" : "Lagre"}
            </button>
            <button className="pastebtn" onClick={() => setDraft(null)}>
              Avbryt
            </button>
          </div>
        </div>
      ) : (
        <button className="pastebtn" style={{ marginTop: 10 }} onClick={newDraft}>
          ＋ Ny profil
        </button>
      )}
    </div>
  );
}
