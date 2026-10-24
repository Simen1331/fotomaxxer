# Portering av FotoMaxxer til Rust

Målet er en Windows-leveranse som er enkel å bygge, installere og starte: én `FotoMaxxer.exe` med ansiktsmodellene i en mappe ved siden av. Den skal ikke trenge Python, ExifTool, Perl eller en separat runtime-installasjon. Dagens PyInstaller- og Qt-pakke fungerer i CI, men er ca. 1,3 GB, består av tusenvis av filer og har krevd egne løsninger for multiprocessing, ExifTool, stier med æøå og avslutning. [desktop-plan.md](desktop-plan.md) anbefalte å beholde Python med mindre pakking viste seg å bli et reelt problem. Det har den blitt, og derfor velger vi omskrivingen.

**Prinsipp: Rust-motoren skal være en drop-in-erstatning.** React-grensesnittet i `web/` brukes uendret. SQLite-skjema v5, HTTP-API-et og datamappemodellen beholdes. En eksisterende `fotomaxxer.db` med `derived/` skal kunne åpnes av Rust-versjonen uten konvertering, med samme album, tagger, personnavn og låste ansikter.

**Repo (fra 2026-10-24):** Koden ligger i det offentlige repoet [`Simen1331/fotomaxxer`](https://github.com/Simen1331/fotomaxxer) på grenen `main`. Det startet som én renset commit av `rust`-grenen fra det gamle private repoet, som nå heter `Simen1331/fotomaxxer-arkiv` og er arkivert med hele historikken (fase 0–7 og Python-versjonen på `master`). Python-koden (`fotomaxxer/`, `api/`) følger fortsatt med og er identisk med `master` i arkivet. Den brukes som fasit til den slettes ved overgangen i fase 8.

## Målarkitektur

```text
FotoMaxxer.exe  (én Rust-binær)
  ├─ CLI: init, scan, derive, tag, faces, source, settings, prepare-handoff, serve, desktop
  ├─ Lokal axum-tjener på loopback
  │    ├─ /api/*        samme kontrakt som FastAPI i dag
  │    ├─ /derived/*    thumbnails fra datamappa
  │    └─ /*            web/dist innebygd i binæren (rust-embed)
  ├─ Jobbkjører: én jobb om gangen, egen tråd og egen SQLite-forbindelse
  └─ Desktop-modus: eget vindu (Tauri/wry) eller standardnettleser
models/
  det_10g.onnx      SCRFD-deteksjon
  w600k_r50.onnx    ArcFace-embedding
```

Vi beholder HTTP i stedet for å bytte til Tauri-kommandoer. Da står frontend urørt, `serve` virker i vanlig nettleser som i dag, og API-et kan sammenlignes direkte mot Python-versjonen.

### Workspace

```text
Cargo.toml                 workspace
crates/
  core/      fotomaxxer-core     db, migrasjoner, storage, settings, profiles, sources,
                                 scan, exif, geocode, derive, tagging, export
  faces/     fotomaxxer-faces    SCRFD, justering, ArcFace (ort), klynging, kuratering
  api/       fotomaxxer-api      axum-rutere, skjemaer, jobbkjører, desktop-tilgang
  app/       fotomaxxer          binæren: CLI, serve og desktop
tools/
  golden/                        Python-skript som lager fasit fra Python-versjonen
web/                             uendret
```

`faces` er en egen crate fordi `ort` tar lang tid å kompilere og har den største risikoen. Resten av crate-grensene følger dagens moduler, slik at hver Python-fil får en åpenbar motpart.

## Erstatninger per lag

| Python | Rust | Merknad |
| --- | --- | --- |
| `sqlite3` | `rusqlite` med feature `bundled` | Ingen system-SQLite. `busy_timeout` 5 s som i Python, journalmodus uendret |
| FastAPI + uvicorn + StaticFiles | `axum`, `tokio`, `tower-http`, `rust-embed` | Frontend bygges inn i binæren |
| pydantic-skjemaer | `serde`-structs | Feltnavn og feilformat (`{"detail": {"error", "message"}}`) må være identiske |
| `ProcessPoolExecutor` | `rayon` | Ingen spawn/fork, freeze_support eller rekursiv oppstart |
| `ThreadPoolExecutor` (tagging) | Tråder (`std::thread::scope`) med `ureq`, 6 samtidige som standard | |
| `requests` | `ureq` (rustls) | Ingen OpenSSL-avhengighet på Windows. Brukes av geokoding og AI-merking |
| `hashlib.sha256` | `sha2` | |
| pyexiftool + ExifTool | `kamadak-exif`, med `imagesize` som reserve for dimensjoner | **Fjerner ExifTool og Perl helt.** Se risiko R3 |
| Pillow | Egne porter av Pillow/libjpeg-turbo: `jpegdec` (dekoding), `resample` (`thumbnail`), `jpegenc` (kvalitet 85); `image` for PNG o.l. | Gir byte-identiske derivater. EXIF-orientering anvendes før skalering |
| pillow-heif | `libheif-rs` | Byggevei på Windows avklares i fase 0. Se risiko R2 |
| insightface + onnxruntime + OpenCV | `ort` + egen pre- og etterbehandling | Bare 2 av 5 `buffalo_l`-modeller trengs |
| sklearn `HDBSCAN` | Egen port av sklearn 1.9 (`crates/faces/src/hdbscan.rs`) med numpys `argsort` (`npsort.rs`) | Bit-like etiketter. Se «Resultat fra fase 4» |
| argparse | `clap` | Samme underkommandoer og flagg |
| PySide6 / Qt WebEngine | Tauri (wry) eller nettleser | Avgjøres i fase 7 |

## Kontrakter som må bevares eksakt

Dette er detaljer der en «naturlig» Rust-implementasjon ville brutt eksisterende data:

- **Skjema og migrasjoner.** `PRAGMA user_version` = 5, og alle migrasjonene v1 → v5 portes, inkludert `_migrate_v5`, som gjør fokus om til profiler. Datamappa `~/dev/fotomaxxer-data` har versjon 2, så migrasjonene må virke på ekte, eldre data. En database med høyere versjon enn appen kjenner skal avvises.
- **`scan_index.mtime`** er Pythons `st_mtime` som `REAL`. Rust må regne ut nøyaktig samme flyttall (sekunder + nanosekunder / 1e9). Ellers bommer hurtigsjekken, og hele biblioteket hashes på nytt ved første scan.
- **Stistrenger.** `files.path` og `sources.root_path` er absolutte og må ha samme form som Pythons `Path.resolve()` og `os.path.join`. På Windows betyr det ingen `\\?\`-prefiks fra `canonicalize`, så `dunce` brukes. Avvik gir duplikatrader per fil.
- **bbox-semantikk.** bbox lagres som `thumb_px * images.width / thumb_w` (tilsvarende for høyde), og frontend regner prosent mot `images.width/height`. Formelen beholdes uansett hva «original» betyr for roterte bilder. `images.width/height` må alltid fylles, med header-dimensjoner som reserve.
- **Embeddings.** 512 × `f32` little-endian, L2-normalisert, som BLOB.
- **Derived-filer.** `derived/<sha256>_<1024|300>.jpg`, og bare filnavnet lagres i `derived.path`.
- **Pekerfil.** Rekkefølgen `--home` > `FOTOMAXXER_HOME` > `~/.config/fotomaxxer/config.json` (`XDG_CONFIG_HOME`) > standard beholdes. Desktop-standarden er `%LOCALAPPDATA%\FotoMaxxer`.
- **AI-prompten.** Kopieres tegn for tegn, med alle tre profilstiene. Det samme gjelder den tolerante JSON-parsingen, tag-normaliseringen (lowercase, dedup, kategori med `COALESCE`) og kostnadsuttrekket fra `usage.cost`.
- **Klyngeregler.** Inkrementell tilordning mot sentroider (terskel `face_match_threshold`, standard 0,5) og så HDBSCAN på resten. `--full` gjenbruker navn via sentroidematching. Låste ansikter røres aldri.
- **Myk feil per fil** og `processing_state`-drevet idempotens, som i dag.

## Ansiktsgjenkjenning i detalj

insightface `FaceAnalysis.get()` kjører i dag alle fem modellene, men koden bruker bare bbox og `normed_embedding`. Rust-versjonen laster to:

1. **SCRFD (`det_10g.onnx`).** Bildet dekodes til RGB, JPEG med `jpegdec` (bit-likt med `cv2.imdecode`). Det skaleres med bevart aspekt inn i 640 × 640 (bilineær skalering, plassert øverst til venstre, nullpadding) og normaliseres med `(x − 127,5) / 128`. Ankre lages for stride 8, 16 og 32 (2 ankre per punkt), og avstandene dekodes til bokser og 5 nøkkelpunkter. Så følger terskel 0,5, NMS 0,4 og skalering tilbake til derivatkoordinater.
2. **Justering.** En likhetstransformasjon (Umeyama) fra de 5 punktene til ArcFace-malen for 112 × 112, og en affin warp med bilineær interpolasjon og svart kant. Dette tilsvarer `skimage.SimilarityTransform` + `cv2.warpAffine`.
3. **ArcFace (`w600k_r50.onnx`).** Normaliseres med `(x − 127,5) / 127,5`, sendes gjennom modellen og L2-normaliseres.

Akseptansekravet gjelder det som faktisk betyr noe: at **personene blir de samme**. Kravet er målt mot fasit fra Python på de samme derivatene:

- Ansiktsantallet er likt, bortsett fra deteksjoner med score innenfor ±0,01 av terskelen 0,5.
- Inkrementell tilordning mot Python-klyngenes sentroider gir samme person for ≥ 99 % av ansiktene.
- Full HDBSCAN-klynging gir samme antall klynger, og ARI ligger innenfor HDBSCANs egen ustabilitet. Den måles ved å stokke rekkefølgen på Python-embeddingene.

Det opprinnelige kravet om cosinus ≥ 0,99 per ansikt viste seg å være strengere enn nødvendig. Klyngingen bruker terskel 0,5, og bare små, uskarpe ansikter med lav score ligger under 0,99.

Holder kravene, kan eksisterende klynger og navn brukes videre. Holder de ikke, må `faces detect` kjøres på nytt for hele biblioteket, og navn flyttes via sentroidematching (`cluster --full`). Det er en akseptabel reserveløsning, men ikke målet.

### Resultat fra fase 0-spiken (2026-10-23)

Testbiblioteket var 926 JPEG-er fra 2011, kjørt gjennom Python-pipelinen. Det ga 649 ansikter og 38 klynger. Verktøy: `crates/faces/examples/compare_golden.rs` og `tools/golden/`.

| Mål | Resultat |
| --- | --- |
| Ansikter Python / Rust | 649 / 649. To bytter på terskelen (score 0,501 og 0,507) |
| bbox-avvik | Maks 4,4 px på ett ansikt med lav score. Ellers ≤ 1,6 px |
| Cosinus mot Python | Median 0,9990, p5 0,9952, min 0,9597 |
| Inkrementell tilordning, samme person | 645/648 (99,5 %). Alle tre avvik ligger på terskelen |
| Full HDBSCAN | 38/38 klynger, ARI 0,946. Til sammenligning gir Python med stokket rekkefølge ARI 0,957, og med støy på samme nivå ARI ~0,949 |
| Fart (veggklokke, 16 kjerner) | Rust ~100 ms per bilde. Python med de samme to modellene ~170 ms. Dagens Python-steg med alle fem modellene og 4 prosesser: 8 min for 926 bilder, ~520 ms per bilde |

**Windows og Linux i CI** (`.github/workflows/rust.yml`, kjøring 39987143001) er grønt på begge. JPEG, PNG og HEIC dekodes, og ansiktet i testbildet blir funnet. ONNX Runtime er lenket statisk, og testprogrammet er 30 MB. libheif bygges statisk via vcpkg på Windows. Det tar ~16 min uten cache, der x265-koderen alene står for 9 min. På Linux brukes systemets libheif.

To funn må løses i fase 7:

- `VCRUNTIME140.dll`/`MSVCP140.dll` (VC++-runtime) mangler på noen rene maskiner. Løsning: lenk CRT statisk (`x64-windows-static` + `crt-static`) eller legg DLL-ene ved exe-fila (tillatt app-lokal redistribusjon).
- `DirectML.dll` kommer fra ONNX Runtime-bygget. Den følger med Windows 10 1903+ og Windows 11, men versjonen må prøves på en ren maskin. Alternativet er et ORT-bygg som bare bruker CPU.

Skalering og warp er bit-eksakte etterligninger av OpenCVs fastpunktregning. Det største gjenværende avviket kom fra JPEG-dekodingen: `image`/zune-jpeg mot libjpeg-turbo gir maks 4 nivåer i 5 % av pikslene. Spiken beholdt `image` fordi resultatet holdt. Fra fase 3 finnes en libjpeg-ekvivalent dekoder (`crates/core/src/jpegdec.rs`), og nå leser også `faces detect` derivatene med den (se «JPEG-dekoding i ansiktsløypa» under fase 4).

`ort` kobles statisk der det er mulig. Ellers legges `onnxruntime.dll` ved siden av exe-fila. Vi bruker én `Session` per arbeidstråd, eller én delt sesjon med `intra_op_threads`. Valget gjøres ut fra målinger i fase 4.

### Resultat fra fase 4 (2026-10-23)

**Klynging er bit-lik med Python.** sklearn sorterer MST-kantene med `np.argsort`, som i numpy 2.4 går til x86-simd-sort og ikke er stabil. Like avstander er vanlige (gjensidige k-te naboer har samme kjerneavstand), og rekkefølgen avgjør treet: en stabil sortering gir 37 i stedet for 38 klynger på testbiblioteket. `crates/faces/src/npsort.rs` etterligner derfor AVX2-varianten av sorteringen eksakt. Sentroider og likheter regnes i `float32` med samme summeringsrekkefølge som numpy og OpenBLAS `sdot` (Haswell/Zen-kjernen), så også tilordninger rett på terskelen blir like. Python på en maskin med AVX-512 (eller uten AVX2) bryter likhet annerledes; Rust følger alltid AVX2-varianten.

| Mål | Resultat |
| --- | --- |
| HDBSCAN mot sklearn | 29 tilfeller på de 649 embeddingene (hele settet med `min_cluster_size` 2–6, 24 tilfeldige delmengder) og 5 syntetiske sett med mange like avstander: alle etiketter identiske |
| Sentroider og likheter | 38/38 sentroider og 24 662/24 662 likheter bit-like |
| `faces cluster` (CLI) | `compare_cli.py` med `scenarios/faces-klynging.json` og `faces-navn.json` (navn og låser via `--prep-sql`): utskrift og hele databasen identiske |
| Kuratering | `compare_faces_ops.py` med `scenarios/faces-kuratering.json` (42 operasjoner: alle sju kurateringsfunksjonene, feilmeldinger, inkrementell og full klynging med navne-remapping): svar og database identiske |
| `faces detect` + `cluster` | `compare_faces_db.py`: 649/649 ansikter uten bytter på terskelen, bbox identisk for alle 649. Cosinus min 0,99997. Inkrementell tilordning 649/649 (100 %). 39 personer mot Pythons 38 (ARI 0,984). Se «JPEG-dekoding i ansiktsløypa» under |
| Fart, 926 bilder | Rust 63 s (68 ms per bilde, 4 analysatorer). Python med 4 prosesser 453 s. Begge målt mens andre jobber gikk på maskinen |

ONNX Runtime tillater ett miljø per prosess, og `Session::run` krever eksklusiv tilgang. `detect` bruker derfor flere analysatorer med hver sine sesjoner og deler kjernene mellom dem (standard `kjerner / 4`, maks 2 fra fase 7; var maks 4). Hver analysator bruker ~430 MB. Trådantallet endrer ikke embeddingene (bit-like med 1 og 16 tråder). Modellmappa slås opp i rekkefølgen `FOTOMAXXER_MODELS` > `<exe>/models` > `~/.insightface/models/buffalo_l` (`crates/core/src/resources.rs`).

Bevisste avvik fra Python: manglende eller ødelagte modeller gir én tydelig feil før noe skrives, ikke `error` på hvert bilde. Feilteksten i `processing_state.error` er norsk i stedet for Pythons unntakstekst.

**JPEG-dekoding i ansiktsløypa.** Pythons `faces detect` leser derivatene med `cv2.imdecode`. OpenCV 4.13 bruker sin innebygde libjpeg-turbo (3.1.2) og endrer ingen av dekodeinnstillingene: islow-DCT, «fancy» oppsampling og blokkutjevning er libjpegs standard. Pillow gjør det samme (bare draft-modus bytter til rask DCT og boks-oppsampling). Derfor gir `jpegdec` de samme pikslene, og `imageio::open_rgb` bruker den nå for JPEG. `image` er reserve for varianter `jpegdec` ikke dekker og for filer libjpeg ville gitt opp, og PNG og HEIC går som før. Derivatene har ingen EXIF, så det spiller ingen rolle at `imdecode` anvender EXIF-orientering. `compare_golden` og `dump_stages` dekoder også via `open_rgb`.

`tools/golden/compare_decode.py` (med eksemplet `decode_stream`) dekoder alle 926 derivatene og sammenligner med `cv2.imdecode` (BGR → RGB). `jpegdec` og Pillow er bit-like på 926/926. `image`/zune-jpeg er bit-lik på 0/926, med maks avvik 5 nivåer, snitt 0,127 og 9,1 % ulike kanalverdier.

| Mål (926 bilder) | Før (`image`) | Etter (`jpegdec`) |
| --- | --- | --- |
| Ansikter Python / Rust | 649 / 649, to bytter på terskelen (score 0,501 og 0,507) | 649 / 649, ingen bytter |
| bbox-avvik, originalpiksler (`compare_faces_db.py`) | Maks 10, p99 2, median 0 | 0 for alle 649 |
| bbox og kps, derivatpiksler (`compare_golden`) | bbox maks 4,44, kps maks 3,28 | Under 0,005 |
| Cosinus mot Python | Median 0,9990, min 0,9597, 12 under 0,99 | Median 0,9999997, min 0,99997 |
| Inkrementell tilordning mot Python-sentroider, samme person | 645/648 (99,5 %) | 649/649 (100 %) |
| Personer etter `faces cluster` (`compare_faces_db.py`) | 38 / 38, ARI 0,931, 296/301 hos samme person | 38 / 39, ARI 0,984, 300/301 hos samme person |
| Full HDBSCAN (`cluster_compare.py`) | 38 / 38, ARI 0,946 | 38 / 38, ARI 0,988 |
| Tid `faces detect` (4 analysatorer, maskinen hadde annen last) | 55–60 s | 55–60 s |

Det som gjenstår, er flyttallsstøy i embeddingene: maks 1,2·10⁻³ per komponent, mot 4,2·10⁻² før. Dekodingen er nå bit-lik, og skalering og warp etterligner OpenCVs fastpunktregning bit-eksakt (fase 0). Støyen kommer derfor trolig fra ONNX Runtime, både i SCRFD-utdataene (kps avviker under 0,005 px) og i ArcFace. Den ekstra personen i databasekjøringen er en ny klynge på 3 ansikter (`min_cluster_size`) som Python lot være støy. I tillegg går ett støyansikt inn i en eksisterende person, og ett ansikt bytter mellom to personer. Pythons egen klyngekode gir nøyaktig de samme 39 klyngene på Rust-embeddingene. Avviket skyldes altså HDBSCANs følsomhet for små endringer og ikke klyngeporten. ARI 0,984 ligger godt over det Python selv oppnår med stokket rekkefølge (0,957). `cluster_compare.py`, som klynger i en annen rekkefølge, gir 38/38. `jpegdec` bruker 6,8 ms per derivat mot 5,2 ms for `image` (én tråd), under 2 % av analysetiden. De to `compare_cli.py`-scenarioene for ansikter er fortsatt identiske. De kjører bare klynging på fasit-embeddingene, så `faces detect` har ingenting å gjøre der.

## Testing og fasit

Python-versjonen er fasiten. `tools/golden/` får skript som kjøres med Python-versjonens venv mot et **testbibliotek** og lagrer forventet resultat som JSON:

- **scan:** hash, EXIF-felter, `width/height`, `place_name`, dedup og antall.
- **derive:** referansethumbnails. Rust-derivatene er byte-identiske med Pillows; `compare_derived.py` sjekker filnavn og dimensjoner og måler i tillegg PSNR og maks pikselavvik.
- **faces:** bbox, embeddings og klyngemerker for faste embeddings. `compare_decode.py` sjekker at ansiktsløypa dekoder derivatene bit-likt med `cv2.imdecode`.
- **tagging:** prompttekst per profilsti og parseresultat for innspilte OpenRouter-svar. Testene gjør ingen betalte kall. Miljøvariabelen `FOTOMAXXER_OPENROUTER_URL` overstyrer OpenRouter-endepunktet i Rust-versjonen (bare for tester); `tools/golden/py_fotomaxxer.py` gjør det samme for Python-versjonen uten å endre koden, så begge kan kjøres mot den falske tjeneren `tools/golden/fake_openrouter.py`.
- **API:** `tools/golden/compare_api.py` starter begge tjenerne mot hver sin kopi av samme datamappe, sender samme liste med forespørsler (`tools/golden/scenarios/api-*.json`) og sammenligner statuskode, headere og normaliserte svar byte for byte, og til slutt databasene. Se «Fase 6 (API)» under fremdriftstabellen.

Testbiblioteket må inneholde JPEG, PNG, HEIC/HEIF, alle EXIF-orienteringer, GPS i og utenfor Norge, duplikater i to kilder, en korrupt fil, filnavn med æøå og nok ansikter til meningsfull klynging (minst ca. 50 ansikter fordelt på 5+ personer). Dagens `fotomaxxer-data` har bare 6 bilder og 1 ansikt, så den holder til migrasjonstest, men ikke til ansiktsfasit.

Rust-siden får vanlige `cargo test`-enhetstester per modul og integrasjonstester som kjører CLI-en mot en midlertidig datamappe. De gamle sjekkene i `tests/verify_handoff_e2e.py` (dedup, idempotens, opprydding, originaler urørt, flytting, klargjøring for deling, at nøkkelen ikke lekker) gjenskapes som Rust-integrasjonstester.

## Faser

Hver fase avsluttes med kjørbar kode og grønne tester. Fase 0 er beslutningspunktet.

| Fase | Innhold | Ferdig når |
| --- | --- | --- |
| **0. Spike** | Workspace-skjelett. Ansikter fra start til slutt på derivater: SCRFD + justering + ArcFace med `ort`. Kompiler `libheif-rs` og `ort` for Windows i CI | Fasitkravene for ansikter holder på testbiblioteket, og Windows-bygget lenker og kjører spiken. **Hvis ikke: stopp og vurder på nytt** |
| **1. Data og konfig** | `db` (DDL + migrasjoner v1–v5), `storage` + pekerfil, `settings`, `profiles`, `sources` (add/list/remove/set_profile, opprydding, klargjøring for deling med VACUUM), CLI `init`/`source`/`settings` | Rust åpner og migrerer `fotomaxxer-data` (v2 → v5) og en v5-database fra master med samme resultat som Python |
| **2. Scan** | Gjennomgang av mapper, whitelist, hurtigsjekk med stat, SHA-256, dedup, EXIF, geokoding med cache, fremdrift | Scan-fasit er lik. Ny scan av en Python-skannet database gjør ingen ny hashing |
| **3. Derive** | Dekoding av JPEG/PNG/HEIC, orientering, 1024 + 300, `rayon`, myk feil | Thumbnails er innenfor PSNR-kravet. Korrupt fil gir `error`, og resten fortsetter |
| **4. Ansikter** | `detect` mot databasen, `cluster` (inkrementell + full, navnemapping), kurateringsfunksjonene (reassign, lock, split, merge, delete, ignore, rename) | Klyngemerkene stemmer med fasit for faste embeddings, og kuratering respekteres av en ny klynging |
| **5. AI-merking** | OpenRouter-klient, backoff, prompt per profil, parsing, lagring, kostnadsgrense i CLI, `--retag`/`--limit`/`source_id` | Prompt- og parsefasit er lik. Testene bruker en falsk leverandør uten nett |
| **6. API og jobber** | Alle `/api`-endepunktene, jobbkjører med «Oppdater alt»-kjeden, `/progress`, `/cost`, eksport, `/fs/list`, servering av SPA og `/derived`, `serve` | API-diffen mot Python er ren, og React-appen fungerer mot Rust-tjeneren i nettleser |
| **7. Desktop og Windows** | Desktop-modus med vindu, sesjonstoken via cookie, enkeltinstans, trygg lukking under jobb og avbrudd ved trygge punkter. NSIS-installasjonsprogram og GitHub Release fra GitHub Actions | Ren Windows 11: installer, start fra Start-menyen, hele flyten fungerer etter [windows-testsjekkliste.md](windows-testsjekkliste.md), og det finnes ikke noe konsollvindu |
| **8. Overgang** | Slett Python-koden, PyInstaller-oppsettet og de gamle workflowene. Oppdater `CLAUDE.md`, README og Windows-dokumentasjonen | Én stack på `main`, og dokumentasjonen beskriver bare Rust-versjonen |

### Fremdrift

| Fase | Status | Verifisert med |
| --- | --- | --- |
| 0 | Ferdig (2026-10-23) | `compare_golden` + `cluster_compare.py` (926 bilder) og CI på Windows/Linux |
| 1 | Ferdig (2026-10-23) | `compare_cli.py --scenario fase1`: 20 CLI-steg og databasen identiske. `--scenario migrering` på `fotomaxxer-data` (v2 → v5) og fasit-databasen (v5) er identiske |
| 2 | Ferdig (2026-10-23) | `exif_compare` mot ExifTool: 2556 filer, 25 kameramodeller, HEIC/PNG/GPS, alle felter identiske. `compare_cli.py --scenario scan` (210 filer med duplikater, korrupte filer, symlenker, æøå og overlappende kilder): utskrift og database identiske |
| 3 | Ferdig (2026-10-23) | `compare_cli.py --scenario-file tools/golden/scenarios/derive.json` (scanlib, 195 bilder): utskrift og database identiske med `--mask-errors` (to feiltekster for ødelagte filer avviker, se «Regresjon» under «Fase 6 (API)»). `compare_derived.py`: 382/382 og (926-settet) 1852/1852 thumbnails **byte-identiske** med Python. `jpeg_variants.py`: 506 JPEG-varianter bit-identiske med Pillow. 2400 korrupte JPEG-er gir samme ok/feil som Pillow. Veggtid derive: Rust 1,3–1,5× raskere. Se «Fase 3 (derive)» under tabellen |
| 4 | Ferdig (2026-10-23) | `hdbscan_golden` + `sklearn_fasit`-testene (etiketter, sentroider og likheter identiske), `compare_cli.py` og `compare_faces_ops.py` (klynging og kuratering: database identisk), `compare_faces_db.py` (deteksjon på 926 bilder: med `jpegdec` like ansikter og bbox, cosinus ≥ 0,99997, 649/649 hos samme person). Se «Resultat fra fase 4» |
| 5 | Ferdig (2026-10-23) | Enhetstester mot fasit fra Python-funksjonene (`tagging_golden.py`: prompt for alle tre profilstier, 44 modellsvar, 25 hele svar, 69 JSON-feiltekster, tagger, kategorier, kostnad, flyttall). `compare_tagging.py` mot `fake_openrouter.py` (16 svarvarianter, 205 bilder, 5 profiler): 515 forespørsler per implementasjon med byte-like kropper og headere, `ai_tags`/`processing_state`/tagger identiske ved 6–12 tråder, kostnadsgrensen og manglende nøkkel likt i CLI. Med `--workers 1` (`scenarios/tagging.json`) er hele databasen identisk uten `--mask-errors` |
| 6 | Ferdig (2026-10-23) | `compare_api.py` med 12 scenarier (`api-*.json`, 765 forespørsler og 22 filsjekker): alle identiske med FastAPI på statuskode, `content-type`, `content-length`, `allow`, kropp, filer og database (`api-jobber-scanlib` med de kjente maskeringene fra fase 3 og 4). Nettlesertest av React-appen mot `fotomaxxer serve` i Chromium (alle hovedflytene, ingen uventede 4xx/5xx eller konsollfeil). CLI-regresjonen (`fase1`, `scan`, `migrering`, `derive.json`) er uendret etter flettingen. Serve-røyktest i CI på Windows og Linux. Se «Fase 6 (API)» |
| 7 | Bygget og verifisert i CI (2026-10-24). **Venter på Simens test på ren Windows 11** | Lokalt (Linux): `cargo test --workspace`, clippy uten advarsler, desktop-røyktesten (`fotomaxxer-gui --smoke`, 17/17 sjekker) på ekte skjerm. `rust.yml` på Windows og Linux: tester, serve-røyktest, desktop-røyktest (xvfb/WebView2) og DLL-sjekk (bare systemets DLL-er). `windows-release.yml` på `windows-2022`: stille installering, filer, snarvei og register, `--smoke` på den installerte `FotoMaxxer.exe`, CLI-en med de installerte modellene (`faces detect` finner ansiktet), stille avinstallering med datamappa urørt. Prerelease `v0.1.0-rc.1` med Setup.exe (i arkivrepoet), og samme kode som første offentlige prerelease i det nye repoet. Se «Fase 7A (desktop)» og «Fase 7B (pakking)» |

Fase 3, 4 og 5 ble utviklet parallelt på hver sin gren og flettet inn i `rust` 2026-10-23. Etter flettingen ble alle sammenligningene i tabellen kjørt på nytt med den flettede binæren, med samme resultat.

Testbibliotekene og fasitfilene ligger i `~/dev/fotomaxxer-testlib/` (private bilder, ikke i repoet).

**Fase 3 (derive).** `image`/zune-jpeg og `fast_image_resize` ga 40–49 dB mot Python: små dekodingsavvik (IDCT, oppsampling) forsterkes når thumbnailen JPEG-kodes på nytt. Derfor er hele kjeden portert direkte fra Pillow 12.2 og libjpeg-turbo: `jpegdec` (baseline/progressiv, restart, CMYK/YCCK, libjpegs feil- og avkuttingssemantikk), `resample` (`thumbnail` med `reducing_gap`, `reduce`, Lanczos med 22-bits fastpunkt) og `jpegenc` (islow-DCT, 4:2:0 som `h2v2_downsample`, standardtabeller). Resultatet er byte-identiske filer. Løkkene kjøres med AVX2 når CPU-en har det (samme heltallsregning). Kjente avvik, bare i ødelagte filer: libjpegs blokkutjevning av ufullstendige progressive filer etterlignes ikke, og ekstreme koeffisienter kan gi andre piksler enn libjpeg-turbos SIMD-IDCT. Aritmetisk og tapsfri JPEG går via `image`. Feiltekstene følger Pythons `"{Type}: {melding}"`; libjpeg-detaljer legges til i parentes.

### Fase 6 (API)

**Ferdig 2026-10-23.** `crates/api` (`fotomaxxer-api`, axum 0.8 + tokio) porterer hele FastAPI-appen. Trinn A la grunnmuren og portet `settings.py`, `sources.py`, `profiles.py`, `fs.py` og `jobs.py` (inkludert «Oppdater alt», `/progress` og `/cost`). Trinn B portet `persons.py` og `albums.py` på én gren og `images.py` og `export.py` på en annen. Begge ble flettet inn i `rust-fase6` og verifisert samlet. Én fil per Python-modul:

| Python | Rust | Merknad |
| --- | --- | --- |
| `api/app.py` | `lib.rs` (`create_app`, `serve`), `spa.rs` | `/api/*`, `/derived/*`, `/assets/*` og SPA-fallback med Starlettes regler for 404/405, `HEAD`, 304 og MIME |
| `api/deps.py` | `state.rs`, `validate.rs` | `DbReq` åpner én tilkobling per forespørsel (`connect` + `init_db`) etter at kroppen er lest og før valideringen, som `Depends(get_conn)` |
| `api/schemas.py` | `schemas.rs` | Alle modellene, pluss sti- og spørremodellene (`ImagesQuery` med `ge`/`le` osv.) |
| `api/jobs.py` | `jobs.rs` | `JobRunner`: én jobb om gangen, egen OS-tråd, egen tilkobling, `Feil: {Type}: {melding}` |
| `api/desktop_access.py` | `desktop_access.rs` | Mellomvare; aktiv bare med `DesktopConfig` (fase 7) |
| `api/routers/*.py` | `routers/*.rs` | Én fil per ruter. `routers/image_query.rs` har `_build_filters`/`_thumb_url`/bildelista som både `images.py` og `persons.py` bruker; `routers/export/pyword.rs` har tabellen over Pythons Unicode-`\w` |

Viktige valg:

- **Feilformat.** `HTTPException` → `{"detail": …}`; ruterfeil som Starlette (`{"detail":"Not Found"}`, `{"detail":"Method Not Allowed"}` med `allow` fra første rute på stien, `HEAD` på `GET`-ruter gir 405). 422 lages av en liten pydantic-port (`validate.rs`): lax-koersjon målt mot pydantic 2.13 (`"1.0"`→1, `"yes"`→true, `true`→1 …), feiltyper og -tekster ordrett, feil samlet i FastAPIs rekkefølge (sti, spørring, kropp), `missing` med hele kroppen som `input`, `model_attributes_type` for kropp uten JSON-innholdstype (FastAPI 0.137 er streng på `Content-Type`), og `json_invalid` med Pythons egne meldinger og tegnposisjoner (en port av `_json.c`-skanneren). Ufangede feil gir 500 `text/plain` «Internal Server Error».
- **Ruting som Starlette.** I Starlette slår et fullt treff et tidligere delvis treff (riktig sti, feil metode). To steder gir det et annet svar enn axums valg, og begge er gjenskapt:
  - Med SPA (`web/dist`) er `GET /{full_path:path}` et fullt treff for alle `GET`-forespørsler. `GET` mot en API-sti som bare har andre metoder (`/api/export`, `/api/jobs/update`, `/api/albums/{id}/order`, `/api/sources/1`, `/api/settings/x` …) gir derfor 404 «ukjent endepunkt» i FastAPI, ikke 405. `Routes::build` (`routers/mod.rs`) legger en egen `GET`-håndterer på slike stier som svarer som SPA-fallbacken; uten SPA gir den 405 med `allow` som før. Det må være en egen håndterer og ikke `fallback`, fordi axum ellers legger på `allow`. Feilen fantes i trinn A og ble funnet uavhengig av begge trinn B-grenene; de to rettelsene var like, og flettingen beholdt én. Dekket av `api-export.json`, `api-albums.json`, enhetstesten `get_paa_sti_uten_get_rute` og serve-røyktesten i CI.
  - `PATCH /api/images/ids` går til `PATCH /images/{image_id}` og gir 422 for `image_id="ids"`. axum velger den statiske stien, så `images.rs` har en egen håndterer som gjenskaper svaret.
- **JSON-bytes.** Svar med `response_model` serialiseres som pydantic (`model_dump_json`): kompakt, UTF-8 og pydantics flyttallsformat (`0.0000535`, `1e-7`, `1e+16`). `dict`-svar og feil går via `json.dumps` med Pythons `repr` (`5.35e-05`). Derfor to svartyper: `JsonResponse` og `DictResponse` (`core::pyjson::dumps_pydantic`/`dumps_compact`).
- **SPA-en bakes inn** med `rust-embed` (`#[allow_missing]`). Bygg frontend først: `cd web && npm ci && npm run build`, så `cargo build --release`. `crates/api/build.rs` bygger craten på nytt når `web/dist` endres. I debug-bygg leses `web/dist` fra disk ved hver forespørsel. Mangler `web/dist`, bygger alt likevel, og tjeneren oppfører seg som Python uten `web/dist` (ingen SPA). `web/dist` ligger ikke i git; CI bygger den med Node før cargo.
- **`/derived`** serveres fra datamappa slik den var ved oppstart (som Starlettes mount) med Starlettes ETag (`md5(f"{st_mtime}-{st_size}")`), så ETag og `last-modified` er like mellom Python og Rust. `relocate_data` kopierer som `shutil.copy2` (bevarer mtime) og gir Pythons `OSError`-tekst (`[Errno 20] Not a directory: '…'`).
- **Jobbene** kaller `fotomaxxer_core` direkte. `workers <= 0` gir `ValueError` bare når Python ville laget en prosesspool (det finnes arbeid), uten å endre databasen.
- **Persons og albums.** Mutasjonene kaller kurateringsfunksjonene i `core::faces` uendret (`Error::Faces` blir 404 eller 400, som Pythons `ValueError`). `PUT /faces/{id}` bruker `body_fields` (`model_fields_set`), så `{"locked": null}` låser opp og `{}` gir 400. `GET /persons/{id}/images` bruker `image_query::list_page` med bare `person` satt; andre filtre i spørringen ignoreres, som i Python. Navn trimmes som Pythons `str.strip()` (også `\x1c`–`\x1f`). Albumposisjonene følger Python også i hjørnene: nye bilder får `MAX(position) + 1` og oppover, og duplikater og ukjente id-er hoppes over uten å bruke en posisjon. Ved omorganisering bruker ukjente id-er opp en indeks, og siste duplikat vinner.
- **Eksport som `shutil.copy2`.** Stiene vises som i `pathlib` (`//`, `/./`, `/` til slutt, relativ sti, `~` og `~bruker`; ukjent bruker gir 500 som `RuntimeError`). Kopieringen følger `copy2`: mappe som mål, `SameFileError` før originalen åpnes for skriving (eksport med originalnavn til mappa der originalen ligger), navngitte rør, mtime/atime og rettigheter. Jobbfeil får Pythons `OSError`-underklasse og tekst (`FileExistsError: [Errno 17] File exists: '…'`, `ValueError: mkdir: embedded null character in path`). `_safe_dirname` bruker en tabell over Pythons Unicode-`\w` (Python 3.14, Unicode 16.0), fordi Rusts `is_alphanumeric` også godtar kombinerende tegn. `core::storage` gjør `py_repr`, `py_os_error` og `mkdir_parents` offentlige for dette.
- **`serve`**: samme flagg (`--host 127.0.0.1`, `--port 8000`, `--reload` → feil og kode 2, som den frosne appen), samme første linje, `--home` setter `FOTOMAXXER_HOME`. Ctrl-C/SIGTERM avslutter pent: jobbporten stenges, og en pågående jobb fullføres før prosessen avslutter med kode 0.
- **`paths::resolve`** er på Unix en port av `posixpath.realpath(strict=False)` for stier som ikke finnes, så symlenker med manglende mål løses som i Python (fanget av `api-fs.json`).

**`compare_api.py`.** Starter `py_fotomaxxer.py serve` (master-venv) og `fotomaxxer serve` på hver sin ledige port, med hver sin kopi av datamappa (`--seed-dir`/`seed_dir`, kopiert med bevarte tidsstempler), egen `XDG_CONFIG_HOME` med pekerfil (så flytting av datamappa virker), egen arbeidsmappe, en felles falsk `HOME` og samme `web/dist` (`FMX_WEB_DIST`). OpenRouter-URL-en peker alltid på `fake_openrouter.py` eller en lukket port. Hvert steg sammenlignes på status, `content-type`, `content-length`, `allow`, `accept-ranges` (pluss valgfrie headere) og normalisert kropp byte for byte; stier, tidsstempler, porter og jobb-id-er normaliseres. Til slutt stoppes tjenerne med SIGINT, og `serve`-utskriften og hele databasen (`iterdump`, valgfri `mask_sql`/`mask_errors`) sammenlignes. Scenarioformatet (steg med `method`/`path`/`query`/`json`/`body`, `wait` på jobber, `fields`, `expect_status`, `check_files`, `var` fra SQL, `exec_sql` for oppsett API-et ikke har, plassholderne `{cwd}`, `{impl}` m.fl.) er dokumentert i skriptets docstring. `api-export.json` kjøres med `--library` på en kopi av scanlib der de absolutte symlenkene peker inn i kopien (scenarioet eksporterer til mappa der originalen ligger for å treffe `SameFileError`).

Alle 12 scenarier kjørt med den flettede release-binæren (765 forespørsler og 22 filsjekker):

| Scenario | Innhold | Forespørsler | Resultat |
| --- | --- | --- | --- |
| `api-feilformat.json` | 404/405/422/400/500, `json_invalid`, health, SPA, `/assets` (304 på ETag og dato), `/derived` | 87 | IDENTISK |
| `api-settings.json` | Innstillinger, maskering, `default_profile_id`, flytting av datamappa (beholde, slette, `~`, feil under en fil, no-op), klargjøring for deling | 32 | IDENTISK |
| `api-sources.json` | Legg til/duplikat/feil/symlenke/æøå/`~`/FK, sett profil, skann, derive, fjern med orphan-opprydding (thumbnails byte-like) | 31 | IDENTISK |
| `api-profiles.json` | Normalisering, duplikat, tomt navn, rediger, slett med opprydding | 22 | IDENTISK |
| `api-fs.json` | Hjemmemappe, undermapper, skjulte, sortering, symlenker (også brutt), `~`, relativ, rot, fil, ingen tilgang | 17 | IDENTISK |
| `api-jobber-golden.json` | py-golden: «Oppdater alt» med og uten nøkkel, tag mot falsk OpenRouter (pydantic-flyttall i `/cost`), full og inkrementell klynging, no-op-steg, skann, ugyldige `workers`, `/derived` med ETag | 54 | IDENTISK |
| `api-jobber-scanlib.json` | Ny pakke + scanlib: skann, derive (byte-like), faces-detect med 409 «busy» underveis, klynging, «Oppdater alt» per kilde | 35 | IDENTISK med maskering av embeddings/bbox/klynger og derive-feiltekster (kjente avvik fra fase 3 og 4) |
| `api-migrering.json` | v2-database (`fotomaxxer-data`) migreres ved første forespørsel, også en 422 | 12 | IDENTISK |
| `api-images.json` | Kopi av py-golden med `prep_sql` (steder og tagger med æøå, vanskelige GPS-flyttall, ekskluderte bilder, duplikatfiler, AI-tagger, personnavn, låste ansikter) og tag-jobb mot `fake_openrouter`: alle filtre alene og kombinert, `limit`/`offset`-grenser og 422, `/images/ids`, detalj, `PATCH` (også `PATCH /images/ids`) og fasetter | 204 | IDENTISK |
| `api-persons.json` | py-golden med noen bilder ekskludert: personliste og cover (låst først, ekskluderte skjult), rutenett, `/persons/{id}/images` med paginering, lax-tall og ignorerte filtre, navn med æøå, blanke navn og `\x1c`–`\x1f`, splitt, flytt/lås/lås opp/ignorer ansikt med delvis kropp, sammenslåing (også med seg selv), sletting, ignorering, 400/404/405/422/500, til slutt inkrementell og full klynging (låste ansikter og navn respekteres) | 110 | IDENTISK |
| `api-albums.json` | py-golden: opprett (trim, æøå, blankt navn), liste, detalj, legg til (posisjoner, duplikater, ukjente id-er, ekskludert bilde, samme bilde i to album), omorganiser (ukjent id, duplikat, delvis liste), fjern element, slett (cascade), tomt album, 404/405/422 | 59 | IDENTISK |
| `api-export.json` | Kopi av scanlib: album og utvalg, manglende original, navnekollisjon, ugyldige mål (fil, under fil, uten tilgang, NUL), stinormalisering, relativ sti og `~`, standard eksport-mappe, 409 under en stor eksport, `SameFileError`, original som er en mappe, `GET` på stier uten `GET`-rute; filene på disk sjekkes med innhold og mtime (13 filsjekker) | 102 | IDENTISK |

**Regresjon etter flettingen.** `compare_cli.py --scenario fase1`, `--scenario scan` (scanlib) og `--scenario migrering` (`fotomaxxer-data`) er identiske. `scenarios/derive.json` gir to avvik i feiltekster for ødelagte filer: Pillow skriver `image file is truncated (3 bytes not processed)` der Rust skriver `image file is truncated`, og Rust legger til libjpeg-detaljen `(Unsupported marker type 0x78)` på `broken data stream`. Med `--mask-errors` er utskrift og database identiske. Tekstene er de samme i fase 3-koden (`b36083c`) og i binæren fra `rust` før fase 6, så dette er ikke en regresjon; fase 3-raden i fremdriftstabellen er rettet.

**Nettlesertest (2026-10-23).** React-appen (`web/dist` bakt inn i release-binæren) ble kjørt i Chromium via chrome-devtools mot `fotomaxxer serve` på en kopi av py-golden (926 bilder, 649 ansikter, 38 personer). Kopien fikk AI-tagger via en tag-jobb mot `fake_openrouter.py` og stedsnavn via SQL. Følgende flyter fungerte, uten uventede 4xx/5xx i nettverksfanen og uten feil i konsollen (bare Chromes merknader om skjemafelt uten `id`/`label`): bildegalleriet med scrolling (virtuoso), filtre på sted, person, tagg, hendelse, sesong og år, både alene og kombinert, og fritekstsøk; bildedetalj i lightbox med ekskludering og gjeninnsetting (også fra rutenettet og med «Vis ekskluderte»); navngiving med æøå, sammenslåing, flytting av ansikter, «Ny person», ignorering av ansikt og «Kryss bort»; album med oppretting, tillegg, endret rekkefølge med dra og slipp, fjerning og eksport til en midlertidig mappe (byte-like filer med bevart mtime, i albumrekkefølge); innstillinger med modell, terskel, standard eksportmappe, ny profil og standardprofil; kilder med mappevelger (`/api/fs/list`), «Bruk sti», fjerning og «Oppdater alt» med fremdriftsvisning over tre kilder (skann, derive med tre ødelagte filer som myk feil, tag, ansikter og klynging). De samme muterende kallene ble spilt av mot Python-tjeneren på en egen kopi, og 18 av 23 lesekall ga samme svar. De fem som avvek (`/progress`, `/persons`, `/facets`) skyldes én klynge på tre ansikter som Python fant og Rust ikke fant ved inkrementell klynging av de nye ansiktene. Det er det kjente HDBSCAN-avviket fra fase 4. Eksportmappene var identiske.

Kjente avvik (ingen kan utløses av frontend):

- ETag for innebygde SPA-filer er SHA-256-basert, og `last-modified` er byggetidens filtid i hele sekunder (Python bruker filas `st_mtime`). `/derived` er likt.
- Range-forespørsler (206) støttes ikke; hele filen sendes. Uten `web/dist` gir stier med skråstrek til slutt 404 i stedet for Starlettes 307-omdirigering.
- `serve` skriver ikke uvicorns loggelinjer og avslutter med kode 0 etter Ctrl-C (Python løfter SIGINT på nytt og avslutter med `KeyboardInterrupt`).
- Heltall utenfor `i64` gir 500 med en gang (Python godtar dem og feiler når de når SQLite). JSON-kropper må være UTF-8; `NaN`/`Infinity` og ensomme surrogater gir 500.
- Jobbfeil får Pythons unntaksnavn der det finnes en motpart (`TaggingError`, `ValueError`, `OperationalError`, `OSError` …), men tekster fra SQLite/OS kan avvike i ordlyd; en panikk blir `Feil: panic: …`. Windows-tekster for `OSError` (`[WinError N] …`) er ikke sammenlignet.
- Websocket-oppgraderinger støttes ikke (desktop-porten i Python lukker dem med 1008).
- Som i Python serveres `/derived` fra mappa tjeneren startet med, også etter en flytting med `delete_source` (da mangler thumbnails til omstart).
- Eksport: `~bruker` slås opp i `/etc/passwd` (Python bruker `getpwnam`, som også ser andre NSS-kilder). Feil midt i kopieringen (`ENOSPC`, `EIO`) får Pythons form (`[Errno N] …: 'src' -> 'dst'`), men er ikke sammenlignet. Utvidede attributter (xattr) kopieres ikke; Pythons `copystat` kopierer `user.*`. På Windows er stiteksten i eksportmeldingen og i `SameFileError` tilnærmet (`std::path` i stedet for `ntpath`), og `CopyFile2` etterlignes ikke.
- Klynging og ansikter arver avvikene fra fase 4 (støy i embeddingene kan gi en klynge mer eller mindre ved HDBSCAN), og derive-feiltekstene arver avvikene fra fase 3 (se «Regresjon» over).

**CI.** `rust.yml` bygger `web/dist` med Node 20 før cargo, og en serve-røyktest kjører release-binæren på både Windows og Linux: SPA (`/`, en klientrute, en JS-ressurs), health, settings, progress, `/fs/list`, opprett og les album, 422, 404 «ukjent endepunkt» for `GET /api/jobs/update` og 405 for `PUT`, en klyngejobb (202 og ferdig), første utskriftslinje og at databasen er laget. På Linux stoppes tjeneren med SIGTERM og må avslutte med kode 0. På Windows stoppes den med `taskkill`, fordi Git Bash ikke kan sende signaler til en Windows-prosess (pen avslutning hører til fase 7).

Desktop-vindu, sesjonstoken og pakking er fase 7.

### Fase 7A (desktop)

`crates/app` har nå et bibliotek (`fotomaxxer_app::desktop`) og to binærer: `fotomaxxer` (CLI, med underkommandoen `desktop`) og `fotomaxxer-gui` (`windows_subsystem = "windows"`, flaggene `--home`, `--smoke` og `--smoke-image`). Én modul per Python-fil:

| Python | Rust (`crates/app/src/desktop/`) | Merknad |
| --- | --- | --- |
| `desktop.py` | `mod.rs`, `log.rs` | Datamappe som `data_root` (pekerfila skrives ved første valg av standarden), `init_db` før tjeneren, `logs/desktop.log` med rotasjon 2 MB × 3 og logget panikk, feildialog med stien til loggen |
| `desktop_server.py` | `server.rs` | Socketen bindes på `127.0.0.1:0` før tjenertråden (egen tokio-runtime) starter; `DesktopConfig` gjør tilgangsporten aktiv |
| `desktop_window.py` | `window.rs`, `instance.rs`, `dialog.rs` | tao + wry, 1280 × 850, inkognito-webview, navigasjon bort fra origin avvises, nye vinduer og nedlastinger blokkeres |
| `desktop_smoke.py` | `smoke.rs` | Ekte vindu; rapporten er `desktop-report.json` |

Valg:

- **Sesjons-cookie.** wry kan ikke legge headere på underforespørsler. Vinduet åpner derfor `/__fotomaxxer/session?code=<engangskode>`, som setter `fotomaxxer_session_<port>=<token>; HttpOnly; SameSite=Strict; Path=/` og svarer 303 til `/`. `desktop_access::is_allowed` godtar headeren eller cookien, med de samme kravene til Host, Origin og `Sec-Fetch-Site`. Koden kan bare brukes én gang. Porten står i cookienavnet fordi cookies ikke skiller på port.
- **Én instans** med en OS-lås på `.desktop.lock` (`File::try_lock`, slippes når prosessen dør) og en TCP-kanal på loopback: port og en tilfeldig hemmelighet ligger i `.desktop-instance.json`. En ny start sender `activate <hemmelighet>`, vinduet hentes frem med en tao-hendelse, og den nye prosessen avslutter med kode 0.
- **Avbrudd ved trygge punkter** (`core::cancel`). `JobRunner::request_cancel` setter et flagg per jobb. Jobbtråden kjører jobben inne i `cancel::scoped`, og scan (per fil), derive (per bilde), tag og faces detect (per resultat) sjekker flagget etter at det ferdige er committet. «Oppdater alt» sjekker også mellom stegene. Arbeidstrådene gjør ferdig bildet de holder på med. Det som ikke ble skrevet, står som pending, så neste kjøring fortsetter. Flagget er tråd-lokalt, så CLI-en, `serve` og testene oppfører seg som før. Etter et avbrudd er `current.message` «Avbrutt. Det som var ferdig er lagret; neste kjøring fortsetter.».
- **Lukking:** kjører en jobb, spør en dialog «Avbryt jobben og lukk» / «Fortsett å bruke appen». Velger man å lukke, stenges jobbporten (skrivende forespørsler gir 503), jobben avbrytes, og vinduet lukkes først når tjenertråden er ferdig. Åpne forbindelser får 5 s. På Linux brukes GTK direkte, fordi rfd ville startet sin egen GTK-tråd ved siden av taos. På Windows brukes rfd (`MessageBoxW`) med Ja/Nei og valgene forklart i teksten, fordi egne knappetekster krever comctl32 v6 og et manifest.
- **Reserve:** kan hendelsesløkken, vinduet eller webview-en ikke lages, åpnes standardnettleseren med en engangslenke (`rundll32 url.dll,FileProtocolHandler` / `xdg-open`). Deretter vises «FotoMaxxer kjører i nettleseren … Trykk OK for å avslutte». En ny start åpner nettleseren igjen. Uten skjerm venter prosessen på Ctrl-C/SIGTERM. Avslutningen avbryter jobben som fra vinduet.
- **WebView2-data** ligger i `%LOCALAPPDATA%\FotoMaxxer\WebView2` (i røyktesten under rapportmappa), ikke ved siden av exe-en og ikke i datamappa, så pakka forblir flyttbar. WebKitGTK kjører i inkognito.
- **`faces detect`** bruker som standard maks 2 analysatorer (ca. 0,9 GB) i stedet for 4. `--workers`/`workers` gjelder som før.
- **Røyktesten** (`--smoke <ny mappe> --smoke-image <bilde>`) sjekker at React monteres, at vinduet viser appens origin, at navigasjon til `https://example.com/` og `file://` avvises og `window.open` ikke åpner noe, at API-et uten token og med fremmed Origin gir 403, at en ny start av samme exe med samme `--home` fokuserer instansen og avslutter med 0, at «Oppdater alt» på testbildet finner minst ett ansikt (uten nøkkel, så tag hoppes over), og at lukking mens «Oppdater alt» er i ansiktssteget (24 bildevarianter) avbryter ved et trygt punkt: jobben får avbruddsmeldingen, resten står som pending, `integrity_check` er `ok`, og tjener og jobb er stoppet før prosessen avslutter. Den tar ca. 10 s lokalt og i CI (xvfb på Linux, WebView2 på `windows-2022`). Hendelsesløkken vekkes av en egen taktråd hvert 100. ms, fordi `ControlFlow::WaitUntil` under xvfb bare kom hvert ~10. s.

Utsatt:

- Lukking av vinduet mens en eksport kjører venter til eksporten er ferdig (eksporten har ingen trygge punkter ennå). Klyngingen er ett steg og avbrytes ikke midt i.
- Ingen ikon eller versjonsressurs i exe-en (hører til pakkingen).
- Lenker til eksterne nettsider fra grensesnittet åpnes ikke i nettleseren. De blokkeres, som i Python.
- Tidspunktene i loggen er UTC.

### Fase 7B (pakking)

- **Statisk C-runtime.** `.cargo/config.toml` setter `+crt-static` for `x86_64-pc-windows-msvc`, og libheif bygges med vcpkg-tripletten `x64-windows-static`. pykes prebygde `onnxruntime.lib` (som `ort` laster ned) er bygd med `/MD` og lar seg ikke lenke med `/MT` (`RuntimeLibrary`-konflikt). CI bygger derfor ONNX Runtime 1.31.0 fra kilde med `packaging/windows/build-onnxruntime.ps1`: pykes eget oppsett for én statisk `.lib` (`ort-artifacts`, festet til en commit, med patchene deres), men med `MultiThreaded`, uten DirectML og uten `/arch:AVX2`. Resultatet caches (`.github/actions/onnxruntime-static`, ~1 time uten cache), og `ORT_LIB_PATH` peker `ort-sys` dit. Lokalt Windows-bygg trenger det samme (skriptet kan kjøres i en Developer PowerShell).
- **DirectML.** `DirectML.dll` kom fra pyke: alle deres Windows-bygg har DirectML, `ort-sys` lenker da `DirectML.lib`/`D3D12.lib`, og standardfunksjonen `copy-dylibs` legger `DirectML.dll` ved exe-en. Med eget CPU-bygg forsvinner alt dette.
- **Uten AVX2-krav.** pyke bygger med `/arch:AVX2`, så en CPU uten AVX2 (enkelte Celeron/Pentium/Atom) ville krasjet. MLAS velger AVX2-kjerner ved kjøring, så farten er den samme på nyere CPU-er.
- **libheif bare med dekoding** (`libheif[core,libde265]`): standardfunksjonen `hevc` drar inn x265-koderen, som er GPL og ikke brukes. Det sparer også ~9 min i et bygg uten cache.
- **DLL-sjekk.** `packaging/windows/check-dependents.ps1` kjører `dumpbin /DEPENDENTS` og feiler på alt som ikke følger med Windows (også `api-ms-win-crt-*`). WebView2-lasteren lenkes statisk av `webview2-com-sys` (`WebView2LoaderStatic.lib`).
- **Fontene** kommer fra `@fontsource-variable/*` og bundles av Vite (lisenser i `web/FONT-LICENSES.txt`).
- **Installasjonsprogram** (`packaging/windows/fotomaxxer.nsi`, NSIS): per bruker til `%LOCALAPPDATA%\Programs\FotoMaxxer`, snarvei i Start-menyen, oppføring under Apper (HKCU). Avinstalleringen sletter bare sine egne filer (ingen `RMDir /r`), aldri datamappa `%LOCALAPPDATA%\FotoMaxxer` eller pekerfila. `/S` gir stille modus for begge.
- **`windows-release.yml`** bygger `FotoMaxxer-Setup-<versjon>.exe`, installerer stille på runneren, sjekker filer/snarvei/register, kjører den installerte appen og CLI-en (init, kilde, scan, derive, ansikter med de installerte modellene), avinstallerer stille og sjekker at datamappa er urørt. Tagg `v*` gir GitHub Release (prerelease ved bindestrek). Med `GUI_KLAR: 'true'` er `FotoMaxxer.exe` = `fotomaxxer-gui.exe`, og `--smoke` kjøres på den installerte exe-en (`'false'` er reserven fra før 7A ble flettet: CLI-kopi med `--version`).
- **Cache og tagger.** GitHub Actions lar en kjøring bruke cache fra sin egen ref og fra standardgrenen. I det gamle repoet var standardgrenen `master`, så tagger bygde ONNX Runtime, libheif og cargo uten cache (ca. 2 timer). I det nye repoet er `main` standardgren, så tagger får cachen fra `main`. `timeout-minutes: 300` i `windows-release.yml` er beholdt for kjøringer uten cache.
- **Artefaktkvoten** gjelder hele GitHub-kontoen. Når den er full, feiler opplastingen av Setup-artefakten (steget har `continue-on-error`). Releasen laster opp fila direkte og påvirkes ikke.
- **Utsatt:** ikon og versjonsinfo i selve exe-ene (`winresource`), kodesignering, oppgradering som lukker en kjørende app automatisk, og lisensgjennomgang (libheif/libde265 er LGPL og lenket statisk; InsightFace-modellene er bare til ikke-kommersiell bruk).

## Risikoer

| # | Risiko | Tiltak |
| --- | --- | --- |
| R1 | Embeddings avviker mer enn kravet | Sammenlign mellomresultater steg for steg mot Python (skalert tensor, rå SCRFD-output, justert 112 × 112-crop). Reserve: kjør detect på nytt og bruk `cluster --full` med navnemapping |
| R2 | `libheif` er vanskelig å bygge eller pakke på Windows | Prøves i fase 0. Alternativer: vcpkg, en ferdigbygd DLL ved siden av exe-fila, eller å dekode HEIC via Windows' egen WIC-kodek |
| R3 | Rust-EXIF gir andre verdier enn ExifTool | Fasittest på testbiblioteket. Avvik i dato/GPS rettes. `width/height` får alltid en verdi |
| R4 | HDBSCAN i Rust gir andre klynger enn sklearn | Merkene sammenlignes for faste embeddings (ARI ≈ 1). Er det avvik, porteres sklearns algoritme (core distance, MST, kondensert tre, EOM) direkte. Løst i fase 4: algoritmen er portert, og etikettene er bit-like (se «Resultat fra fase 4») |
| R5 | WebView2 mangler på maskinen | Følger med Windows 11. Desktop-modus faller tilbake til standardnettleseren |
| R6 | Lisens for `buffalo_l`-modellene | Uendret fra i dag: ikke-kommersiell bruk. Greit for privat bruk, men må avklares før eventuell videre distribusjon |

## Avklarte valg (2026-10-23)

1. **Testbibliotek:** Et utvalg fra Simens eget bildearkiv for 2011 (926 JPEG-er). Arkivet har ingen HEIC- eller PNG-filer, så disse lages av JPEG-ene. Det samme gjelder orienteringsvarianter, duplikater og korrupte filer. Biblioteket og fasiten ligger **utenfor repoet** i `~/dev/fotomaxxer-testlib/`, fordi det er private bilder og embeddings. CI bruker et lite syntetisk eller offentlig sett.
2. **Eget vindu fra start.** Desktop-modus bruker Tauri/wry med WebView2 på Windows og WebKitGTK på Linux. Standardnettleseren brukes bare som reserve hvis vinduet ikke kan opprettes.
3. **CI bygger både Windows og Linux** fra fase 0.
4. **Fase 7, pakking:** **installasjonsprogram** (NSIS-`Setup.exe` med snarvei i Start-menyen og avinstallering under Apper; avinstallering sletter aldri datamappa `%LOCALAPPDATA%\FotoMaxxer`). CI lager en **GitHub Release** når en versjonstagg (`v*`) pushes, i tillegg til artefakt ved `workflow_dispatch`. **Simen tester** på en ren Windows 11-maskin etter en sjekkliste før fila deles videre. Første versjon skal bare virke; vi itererer på bygget etterpå.
5. **Fase 7, tekniske valg:** statisk C-runtime (ingen VCRUNTIME140/MSVCP140), ONNX Runtime bare med CPU (ingen DirectML.dll), fontene bakes inn i `web/` så appen virker uten nett, og exe-fila er usignert, med SmartScreen-veiledning («Flere detaljer → Kjør likevel»). GUI-exe-en åpner uten konsollvindu, og CLI-en finnes som egen konsoll-exe (dagens Python-pakke bruker `FotoMaxxer.exe` + `FotoMaxxer-cli.exe`; navnene kan ikke bare skille på store/små bokstaver på Windows).

## Neste steg (fase 7)

Status 2026-10-24: fase 7 er bygget og verifisert i CI (punkt 1–4 under), og prereleasen `v0.1.0-rc.1` er laget (i arkivrepoet; det nye offentlige repoet får sin egen prerelease av samme kode). Det som gjenstår, er Simens test på ren Windows 11 etter [windows-testsjekkliste.md](windows-testsjekkliste.md). Installasjonsveiledningen er [desktop-windows-install.md](desktop-windows-install.md). Fase 8 startes først når testen er godkjent.

Punktene slik de ble gitt for fase 7:

1. **Desktop-modus:** vindu med tao + wry (WebView2/WebKitGTK) mot den innebygde axum-tjeneren, sesjonstoken (se `api/desktop_access.py` og `crates/api/src/desktop_access.rs`), én instans per datamappe (hent frem eksisterende vindu), trygg lukking mens en jobb kjører, avbrudd ved trygge punkter, logg i `logs/desktop.log`, reserve til standardnettleser og en `--smoke`-modus som CI kan kjøre. Motparter i Python: `fotomaxxer/desktop.py`, `desktop_window.py`, `desktop_server.py`, `desktop_smoke.py`.
2. **Pakking og CI:** statisk CRT, ORT uten DirectML, innebygde fonter, modellene (`det_10g.onnx`, `w600k_r50.onnx`) i `models/` ved siden av exe-en, NSIS-installasjonsprogram, release-workflow på tagg og artefakt ved `workflow_dispatch`. Sjekk at `Setup.exe` installerer og avinstallerer stille på `windows-2022`-runneren.
3. **Minne:** `faces detect` bruker ca. 430 MB per analysator (1,7 GB med 4). Velg en standard som tåler en svak Windows-maskin.
4. **Sjekkliste** for Simens test på ren Windows 11, og ny `docs/desktop-windows-install.md` for Rust-pakka.
5. **Fase 8** etter godkjent test: slett Python-koden og de gamle workflowene (`build-desktop-windows.yml`, `build-windows-bundle.yml`), og oppdater `CLAUDE.md`/README/dokumentasjonen. Flettingen `rust` → `master` faller bort, fordi det nye repoet bare har `main`.

Funn i frontenden fra nettlesertesten i fase 6 (likt i Python-versjonen, tas etter porten): biblioteket viser bare de første 200 bildene, kildelisten oppdateres ikke mens en jobb kjører, personfilteret teller ansikter mens bildelisten teller bilder, «Bruk sti» i mappevelgeren legger til kilden med en gang, og Chrome klager på skjemafelt uten id/label.

**Arbeidsmiljø utenfor repoet** (trengs for sammenligningene mot Python, ikke for å bygge): Python-fasiten er Python-koden i repoet (identisk med `master` i arkivet) med `pip install -e .` i en venv, ansiktsmodellene ligger i `~/.insightface/models/buffalo_l` (lastes ned av CI), og de private testbibliotekene ligger bare på Simens PC i `~/dev/fotomaxxer-testlib/` (`golden` 3 MB, `py-golden` 144 MB, `scanlib` 694 MB). Regler: aldri ekte OpenRouter-kall i tester (bruk `tools/golden/fake_openrouter.py`), og aldri private testdata i repoet.
