# Installere FotoMaxxer på Windows

Dette gjelder Rust-utgaven av FotoMaxxer, som installeres med `FotoMaxxer-Setup-<versjon>.exe`. Den trenger ikke Python, Node, ExifTool eller administratorrettigheter, og den har ingen egen runtime (VC++ eller .NET). Grensesnittet vises i et eget vindu med WebView2, som følger med Windows 11.

Den gamle Python- og Qt-pakka (ZIP med `_internal\`) er beskrevet i git-historikken på `master`. Den fases ut i fase 8 av [rust-port-plan.md](rust-port-plan.md).

## Krav

- Windows 11 x64. Windows 10 x64 med WebView2 bør også fungere, men er ikke prøvd.
- Ca. 300 MB ledig plass til programmet. I tillegg trengs plass til biblioteket: databasen og `derived\`, som er ca. 150 KB per bilde.
- Ca. 1 GB ledig minne under ansiktsgjenkjenning. Den bruker 2 analysatorer à ca. 430 MB som standard.
- Internett trengs bare til AI-merking (OpenRouter) og stedsnavn (Kartverket). Bilder, kuratering, album, eksport og ansiktsgjenkjenning virker uten nett. Fontene og ansiktsmodellene følger med.

## 1. Hent installasjonsprogrammet

Installasjonsprogrammet bygges av workflowen **Windows-installasjonsprogram** (`.github/workflows/windows-release.yml`) i repoet [`Simen1331/fotomaxxer`](https://github.com/Simen1331/fotomaxxer). Siste versjon ligger under [Releases](https://github.com/Simen1331/fotomaxxer/releases), og fila kan lastes ned rett fra nettleseren uten innlogging:

- **Release (anbefalt):** Når en tagg `v*` pushes, lages en GitHub Release med `FotoMaxxer-Setup-<versjon>.exe` og en `.sha256`-fil. Tagger med bindestrek, som `v0.1.0-rc.1`, blir prerelease.

  ```powershell
  gh release list -R Simen1331/fotomaxxer
  gh release download v0.1.0-rc.2 -R Simen1331/fotomaxxer -p "FotoMaxxer-Setup-*"
  ```

- **Artefakt:** Hver push til `main` laster opp Setup.exe som artefakten `FotoMaxxer-Setup-<versjon>` under Actions. Det gjelder bare så lenge kontoens artefaktkvote ikke er brukt opp.

Kontroller gjerne summen før du gir fila videre:

```powershell
(Get-FileHash .\FotoMaxxer-Setup-0.1.0-rc.2.exe -Algorithm SHA256).Hash
Get-Content .\FotoMaxxer-Setup-0.1.0-rc.2.exe.sha256
```

## 2. Installer

1. Dobbeltklikk `FotoMaxxer-Setup-<versjon>.exe`.
2. **SmartScreen:** Programmet er ikke kodesignert, så Windows viser «Windows beskyttet PC-en din». Klikk **Flere detaljer** og så **Kjør likevel**. Det samme kan dukke opp første gang du starter selve FotoMaxxer.
3. Følg veiviseren. Det trengs ingen administrator, og det kommer ingen UAC-spørsmål.

Programmet installeres i **`%LOCALAPPDATA%\Programs\FotoMaxxer`**, som er `C:\Users\<deg>\AppData\Local\Programs\FotoMaxxer`:

```text
FotoMaxxer.exe        vinduet (uten konsoll)
FotoMaxxer-cli.exe    kommandolinja
models\               det_10g.onnx + w600k_r50.onnx (ansiktsmodellene)
licenses\             lisenser for fonter og tredjepartskomponenter
README.txt
uninstall.exe
```

Du får også snarveien **FotoMaxxer** i Start-menyen, og programmet dukker opp under **Innstillinger → Apper → Installerte apper** som «FotoMaxxer».

## 3. Første oppstart

Start **FotoMaxxer** fra Start-menyen.

- Et vindu med FotoMaxxer åpnes etter noen sekunder, og det kommer ikke noe svart konsollvindu.
- **Datamappa** (biblioteket) opprettes automatisk i **`%LOCALAPPDATA%\FotoMaxxer`**. Den inneholder `fotomaxxer.db`, `derived\`, `logs\` og `WebView2\`. Valget lagres i pekerfila `%USERPROFILE%\.config\fotomaxxer\config.json`. Ta backup av datamappa.
- **Har du brukt Python-pakka før,** finner FotoMaxxer den eksisterende datamappa via pekerfila og bruker den uendret, med samme album, tagger, personer og låste ansikter. Databaser fra eldre versjoner oppgraderes automatisk ved første oppstart. Ta likevel en kopi av datamappa før du prøver første gang, og ikke kjør den gamle og den nye pakka samtidig.
- Starter du FotoMaxxer mens den allerede kjører, hentes det åpne vinduet frem. Det kjører bare én instans per datamappe.

Vil du bruke en annen datamappe, for eksempel på en ekstern disk, starter du én gang fra PowerShell. Valget lagres bare hvis det ikke finnes noe valg fra før. Bruk ellers `--home` hver gang eller miljøvariabelen `FOTOMAXXER_HOME`:

```powershell
& "$env:LOCALAPPDATA\Programs\FotoMaxxer\FotoMaxxer.exe" --home "D:\Foto\FotoMaxxer-bibliotek"
```

Datamappa kan ikke flyttes fra selve vinduet i denne versjonen.

## 4. Ta i bruk

1. **Kilder → Legg til:** velg mappa med originalbildene. Originalene endres aldri, og bare stien lagres.
2. **Oppdater alt** skanner, lager arbeidskopier, AI-merker (hvis det finnes en nøkkel), finner ansikter og klynger dem til personer. Fremdriften vises underveis.
3. **Innstillinger:** lim inn OpenRouter-nøkkelen for AI-merking. Nøkkelen lagres i databasen, ikke i en fil. AI-merking koster penger. Uten nøkkel hoppes merkingen over.
4. **Personer:** gi navn, slå sammen, flytt og lås ansikter. **Album:** lag album, endre rekkefølgen og eksporter til en mappe.

Stedsnavn hentes fra Kartverket under skanningen. Skanner du uten nett, mangler stedsnavnene for de bildene. Alt annet virker.

### Lukking mens en jobb kjører

Lukker du vinduet mens en jobb kjører, spør FotoMaxxer om du vil avbryte jobben og lukke («Ja») eller fortsette å bruke appen («Nei»). Velger du å avbryte, stopper jobben ved neste trygge punkt (etter bildet den holder på med), og alt som er ferdig, er lagret. Neste «Oppdater alt» fortsetter der den slapp. En eksport eller klynging som er i gang, gjøres ferdig før vinduet lukkes.

## Kommandolinje (valgfritt)

`FotoMaxxer-cli.exe` har de samme kommandoene som `fotomaxxer` på Linux, og bruker samme datamappe som vinduet (`--home` > `FOTOMAXXER_HOME` > pekerfil):

```powershell
$fmx = "$env:LOCALAPPDATA\Programs\FotoMaxxer\FotoMaxxer-cli.exe"
& $fmx --version
& $fmx source list
& $fmx source add "D:\Bilder\2020"
& $fmx scan --all
& $fmx derive
& $fmx faces detect          # standard maks 2 analysatorer; --workers N for flere
& $fmx faces cluster
```

Kjør ikke CLI-en mot datamappa mens vinduet holder på med en jobb.

## Oppdatere

1. Lukk FotoMaxxer. Installasjonsprogrammet nekter å fortsette så lenge FotoMaxxer kjører, og ber deg prøve igjen.
2. Kjør den nye `FotoMaxxer-Setup-<versjon>.exe`. Den installerer over den gamle versjonen i samme mappe.
3. Datamappa og pekerfila er urørt. Databasen oppgraderes ved første oppstart hvis den nye versjonen krever det.

## Avinstallere

**Innstillinger → Apper → Installerte apper → FotoMaxxer → Avinstaller**. Du kan også kjøre `uninstall.exe` i programmappa.

Avinstalleringen fjerner bare programfilene, snarveien i Start-menyen og oppføringen under Apper. **Datamappa `%LOCALAPPDATA%\FotoMaxxer` (bibliotek, thumbnails, logger), pekerfila og bildene dine slettes aldri.** Vil du fjerne alt, sletter du disse selv etter avinstalleringen:

- `%LOCALAPPDATA%\FotoMaxxer` (og en eventuell datamappe du har valgt med `--home`)
- `%USERPROFILE%\.config\fotomaxxer\config.json`

### Stille modus (for skript og CI)

```powershell
.\FotoMaxxer-Setup-0.1.0-rc.2.exe /S                 # installer stille
.\FotoMaxxer-Setup-0.1.0-rc.2.exe /S /D=C:\FotoMaxxer  # annen mappe (/D sist, uten anførselstegn)
& "$env:LOCALAPPDATA\Programs\FotoMaxxer\uninstall.exe" /S   # avinstaller stille
```

Kjører FotoMaxxer, avsluttes stille modus med exit-kode 2. Avinstalleringsprogrammet kopierer seg selv til `%TEMP%` og returnerer før filene er slettet. Vent noen sekunder før du sjekker resultatet.

## Feilsøking

| Symptom | Tiltak |
| --- | --- |
| SmartScreen blokkerer | **Flere detaljer → Kjør likevel**. Programmet er usignert. |
| Antivirus stopper eller fjerner filer | Legg `%LOCALAPPDATA%\Programs\FotoMaxxer` til som unntak og installer på nytt. |
| Vinduet åpnes ikke, eller lukkes med en gang | Se `%LOCALAPPDATA%\FotoMaxxer\logs\desktop.log`. Oppstartsfeil vises også i en dialog med stien til loggen. |
| FotoMaxxer åpnes i nettleseren i stedet for i eget vindu | WebView2 mangler eller kunne ikke startes. Installer «Microsoft Edge WebView2 Runtime» (Evergreen) fra Microsoft og start på nytt. Appen virker i nettleseren så lenge dialogen «FotoMaxxer kjører i nettleseren» er åpen. Trykk OK der for å avslutte. |
| Hvitt eller tomt vindu | Lukk FotoMaxxer, slett mappa `%LOCALAPPDATA%\FotoMaxxer\WebView2` (bare nettlesercache, ikke biblioteket) og start på nytt. Se ellers `desktop.log`. |
| Brannmurvarsel | Skal ikke komme, fordi tjeneren bare lytter på `127.0.0.1`. Velg i så fall *Avbryt*. Nettverkstilgang trengs ikke. |
| Ny start gjør ingenting synlig | FotoMaxxer kjører allerede og prøver å hente frem vinduet. Se på oppgavelinja. Henger den, avslutter du `FotoMaxxer.exe` i Oppgavebehandling. |
| «Appen avslutter. Vent til pågående arbeid er ferdig.» | Vinduet er på vei til å lukkes og venter på at jobben stopper ved et trygt punkt. Vent. |
| Ansikter blir ikke funnet | Sjekk at `models\det_10g.onnx` og `models\w600k_r50.onnx` finnes i programmappa. Installer på nytt hvis ikke. Feilmeldingen i appen sier hvilken mappe som ble sjekket. |
| Installering eller avinstallering nekter | FotoMaxxer kjører. Lukk vinduet (og eventuelle `FotoMaxxer-cli.exe`) og prøv igjen. |

Ved feil sender du `desktop.log` (og de roterte `desktop.log.1`–`.3` hvis de finnes), Windows-versjonen og hva du gjorde. Loggen inneholder stier, men ingen bilder eller nøkler.
