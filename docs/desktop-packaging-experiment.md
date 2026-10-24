# Desktop-pakkeutprøving

Dette implementerer beslutningspunktet i [desktop-planen](desktop-plan.md): pakk
den eksisterende motoren, og bruk den deretter i eget Qt-vindu. Resultatet er en
PyInstaller-pakke i mappemodus med `FotoMaxxer` for desktop og `FotoMaxxer-cli`
for kommandolinje. Begge bruker samme støttefiler: React, Qt WebEngine, FastAPI,
native bildebiblioteker, ExifTool og lokale buffalo_l-modeller. Den eksisterende
utviklingsstarten fungerer fortsatt. Installer er en senere leveranse.

## Start desktop-utgaven

Kjør `dist/FotoMaxxer/FotoMaxxer` på Linux, eller `FotoMaxxer.exe` på Windows.
Hele mappa må være med. GUI-inngangen åpner eget vindu; Windows-bygget bruker
PyInstallers windowed-modus uten permanent konsollvindu.

Datamappa velges i rekkefølgen `--home` → `FOTOMAXXER_HOME` → eksisterende
bootstrap-pekerfil. Uten eksisterende valg bruker desktop-starten
`$XDG_DATA_HOME/fotomaxxer` (ellers `~/.local/share/fotomaxxer`) på Linux, og
`%LOCALAPPDATA%\FotoMaxxer` på Windows. Førstegangsstart oppretter skjema og
derived-mappe og lagrer peker først etter vellykket initialisering. CLI-startens
fallback til arbeidsmappa er beholdt.

```bash
dist/FotoMaxxer/FotoMaxxer --home '/sti/til/testbibliotek'
# Fra kildekode, med PySide6 installert:
.venv/bin/python -m fotomaxxer.desktop --home '/sti/til/testbibliotek'
```

En ny desktop-start med samme datamappe fokuserer det eksisterende vinduet via
QLockFile og QLocalServer. Ved lukking under en jobb kan du velge «Vent og lukk»
eller «Fortsett å bruke appen». Venting stenger nye skrivekall og jobber, lar
aktiv jobb lukke sin databaseforbindelse og prosesspool, og stopper deretter
API-tjeneren. Det finnes foreløpig ingen avbryt-knapp for pipeline-jobber.

Loggen ligger i `<datamappe>/logs/desktop.log`, med rotasjon. Oppstartsfeil som
launcheren kan håndtere vises i en dialog med loggsti. Ingen av prøvene nedenfor
åpner produksjonsbiblioteket.

## Bygg på Linux

Bygg på Linux x86-64. Byggemaskina trenger Python, Node/npm, ExifTool og Perl.
ExifTool og Perl tas med i pakken; de er ikke forutsetninger på kjøremaskina.
Modellene hentes fra en eksplisitt lokal mappe, uten automatisk nedlasting.

```bash
python3.14 -m venv .venv
.venv/bin/python -m pip install -r packaging/requirements-linux-x86_64-py314.txt
.venv/bin/python -m pip install --no-deps -e '.[desktop]'
.venv/bin/python scripts/build-desktop-probe.py \
  --models ~/.insightface/models/buffalo_l \
  --exiftool /usr/bin/vendor_perl/exiftool
```

ExifTool-stien varierer mellom distribusjoner; bruk `command -v exiftool` på
byggemaskina. Byggeskriptet bruker Perls `Config` til å finne core- og
arkitekturmodulene, og ExifTool-modulene under `vendorlib`. `--perl` kan angi en
annen Perl-binær med tilhørende moduler. Et separat nedlastet ExifTool-arkiv må
installeres i dette byggemiljøet først.

Frontend bygges med `npm ci` og `npm run build`. `--skip-frontend` gjenbruker et
allerede bygget `web/dist`; bruk bare flagget når det bygget er oppdatert.

Python-fila med Linux-versjoner er et snapshot av det prøvde CPython 3.14.7-miljøet,
ikke en verifisert Windows-lock eller et bygg for Ubuntu. PyInstaller og
pakkehooks er separat festet i `packaging/requirements-build.txt`.

Resultatet ligger i `dist/FotoMaxxer/`. Hele mappa må følge executable-fila.
Linux-arkiver må bevare symlinker, eksempelvis med `tar`, og bygg må gjøres på en
kompatibilitetsbaseline som passer målmaskinene. Dagens Arch/Omarchy-bygg med
glibc 2.44 er **ikke** dokumentasjon på støtte for Ubuntu 24.04.

## Bygg på Windows

Bygg på Windows x64; Linux-bygget kan ikke produsere en Windows-executable.
Samme byggeskript og spec brukes på begge OS. Windows-bygget kjøres i GitHub Actions
(se [Windows-bygg i CI](#windows-bygg-i-ci)) og er prøvd på en `windows-2022`-runner.

```powershell
py -3.14 -m venv .venv
.\.venv\Scripts\python.exe -m pip install -e '.[desktop]'
.\.venv\Scripts\python.exe -m pip install -r packaging\requirements-build.txt
.\.venv\Scripts\python.exe scripts\build-desktop-probe.py `
  --models C:\testressurser\buffalo_l `
  --exiftool C:\testressurser\exiftool
```

ExifTool-mappa skal inneholde `exiftool.exe` (omdøpt fra `exiftool(-k).exe`) og
hele `exiftool_files/`. Byggeskriptet kopierer begge og avviser en løs exe-fil.
Det kan være nødvendig med C++-byggeverktøy for InsightFace på byggemaskina.
Windows-avhengigheter er foreløpig ikke låst; versjonene fra det vellykkede CI-bygget
ligger i `build.dependencies` i
[windows-2022-desktop.json](desktop-packaging-results/windows-2022-desktop.json)
og kan brukes som utgangspunkt for en låsefil.
Resultatet blir `dist\FotoMaxxer\FotoMaxxer.exe` og `FotoMaxxer-cli.exe` med støttefiler.

## Windows-bygg i CI

Workflow: `.github/workflows/build-desktop-windows.yml` (manuell start,
`workflow_dispatch`). Den eldre `build-windows-bundle.yml` bygger den gamle
embedded-Python-bundlen og er uavhengig av denne.

```bash
gh workflow run build-desktop-windows.yml -R Simen1331/fotomaxxer
gh run list  -R Simen1331/fotomaxxer --limit 3
gh run watch <id> -R Simen1331/fotomaxxer --exit-status
gh run download <id> -R Simen1331/fotomaxxer -n FotoMaxxer-desktop-windows-<id>   # ZIP, ca. 1 GB
gh run download <id> -R Simen1331/fotomaxxer -n FotoMaxxer-windows-verify-<id>    # logger, rapporter, skjermbilde
```

Stegene: Python 3.14 og Node 20 → `pip install -e ".[desktop]" scikit-image` og
`packaging/requirements-build.txt` → ExifTool 13.59 fra SourceForge (komplett mappe) →
`buffalo_l.zip` fra InsightFace v0.7-releasen → `scripts/build-desktop-probe.py` →
`scripts/verify-desktop-probe.py --desktop` → logger vises i jobbloggen →
ZIP av `dist\FotoMaxxer`. Artefaktene beholdes i 14 dager.

Repo: `Simen1331/fotomaxxer` (den gang privat; remoten het `github`). Merk: en nyopprettet
workflow-fil ble ikke registrert av GitHub før en ny commit endret den
(`workflow not found on the default branch` til da).

### Resultat, kjøring 37623754711 (7. oktober 2026)

Windows Server 2022 (10.0.20348), CPython 3.14.8, PySide6 6.11.2, ExifTool 13.59.
Alle kontroller bestod. Rådata:
[verification](desktop-packaging-results/windows-2022-verification.json),
[Qt-rapport](desktop-packaging-results/windows-2022-desktop.json),
[skjermbilde](desktop-packaging-results/windows-2022-desktop.png).

| Kontroll eller måling | Resultat |
|---|---|
| Pipeline i flyttet pakke (sti med æøå, tom PATH) | 14/14 |
| Executable, API, React, thumbnails, `serve`-avslutning | 10/10 |
| Qt-vindu, enkeltinstans, lukking under jobb | 12/12 |
| Pakkestørrelse | 1 364 528 178 byte, ca. 1,27 GiB |
| `--version` | 1,5 s |
| API-oppstart → helsesjekk | 1,1 s |
| Pipeline-prøve (CLI / fra Qt) | 11,7 s / 10,9 s |
| Qt `ui_ready` | 1,3 s |

Dette er en CI-runner uten GPU (Chromium kjører med `--disable-gpu`; GLES-feil i
`desktop.log` er forventet) og er ikke det samme som en ren Windows 11-maskin.

### Windows-funn som ble rettet

- **`cv2.imread` åpner ikke stier med æøå på Windows** → ansiktsdeteksjon feilet
  stille for bilder i mapper som «bibliotek med æøå». Rettet i `fotomaxxer/faces.py`
  (`cv2.imdecode(np.fromfile(...))`). Dette gjaldt også den vanlige installasjonen, ikke bare pakka.
- Verifiseringsskriptet manglet Windows-miljø (`SystemRoot`, `APPDATA`/`LOCALAPPDATA`/`TEMP`)
  og leste Windows-logger som UTF-8 (nå `PYTHONUTF8=1` og `errors="replace"`).
- `serve` avsluttes rent via Ctrl+Break («Application shutdown complete») men gir
  exit-kode 3 på Windows; testen godtar det.

Installasjonsveiledning for sluttbruker: [desktop-windows-install.md](desktop-windows-install.md).

## Prøv pakken

Velg et godkjent testportrett med minst ett tydelig ansikt. Til den lokale
utprøvingen brukte vi NASA-bildet av Eileen Collins som følger med scikit-image:
`skimage/data/astronaut.png`. [scikit-image dokumenterer bildet som public domain](https://scikit-image.org/docs/stable/api/skimage.data.html#skimage.data.astronaut).
Bildet er testinput; det tas ikke med som en ny appressurs i repoet.

```bash
.venv/bin/python scripts/verify-desktop-probe.py \
  --bundle dist/FotoMaxxer \
  --face-image /sti/til/godkjent-testportrett.png \
  --output '/tmp/fotomaxxer ny utprøving' \
  --isolate-linux --desktop
```

`--output` må være en ny mappe utenfor repoet. Testen kopierer pakken til en sti
med mellomrom og æøå, bruker en annen arbeidsmappe og fjerner utviklingsverktøy
fra PATH. Den bruker egne databaser og egen XDG-konfigplassering, aldri det
eksisterende biblioteket eller bootstrap-pekerfila.

`--isolate-linux` krever bubblewrap og tillatte brukernavnrom. Testen skjuler
repoet (inkludert venv), eksisterende InsightFace-modeller, systemets Perl-binær
og Perl-moduler. Nettverk er slått av under hele bilde-/ansiktsprøven. API-prøven
bruker loopback i vanlig nettverksnavnrom for at testdriveren skal nå tjeneren.
GUI-prøven med `--desktop` bruker et eget nettverksnavnrom med bare loopback,
slik at både React og pipeline prøves uten nett. Testen bruker en skrivbar
TMPDIR under testmappa for Qts lokale sockets. Den åpner et vindu kort og lukker
automatisk mens en ekte bakgrunnsjobb med prosessarbeidere pågår. Sett
`QT_QPA_PLATFORM=offscreen` for kjøring uten skjerm; Wayland og X11/XWayland er
også prøvd med faktisk rendering.
Systemets øvrige native biblioteker er fortsatt tilgjengelige; en ren VM-test er
fortsatt nødvendig. Uten bubblewrap kan flagget utelates.

På Windows brukes samme testdriver med `.venv\Scripts\python.exe`, uten
`--isolate-linux`, og med en ny mappe utenfor repoet. Driveren krever Python på
testmaskina, men appen gjør ikke det. Selve pipeline-prøven kan også kjøres direkte
på en maskin uten Python:

```bash
/sti/til/FotoMaxxer/FotoMaxxer-cli packaging-smoke \
  --output '/tmp/fotomaxxer ny pipeline' \
  --face-image /sti/til/godkjent-testportrett.png --workers 2
```

`packaging-smoke` lager ti filer: JPEG med EXIF-rotasjon og opptaksdato, PNG,
HEIC, HEIF, en eksakt duplikat, en korrupt JPEG og fire varianter av portrettet.
Det forventes ni unike bilder, åtte vellykkede derive-resultater, én isolert
derive-feil og minst ett ansikt i hvert portrett. Prøven kontrollerer:

- Ressurssjekksummer, EXIF, deduplisering, stat-cache og idempotens.
- Begge thumbnail-størrelser og orientering, med to prosessarbeidere.
- Faktisk InsightFace/ONNX/OpenCV-inferens og normaliserte 512-float embeddings.
- HDBSCAN-kjøring, uendrede originaler og SQLite-integritet.

HDBSCAN kan markere de fire portrettene som støy. Prøven bekrefter at klyngingen
kjører, men vurderer ikke personkuratering eller kvalitet på gruppering.
Testdriveren kontrollerer i tillegg helsesjekk, React-assets, SPA-ruting,
thumbnail-servering og normal avslutning av API-tjeneren. GUI-prøven sjekker
rendering, sesjonstilgang, navigasjonsgrense, off-record-profil, enkeltinstans og
lukking under ekte bilde- og ansiktsbehandling fra Qt-prosessen. Feil gir exitkode ulik
null, også når selve CLI-steget vanligvis håndterer per-bilde-feil mykt.

Rapporter og logger blir liggende i testmappa: `verification.json`,
`pipeline/report.json`, `pipeline.log` og `server.log`. Bygget inkluderer
`_internal/resource-manifest.json` med ressursenes SHA-256, modellfiler,
ExifTool/Perl-versjoner, Python-avhengigheter og frontend-lockens sjekksum.
GUI-prøven skriver også `desktop/desktop-report.json`, `desktop/window.png`
og `desktop.log`.

## Ressurser og arbeidsprosesser

`fotomaxxer/resources.py` skiller installasjonsressurser fra brukerdata.
En frosset app slår alltid opp ExifTool og modeller i sin egen pakke. Manglende
modellfiler gir en lokal feil før InsightFace kan starte nedlasting.
Utviklingsstarten bruker fortsatt system-ExifTool og vanlig InsightFace-cache.

Linux-pakken inkluderer Perl, core-/arkitekturmodulene og ExifTool-modulene.
Launcher-scriptet bruker bare shell-builtins og den medfølgende Perl-binæren;
Perls modulsøk begrenses til pakken. PyInstaller tar med native avhengigheter
til disse binærene. InsightFaces landmark-støttefiler kopieres også til
`_internal/objects/`, som biblioteket selv krever i frosset modus.

`scripts/frozen_entry.py` kaller `freeze_support()` før CLI og ML-bibliotekene
importeres, og velger spawn på begge OS. Modulstarten har nå også beskyttet
hovedinngangspunkt og korrekt exitkode. Dette følger
[PyInstallers multiprocessing-anvisning](https://pyinstaller.org/en/stable/common-issues-and-pitfalls.html#multi-processing).
Utviklingsmodus med `serve --reload` avvises i en frosset app.

Desktop-API-et binder én socket til `127.0.0.1:0` og beholder den gjennom hele
levetiden. Qt laster først siden etter en autentisert helsesjekk. WebEngine legger
til et tilfeldig sesjonstoken i header for alle lokale ressurser; tokenet legges
ikke i URL, cookies eller JavaScript. API-et kontrollerer token, Host, Origin og
Sec-Fetch-Site. Fremmede nettsider og fil-URL-er kan ikke navigeres til i vinduet,
eksterne nettressurser blokkeres og downloads avvises. Google Fonts-lenkene i
dagens frontend bruker derfor CSS-fallback-fontene i desktop-utgaven.
`fotomaxxer serve` beholder utviklingsstarten uten desktop-token.

Desktop-utprøvingen avviser datamappeflytting med en tydelig 409-feil. Dette
hindrer at dagens kopiering og statiske thumbnail-mount brukes under aktive
desktop-forbindelser. Flytteflyten må implementeres samlet med quiescence,
konsistent SQLite-kopi, nye thumbnail-referanser og ny instanslås.

## Status og videre prøving

Den opprinnelige motorprøven ligger i [baseline-resultatfila](desktop-packaging-results/linux-x86_64.json).
Qt-utgaven er registrert i [desktop-resultatfila](desktop-packaging-results/linux-desktop.json),
med et [skjermbilde fra testbiblioteket](desktop-packaging-results/linux-desktop.png).
Vi har verifisert reell bilde- og ansiktsbehandling i den isolerte, flyttede
pakken under både Wayland og X11/XWayland. De eksisterende lagringskontrollene
består 18/18, ende-til-ende-kontrollen 39/39, ressurskontrollene 5/5 og de nye
desktop-kontrakttestene 6/6. Flyttetesten forventer nå at gammel database
beholdes, i tråd med lagringskontrakten.

Målt 7. oktober 2026 på Linux 7.2.5 / Omarchy x86-64, glibc 2.44,
CPython 3.14.7 og PySide6 6.11.2:

| Kontroll eller måling | Resultat |
| --- | --- |
| Pipeline i flyttet pakke uten nett | 14/14 kontroller |
| Executable, API, React, bilder og avslutning | 10/10 kontroller |
| GUI, enkeltinstans og lukking under jobb | 12/12 under både Wayland og X11/XWayland |
| Pipeline fra Qt-prosessen | 14/14 kontroller |
| Pakke, uten dobbelttelling av symlinker | 1 630 688 967 byte, ca. 1,52 GiB |
| CLI-executable → `--version` ferdig | 0,357 sekunder |
| API-oppstart → helsesjekk | 0,746 sekunder |
| Komplett CLI-pipeline-prøve, to arbeidere | 6,1 sekunder |
| Komplett GUI-prøve, inkl. egen pipeline | 7,47 sekunder |
| Største barneprosess, peak RSS | 925 000 KiB, ca. 903 MiB |

Tidene er én kjøring på denne maskina med varm filcache. GUI-rapportens
`ui_ready_seconds` måler fra vindu/tjener er opprettet til React og bilder er
klare; den inkluderer ikke all Qt- og launcher-initialisering. RSS-målingen er
største enkeltprosess, ikke samlet minnebruk for
samtidige arbeidere. Ingen prosesser fra utprøvingen ble stående etter avslutning.

Før beslutningspunktet for begge OS kan godkjennes, gjenstår Windows-bygg og
ren Windows-VM, Linux-bygg mot Ubuntu-baseline og ren Linux-VM, samt prøving av
de faktiske brukerflytene og ansiktskvalitet. Jobbavbrudd, trygg datamappeflytting
og installer er senere leveranser. Kontrakttestene kjøres med
`.venv/bin/python tests/verify_desktop.py` etter at `httpx==0.28.1` er installert
som testavhengighet.
Pakken er en intern utprøving med lokale modeller; modellrettigheter og endelig
lisenssamling må avklares før ekstern distribusjon, som desktop-planen krever.

## Oppfølging

Gjort: Linux (Wayland, X11), Windows-bygg og -røykprøve i CI.

Åpent, omtrent i prioritert rekkefølge:

1. **Ren Windows 11-maskin/VM** uten utviklerverktøy: pakk ut ZIP-en fra CI, start
   `FotoMaxxer.exe`, kjør ekte bibliotek (HEIC, mange bilder, stier med æøå).
   Se etter SmartScreen/antivirus-oppførsel og at WebEngine fungerer med GPU.
2. **Ubuntu 24.04 og ren Linux-VM** (glibc-baseline; bygget er laget på glibc 2.44 og
   kan ikke antas å kjøre på eldre distroer — bygg på Ubuntu i CI).
3. **Lås avhengigheter for Windows** (`packaging/requirements-windows-py314.txt`,
   uttrukket fra rapporten) og bruk dem i workflowen. `insightface` rapporteres
   som 2.1 og `opencv-python` som 5.0.0.93 i Windows-bygget; sammenlign med Linux-låsen.
4. **Windows-sti-sjekk i resten av koden**: `cv2.imread`-feilen tyder på at andre
   biblioteker med C-stier (ExifTool-kall, andre cv2-/onnx-kall) også bør testes med æøå.
5. Datamappeflytting i GUI-et (sperret), jobbavbrudd, installer/signering, auto-oppdatering.
6. Modell- og ExifTool-lisenser før ekstern distribusjon. Modellene lastes nå fra
   InsightFace-releasen i CI uten sjekksum; legg til SHA-256-verifisering.
7. Oppgrader actions fra Node 20-versjoner (`checkout`, `setup-node`, `setup-python`,
   `upload-artifact`) når nye hovedversjoner finnes.
