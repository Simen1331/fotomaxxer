# FotoMaxxer — Development Context

Lokal-først, bærbar foto-kuratering for trykte album. Single-user, ingen sky,
ingen GPU. En idempotent, `processing_state`-drevet berikelses-pipeline med et lokalt
web-GUI som kontroll- og bla-lag.

Arkitektur-autoritet: `artifacts/blueprint/ta-fotomaxxer-v1.md` (utenfor dette repoet).
Subsystem-spesifikasjoner (Technical Briefs): data-model, AI-pipeline, GUI.

## Stack (låst på tvers av de tre TB-ene)

| Lag | Valg |
|-----|------|
| Pipeline + CLI | Python 3.11+ (verifisert på 3.14.5), flat `fotomaxxer/`-pakke |
| Database | SQLite (én fil, `fotomaxxer.db`), skjema designet for ren Postgres-migrasjon |
| Bilder | Pillow + pillow-heif (derive), pyexiftool (EXIF), Kartverket/Geonorge (geocode) |
| AI-merking | OpenRouter vision (default `google/gemini-2.5-flash`), nøkkel i `settings`, ikke `.env` |
| Ansikter | insightface `buffalo_l` + onnxruntime (CPU), HDBSCAN via scikit-learn |
| Backend-API | FastAPI + uvicorn (`api/`) — importerer pipeline-funksjonene direkte |
| Frontend | Vite + React (TypeScript) SPA (`web/`) — TanStack Query, Zustand, react-virtuoso |

## Repo-struktur

```
fotomaxxer/        flat Python-pakke — ett pipeline-steg / én concern per modul
  cli.py           argparse: init/scan/derive/tag/faces/source/prepare-handoff/serve
  db.py            connection + full DDL + idempotent init_db (PRAGMA user_version)
  storage.py       ett lag: "hvor ligger bildet" (pakke-relativ resolver)
  scan.py          walk → whitelist → stat-fast-path → hash → dedup → EXIF/geocode
  exif.py geocode.py
  derive.py        1024 + 300, HEIC, orientering, prosess-pool
  sources.py       add/list/remove + orphan-opprydding + prepare-for-handoff
  settings.py      get/set/all_settings + mask_value (nøkkel/modell/terskler)
  tagging.py       OpenRouter vision bak VisionProvider → ai_tags + tags
  faces.py         detect (insightface, prosess-pool) + cluster (HDBSCAN)
api/               FastAPI-app (planlagt — GUI-TB): routers, jobs.py, schemas.py
web/               Vite + React SPA (planlagt — GUI-TB)
design/            visuell fasit — fotomaxxer-mockup.html (mørkerom/arkiv-retning, åpne i nettleser)
fotomaxxer.db      SQLite (generert, del av flyttbar pakke)
derived/           avledede thumbnails <hash>_<size>.jpg (generert)
exports/           genererte album (mapper + PDF — kommer med eksport-TB)
```

Flat med vilje: steg bindes sammen av skjemaet og `processing_state`-kontrakten, ikke av
mappestruktur. Nye steg legges som flate søsken-moduler. `api/` og `web/` er bevisst egne
søsken-mapper utenfor Python-pakka (egen stack), ikke undermapper.

## Lokal oppsett

```bash
python3 -m venv .venv && source .venv/bin/activate
pip install -e .                                  # installerer fotomaxxer + deps
fotomaxxer init                                   # oppretter fotomaxxer.db + derived/
```

`FOTOMAXXER_HOME` (valgfri) peker på pakke-rota (mappa med `fotomaxxer.db`); ellers cwd.

## Vanlige kommandoer

```bash
fotomaxxer source add /sti/til/mappe              # registrer en kilde-mappe
fotomaxxer scan --source /sti/til/mappe           # (eller --all) hash + EXIF + geocode → DB
fotomaxxer derive                                 # nedskaler til 1024 + 300 (prosess-pool)

fotomaxxer settings set openrouter_api_key <key>  # nøkkel lever i settings, ALDRI .env
fotomaxxer settings set ai_model google/gemini-2.5-flash
fotomaxxer settings list                          # api-nøkkel maskeres

fotomaxxer tag [--limit N] [--workers N] [--retag] [--yes]   # AI-merking (betalt API, kostnadsgate)
fotomaxxer faces detect [--workers N] [--retry-errors]       # ansikts-deteksjon + embeddings
fotomaxxer faces cluster [--full] [--threshold T]            # klynging (inkrementell | full re-map)

fotomaxxer serve                                  # (planlagt — GUI-TB) start FastAPI på 127.0.0.1:8000
```

## Konvensjoner

- **Idempotente steg:** alt arbeid drives av `processing_state(stage, status)`. `done`-bilder
  hoppes over; re-kjøring uten flagg gjør intet. AI/faces plukker kun bilder der `derive`=`done`.
- **Myk per-fil-feil:** én råtten fil/respons → `status='error'` + melding, batchen fortsetter. Aldri la én fil stoppe en jobb.
- **Pipeline-funksjoner er rene `(conn, storage, …)`** — GUI/API kaller dem uendret (ingen subprocess).
- **Nøkkel i `settings`, ikke `.env`** — så pakka er delbar og en ny bruker slipper å redigere filer.
- **Bærbar pakke:** appens egne stier er relative til pakke-rota; kun `files.path` (eksterne originaler) er absolutt.
- **bbox i originalkoordinater; embeddings L2-normalisert float32 BLOB** (cosinus = punktprodukt).
- Norsk for bruker-vendt tekst og kommentarer.

## Arkitektur-referanse

- TA: `artifacts/blueprint/ta-fotomaxxer-v1.md`
- TB-er: `tb-fotomaxxer-data-model-v1`, `tb-fotomaxxer-ai-pipeline-v1`, `tb-fotomaxxer-gui-v1`
  (utenfor dette repoet)
