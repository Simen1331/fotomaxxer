# FotoMaxxer på Windows

> Ny pakket desktop-utgave (ingen Python nødvendig): se [desktop-windows-install.md](desktop-windows-install.md).

Hvordan kjøre FotoMaxxer på en Windows-maskin (f.eks. hos en sluttbruker). To veier:

- **Vei A — manuell installasjon.** Virker i dag, ingen ny byggejobb. Noen som er litt teknisk
  setter det opp én gang; etterpå starter brukeren det med en snarvei. **Anbefalt nå.**
- **Vei B — pakket `.exe`.** Ekte dobbeltklikk uten Python/PATH-fikling, men krever en egen
  pakkejobb. Oversikt nederst — velg den hvis FotoMaxxer skal være selvbetjent eller deles videre.

Stien til datamappa er OS-nøytral: pekerfila havner i `C:\Users\<bruker>\.config\fotomaxxer\config.json`
når `XDG_CONFIG_HOME` ikke er satt. Den flyttbare pakke-modellen (`fotomaxxer.db` + `derived/`
relativt til pakke-rota) virker likt på Windows.

---

## Vei A — manuell installasjon (anbefalt)

### Det du trenger først
- En ferdig prosjektmappe med **bygd frontend** (`web/dist/` må ligge der — se «Bygg frontend» nederst).
  Brukeren skal ikke trenge Node; du bygger `dist` på din maskin og legger den med.
- Internett på Windows-maskinen første gang (insightface laster ned ansiktsmodellen, og AI-merking
  treffer OpenRouter).

### 1. Installer Python
Last ned **Python 3.11 eller 3.12** for Windows fra python.org. Huk av **«Add python.exe to PATH»**
i installasjonsveiviseren.

> Hvorfor 3.11/3.12 og ikke nyeste: `insightface` (ansikter) trenger en ferdigbygd wheel som matcher
> Python-versjonen. Nyeste Python mangler ofte wheel og tvinger fram kompilering (se feilsøking).

### 2. Installer ExifTool
`pyexiftool` er bare en wrapper — den krever selve **ExifTool-binæren**:

1. Last ned Windows-zip fra https://exiftool.org (filen heter f.eks. `exiftool-12.xx_64.zip`).
2. Pakk ut. Inni ligger `exiftool(-k).exe` — **døp den om til `exiftool.exe`**.
3. Legg den et sted som er på PATH. Enklest og mest selvinneholdt: legg `exiftool.exe` i prosjektets
   `.venv\Scripts\` etter at venv-et er laget i steg 3 (den mappa er på PATH når venv-et er aktivt).
   Alternativ: lag `C:\tools\`, legg `exiftool.exe` der, og legg `C:\tools` til i PATH.

Sjekk at den funker: `exiftool -ver` skal skrive ut et versjonsnummer.

### 3. Sett opp prosjektet
Kopier prosjektmappa til maskinen (med `web/dist/`). Åpne **PowerShell** i mappa og kjør:

```powershell
py -3.12 -m venv .venv
.\.venv\Scripts\Activate.ps1
pip install -e .
```

> Hvis PowerShell nekter å aktivere venv-et («running scripts is disabled»), kjør én gang:
> `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned`, og prøv `Activate.ps1` på nytt.

`pip install -e .` er steget der det kan butte (insightface) — se feilsøking hvis det feiler.

### 4. Opprett databasen
```powershell
fotomaxxer init
```
Oppretter `fotomaxxer.db` + `derived/` i mappa. Idempotent — trygt å kjøre om igjen. (Kjører du mot
en eldre database, migreres skjemaet automatisk opp til siste versjon.)

### 5. Start GUI-et
```powershell
fotomaxxer serve
```
Åpne **http://127.0.0.1:8000** i nettleseren. (`serve` tar `--host` og `--port` om du vil endre.)

### 6. Første gangs bruk i nettleseren
1. **Innstillinger → OpenRouter API-nøkkel:** lim inn nøkkelen (lagres lokalt i pakka, ikke i en fil).
2. **Innstillinger → Tagging-profiler:** lag evt. en profil (f.eks. preset «Design & håndverk») om du
   vil ha kategoriserte stikkord.
3. **Kilder & pipeline:** legg til mappene med bilder, og trykk **«Oppdater alt»** (skann →
   arbeidskopier → AI-merking → ansikter).

### 7. Enkel start for brukeren — en `.bat`-snarvei
Lag en fil `start-fotomaxxer.bat` i prosjektmappa med dette innholdet, så slipper brukeren PowerShell —
han dobbeltklikker bare den:

```bat
@echo off
cd /d "%~dp0"
call .venv\Scripts\activate.bat
start "" http://127.0.0.1:8000
fotomaxxer serve
```

Den hopper til prosjektmappa, aktiverer venv-et, åpner nettleseren, og starter serveren. Vinduet som
åpnes må stå åpent mens FotoMaxxer er i bruk — lukkes det, stopper serveren. Lag gjerne en snarvei til
`.bat`-fila på skrivebordet.

---

## Feilsøking (Windows)

- **«exiftool not found» / EXIF mangler ved scan:** `exiftool.exe` er ikke på PATH. Se steg 2; test
  med `exiftool -ver`.
- **`pip install -e .` feiler på insightface:** den prøver å kompilere fordi det ikke finnes en wheel
  for Python-versjonen. To utveier: (a) bruk Python 3.11/3.12 der wheel finnes, eller (b) installer
  **Visual Studio Build Tools** (med «Desktop development with C++») og kjør `pip install -e .` på nytt.
- **Ansikts-deteksjon henger/feiler første gang:** insightface laster ned `buffalo_l`-modellen
  (flere hundre MB) ved første `faces detect` — krever internett. Den havner i
  `C:\Users\<bruker>\.insightface\models\`. Kan forhåndskopieres dit hvis maskinen er offline.
- **Blank side på http://127.0.0.1:8000:** `web/dist/` mangler i pakka. Bygg frontend (under) og legg
  den med før du kopierer mappa.
- **«Port 8000 already in use»:** noe annet bruker porten. Start med `fotomaxxer serve --port 8080` og
  bruk http://127.0.0.1:8080.

---

## Vei B — pakket `.exe` (oversikt)

Den brukervennlige enden: én mappe/`.exe` som bunter **Python + alle deps + `web/dist` +
`exiftool.exe` + insightface-modellen**, og åpner nettleseren automatisk. Ingen Python-installasjon,
ingen PATH, ingen ExifTool-nedlasting — bare dobbeltklikk.

Hva det krever:
- **PyInstaller** (eller tilsvarende), bygd **på** Windows.
- Egne hooks for `insightface`/`onnxruntime` — de native binærene og `buffalo_l`-modellen pakkes ikke
  automatisk og må legges inn manuelt.
- `exiftool.exe` buntes som data-fil, og koden pekes på den (eller PATH settes ved oppstart).
- `web/dist` buntes som data-fil.
- Pakka blir stor (flere hundre MB) pga. onnxruntime + ansiktsmodellen.

Dette er en egen liten leveranse og fortjener et eget Blueprint-TB (Windows-pakking/distribusjon) før
bygging. Velg Vei B hvis FotoMaxxer skal kjøre selvbetjent hos brukeren eller deles videre til flere.

---

## Vedlegg — bygg frontend (gjøres på din maskin)

`web/dist/` produseres med Node/npm og legges med i pakka du sender:

```bash
cd web
npm install
npm run build      # skriver web/dist/
```

FastAPI serverer `web/dist/` på `/` automatisk når mappa finnes. Bygg på nytt og legg ved oppdatert
`dist` hver gang frontend endres.

---

## Klargjøre pakka for deling

Skal du gi pakka videre ren (uten dine kilder, thumbnails eller API-nøkkel): kjør
`fotomaxxer prepare-handoff` (eller «Klargjør for deling» i Innstillinger) før kopiering. Da tømmes
kilder + innstillinger, og databasen kompakteres så nøkkelen ikke kan graves ut av filbytes.
Originalbildene på disk røres aldri.
