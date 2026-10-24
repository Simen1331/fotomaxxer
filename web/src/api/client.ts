// Tynn fetch-klient + TS-typer mot FastAPI-et. Speiler api/schemas.py — hold i synk.
// Dev: Vite proxer /api til :8000. Prod: samme origin som SPA-en.

export interface ImageListItem {
  id: number;
  hash: string;
  thumb_url: string;
  taken_at: string | null;
  year: number | null;
  season: string | null;
  place_name: string | null;
  person_count: number;
  tag_count: number;
  excluded: boolean;
}

export interface ImageList {
  items: ImageListItem[];
  total: number;
}

export interface Face {
  id: number;
  bbox: [number, number, number, number];
  cluster_id: number | null;
  person_name: string | null;
  locked: boolean;
}

export interface ImageDetail {
  id: number;
  hash: string;
  width: number | null;
  height: number | null;
  taken_at: string | null;
  year: number | null;
  season: string | null;
  place_name: string | null;
  camera_make: string | null;
  camera_model: string | null;
  gps_lat: number | null;
  gps_lon: number | null;
  excluded: boolean;
  image_url: string;
  ai: {
    scene: string | null;
    event_type: string | null;
    season_ai: string | null;
    indoor_outdoor: string | null;
    person_count: number | null;
    description: string | null;
    provider: string | null;
  } | null;
  tags: string[];
  faces: Face[];
  files: { path: string; source_id: number }[];
}

export interface Facets {
  years: number[];
  seasons: string[];
  places: string[];
  events: string[];
  tags: { name: string; count: number; category: string | null }[];
  persons: { cluster_id: number; name: string | null; count: number }[];
}

export interface PersonCover {
  image_id: number;
  hash: string;
  bbox: [number, number, number, number];
  width: number | null;
  height: number | null;
}

export interface PersonSummary {
  cluster_id: number;
  name: string | null;
  face_count: number;
  cover: PersonCover | null;
}

export interface PersonFaceItem {
  face_id: number;
  image_id: number;
  hash: string;
  bbox: [number, number, number, number];
  locked: boolean;
  width: number | null;
  height: number | null;
}

export interface AlbumSummary {
  id: number;
  name: string;
  item_count: number;
  created_at: string;
}

export interface AlbumItem {
  image_id: number;
  hash: string;
  thumb_url: string;
  position: number;
}

export interface AlbumDetail {
  id: number;
  name: string;
  items: AlbumItem[];
}

export interface JobCurrent {
  running: boolean;
  kind: string | null;
  step: string | null;
  started_at: string | null;
  message: string | null;
  progress: Record<string, number>;
  source_id: number | null;
}

export interface SourceItem {
  id: number;
  root_path: string;
  source_type: string;
  last_scanned_at: string | null;
  file_count: number;
  profile_id: number | null; // tagging-profil; null = arv globalt
}

export interface Profile {
  id: number;
  name: string;
  focus: string | null;
  categories: string[];
}

export interface FsEntry {
  name: string;
  path: string;
}

export interface FsList {
  path: string;
  parent: string | null;
  entries: FsEntry[];
}

export interface StageProgress {
  pending: number;
  done: number;
  error: number;
}

export interface Progress {
  derive: StageProgress;
  ai: StageProgress;
  faces: StageProgress;
  images_total: number;
  faces_total: number;
  persons_total: number;
  confirm_threshold: number;
}

export interface CostSummary {
  total: number;
  avg: number;
  count: number;
}

export interface Settings {
  ai_model: string | null;
  face_match_threshold: string | null;
  face_min_cluster_size: string | null;
  default_export_dir: string | null;
  default_profile_id: number | null; // globalt standard tagging-profil; null = generisk
  openrouter_api_key: string; // alltid maskert
}

// Avansert: enkelt-steg i pipeline-kjeden.
export type StepKind = "scan" | "derive" | "tag" | "faces-detect" | "faces-cluster";

// Filtre som speiler GET /api/images query-parametre.
export interface ImageFilter {
  year?: number;
  season?: string;
  place?: string;
  event?: string;
  indoor_outdoor?: string;
  tag?: string;
  person?: number;
  q?: string;
  include_excluded?: boolean;
}

function toQuery(params: Record<string, unknown>): string {
  const sp = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v !== undefined && v !== null && v !== "") sp.set(k, String(v));
  }
  const s = sp.toString();
  return s ? `?${s}` : "";
}

async function sendJSON<T>(method: string, url: string, body: unknown): Promise<T> {
  const res = await fetch(url, {
    method,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    let detail = res.statusText;
    try {
      const b = await res.json();
      detail = b?.message ?? b?.detail?.message ?? detail;
    } catch {
      /* behold statusText */
    }
    throw new Error(`${res.status}: ${detail}`);
  }
  return res.json() as Promise<T>;
}

async function getJSON<T>(url: string): Promise<T> {
  const res = await fetch(url);
  if (!res.ok) {
    let detail = res.statusText;
    try {
      const body = await res.json();
      detail = body?.message ?? body?.detail?.message ?? detail;
    } catch {
      /* behold statusText */
    }
    throw new Error(`${res.status}: ${detail}`);
  }
  return res.json() as Promise<T>;
}

export const api = {
  images(filter: ImageFilter = {}, limit = 200, offset = 0): Promise<ImageList> {
    return getJSON<ImageList>(
      `/api/images${toQuery({ ...filter, limit, offset } as Record<string, unknown>)}`,
    );
  },
  imageIds(filter: ImageFilter = {}): Promise<{ ids: number[] }> {
    return getJSON<{ ids: number[] }>(
      `/api/images/ids${toQuery(filter as Record<string, unknown>)}`,
    );
  },
  image(id: number): Promise<ImageDetail> {
    return getJSON<ImageDetail>(`/api/images/${id}`);
  },
  setExcluded(id: number, excluded: boolean): Promise<{ id: number; excluded: boolean }> {
    return sendJSON(`PATCH`, `/api/images/${id}`, { excluded });
  },
  facets(): Promise<Facets> {
    return getJSON<Facets>(`/api/facets`);
  },

  persons(): Promise<{ items: PersonSummary[] }> {
    return getJSON<{ items: PersonSummary[] }>(`/api/persons`);
  },
  personFaces(clusterId: number): Promise<{ items: PersonFaceItem[] }> {
    return getJSON<{ items: PersonFaceItem[] }>(`/api/persons/${clusterId}/faces`);
  },
  renamePerson(clusterId: number, name: string | null): Promise<{ cluster_id: number; name: string | null }> {
    return sendJSON(`PUT`, `/api/persons/${clusterId}`, { name });
  },
  createPerson(face_ids: number[], name?: string | null): Promise<{ cluster_id: number }> {
    return sendJSON(`POST`, `/api/persons`, { face_ids, name: name ?? null });
  },
  mergePerson(clusterId: number, into_cluster_id: number): Promise<{ cluster_id: number; name: string | null }> {
    return sendJSON(`POST`, `/api/persons/${clusterId}/merge`, { into_cluster_id });
  },
  deletePerson(clusterId: number): Promise<{ deleted: number }> {
    return sendJSON(`DELETE`, `/api/persons/${clusterId}`, {});
  },
  ignorePerson(clusterId: number): Promise<{ ignored: number; faces: number }> {
    return sendJSON(`POST`, `/api/persons/${clusterId}/ignore`, {});
  },
  patchFace(
    faceId: number,
    body: { cluster_id?: number | null; locked?: boolean },
  ): Promise<{ id: number; cluster_id: number | null; locked: boolean }> {
    return sendJSON(`PUT`, `/api/faces/${faceId}`, body);
  },

  albums(): Promise<{ items: AlbumSummary[] }> {
    return getJSON<{ items: AlbumSummary[] }>(`/api/albums`);
  },
  album(id: number): Promise<AlbumDetail> {
    return getJSON<AlbumDetail>(`/api/albums/${id}`);
  },
  createAlbum(name: string): Promise<AlbumSummary> {
    return sendJSON<AlbumSummary>(`POST`, `/api/albums`, { name });
  },
  addToAlbum(id: number, image_ids: number[]): Promise<AlbumDetail> {
    return sendJSON<AlbumDetail>(`POST`, `/api/albums/${id}/items`, { image_ids });
  },
  removeFromAlbum(id: number, image_id: number): Promise<AlbumDetail> {
    return sendJSON<AlbumDetail>(`DELETE`, `/api/albums/${id}/items/${image_id}`, {});
  },
  reorderAlbum(id: number, image_ids: number[]): Promise<AlbumDetail> {
    return sendJSON<AlbumDetail>(`PATCH`, `/api/albums/${id}/order`, { image_ids });
  },
  deleteAlbum(id: number): Promise<{ deleted: number }> {
    return sendJSON<{ deleted: number }>(`DELETE`, `/api/albums/${id}`, {});
  },

  exportAlbum(body: {
    album_id?: number;
    image_ids?: number[];
    dest_path?: string;
    keep_original_names?: boolean;
  }): Promise<{ job_id: string; kind: string }> {
    return sendJSON(`POST`, `/api/export`, body);
  },
  jobCurrent(): Promise<JobCurrent> {
    return getJSON<JobCurrent>(`/api/jobs/current`);
  },

  sources(): Promise<{ items: SourceItem[] }> {
    return getJSON<{ items: SourceItem[] }>(`/api/sources`);
  },
  addSource(path: string, profile_id?: number | null): Promise<SourceItem> {
    return sendJSON<SourceItem>(`POST`, `/api/sources`, { path, profile_id: profile_id ?? null });
  },
  setSourceProfile(id: number, profile_id: number | null): Promise<SourceItem> {
    return sendJSON<SourceItem>(`PATCH`, `/api/sources/${id}`, { profile_id });
  },
  deleteSource(id: number): Promise<{ removed: number; summary: string }> {
    return sendJSON(`DELETE`, `/api/sources/${id}`, {});
  },

  profiles(): Promise<{ items: Profile[] }> {
    return getJSON<{ items: Profile[] }>(`/api/profiles`);
  },
  createProfile(body: { name: string; focus: string | null; categories: string[] }): Promise<Profile> {
    return sendJSON<Profile>(`POST`, `/api/profiles`, body);
  },
  updateProfile(
    id: number,
    body: { name: string; focus: string | null; categories: string[] },
  ): Promise<Profile> {
    return sendJSON<Profile>(`PUT`, `/api/profiles/${id}`, body);
  },
  deleteProfile(id: number): Promise<{ deleted: number }> {
    return sendJSON(`DELETE`, `/api/profiles/${id}`, {});
  },
  browse(path?: string): Promise<FsList> {
    return getJSON<FsList>(`/api/fs/list${path ? toQuery({ path }) : ""}`);
  },

  updateLibrary(body: { source_id?: number; all?: boolean }): Promise<JobAccepted> {
    return sendJSON<JobAccepted>(`POST`, `/api/jobs/update`, body);
  },
  runStep(kind: StepKind, body: Record<string, unknown> = {}): Promise<JobAccepted> {
    return sendJSON<JobAccepted>(`POST`, `/api/jobs/${kind}`, body);
  },
  progress(sourceId?: number): Promise<Progress> {
    return getJSON<Progress>(
      `/api/progress${sourceId != null ? toQuery({ source_id: sourceId }) : ""}`,
    );
  },
  cost(sourceId?: number): Promise<CostSummary> {
    return getJSON<CostSummary>(
      `/api/cost${sourceId != null ? toQuery({ source_id: sourceId }) : ""}`,
    );
  },

  settings(): Promise<Settings> {
    return getJSON<Settings>(`/api/settings`);
  },
  putSetting(key: string, value: string): Promise<Settings> {
    return sendJSON<Settings>(`PUT`, `/api/settings/${key}`, { value });
  },
  dataDir(): Promise<{ data_dir: string }> {
    return getJSON<{ data_dir: string }>(`/api/maintenance/data-dir`);
  },
  moveDataDir(path: string, deleteSource = false): Promise<{ data_dir: string }> {
    return sendJSON(`POST`, `/api/maintenance/data-dir`, { path, delete_source: deleteSource });
  },
  prepareHandoff(): Promise<{ message: string }> {
    return sendJSON(`POST`, `/api/maintenance/prepare-handoff`, {});
  },
};

interface JobAccepted {
  job_id: string;
  kind: string;
}
