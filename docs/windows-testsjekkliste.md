# Testsjekkliste: FotoMaxxer på ren Windows 11

Denne lista brukes til Simens test av `FotoMaxxer-Setup-<versjon>.exe` på en ren Windows 11-maskin før fila deles videre. Ren betyr: ingen Python, ingen tidligere FotoMaxxer-pakke og ingen Visual C++-runtime installert med vilje. Installasjon og bruk er beskrevet i [desktop-windows-install.md](desktop-windows-install.md).

Fyll inn ✅/❌ og notater. Ved ❌ tar du med `%LOCALAPPDATA%\FotoMaxxer\logs\desktop.log`, et skjermbilde og hva du gjorde.

**Versjon som testes:** `__________` · **Dato:** `__________` · **Windows-build** (`winver`): `__________`

## Forberedelser

Ha med en mappe med testbilder, for eksempel 50–200 bilder. Den bør inneholde:

- JPEG-er fra mobil eller kamera med GPS (gir stedsnavn).
- Minst ett HEIC-bilde fra iPhone.
- Et par bilder som ligger på siden (EXIF-orientering).
- Flere bilder av de samme 3–5 personene, så det blir personer.
- Et filnavn eller en mappe med æøå.

Legg mappa for eksempel i `C:\Users\<deg>\Pictures\Testbilder æøå`.

| # | Sjekk | Resultat |
| --- | --- | --- |
| F1 | `%LOCALAPPDATA%\FotoMaxxer` og `%USERPROFILE%\.config\fotomaxxer` finnes ikke fra før | |
| F2 | Sjekksummen til Setup.exe stemmer med `.sha256` fra releasen (`Get-FileHash`) | |

## 1. Installering

| # | Sjekk | Resultat |
| --- | --- | --- |
| 1.1 | Dobbeltklikk Setup.exe. SmartScreen vises, og **Flere detaljer → Kjør likevel** fungerer | |
| 1.2 | Det kommer ingen UAC- eller administrator-spørsmål | |
| 1.3 | Veiviseren fullfører uten feil. Standardmappa er `%LOCALAPPDATA%\Programs\FotoMaxxer` | |
| 1.4 | Mappa inneholder `FotoMaxxer.exe`, `FotoMaxxer-cli.exe`, `models\det_10g.onnx`, `models\w600k_r50.onnx`, `licenses\`, `README.txt` og `uninstall.exe`, og ingen DLL-er (ingen `DirectML.dll`, `onnxruntime.dll`, `VCRUNTIME140.dll` eller `MSVCP140.dll`) | |
| 1.5 | Start-menyen har snarveien **FotoMaxxer** med ikon | |
| 1.6 | **Innstillinger → Apper → Installerte apper** viser «FotoMaxxer» med versjon og størrelse | |

## 2. Første oppstart

| # | Sjekk | Resultat |
| --- | --- | --- |
| 2.1 | Start FotoMaxxer fra Start-menyen. Godta eventuelt SmartScreen én gang til. Vinduet «FotoMaxxer» åpnes innen ca. 10 s | |
| 2.2 | **Det kommer ikke noe konsollvindu**, heller ikke et som blinker kort | |
| 2.3 | Grensesnittet ser riktig ut: mørkt tema og riktige fonter (Fraunces i overskrifter, ikke Times eller Arial) og ingen tomme ruter | |
| 2.4 | `%LOCALAPPDATA%\FotoMaxxer` er opprettet med `fotomaxxer.db`, `derived\` og `logs\desktop.log`, og `%USERPROFILE%\.config\fotomaxxer\config.json` peker dit | |
| 2.5 | Ingen brannmurvarsel | |
| 2.6 | Høyreklikk, F5 og Alt+venstrepil fører ikke bort fra appen. Etter F5 virker appen som før, uten feil om manglende tilgang | |

## 3. Uten nett

Koble fra nettverket (flymodus) og start FotoMaxxer på nytt.

| # | Sjekk | Resultat |
| --- | --- | --- |
| 3.1 | Appen starter og ser lik ut som i 2.3, fontene inkludert | |
| 3.2 | Punkt 4–6 under virker uten nett. Det eneste som mangler, er stedsnavn og AI-tagger. Kjør gjerne punkt 4 og 5 uten nett, og slå nettet på igjen før punkt 4.4 | |

## 4. Kilde og «Oppdater alt»

| # | Sjekk | Resultat |
| --- | --- | --- |
| 4.1 | **Kilder → Legg til:** mappevelgeren starter i hjemmemappa. Naviger til testmappa med æøå, eller lim inn stien og bruk «Bruk sti». Den legges til | |
| 4.2 | **Oppdater alt** kjører skanning, derive, (AI-merking hoppes over uten nøkkel), ansikter og klynging. Fremdriften vises | |
| 4.3 | Biblioteket viser thumbnails for alle bildene, også HEIC, og liggende bilder står riktig vei | |
| 4.4 | Med nett: stedsnavn på GPS-bilder. Legg eventuelt inn en OpenRouter-nøkkel under **Innstillinger** og kjør **Oppdater alt** med et lite utvalg. Det gir AI-tagger og en kostnad i oversikten. Valgfritt, fordi det koster penger | |
| 4.5 | Filtrene (sted, person, tagg, år, fritekst) gir rimelige treff | |
| 4.6 | Bildedetalj (lightbox), ekskludering og gjeninnsetting av et bilde virker | |

## 5. Ansikter og personer

| # | Sjekk | Resultat |
| --- | --- | --- |
| 5.1 | **Personer** viser klynger med rimelige forsidebilder | |
| 5.2 | Gi en person navn med æøå. Navnet blir stående etter at appen er startet på nytt | |
| 5.3 | Slå sammen to personer, flytt et ansikt til en annen person og lås det, og ignorer et ansikt | |
| 5.4 | Kjør **Oppdater alt** igjen. Navn og låste ansikter består | |
| 5.5 | Minnebruken i Oppgavebehandling under ansiktssteget holder seg rundt 1–1,5 GB. Maskina blir ikke ubrukelig | |

## 6. Album og eksport

| # | Sjekk | Resultat |
| --- | --- | --- |
| 6.1 | Lag et album med æøå i navnet og legg til bilder fra biblioteket og fra en person | |
| 6.2 | Endre rekkefølgen med dra og slipp og fjern et bilde. Rekkefølgen blir stående etter omstart | |
| 6.3 | **Eksporter** til en ny mappe, for eksempel `C:\Users\<deg>\Desktop\Eksport æøå`. Filene er originalene (full størrelse) i albumrekkefølge | |
| 6.4 | Originalene i testmappa er urørt (endringsdato og størrelse) | |

## 7. Lukking under jobb

Bruk gjerne en større mappe, eller legg til en ny kilde, så jobben varer en stund.

| # | Sjekk | Resultat |
| --- | --- | --- |
| 7.1 | Start **Oppdater alt** og lukk vinduet (X) mens derive eller ansikter kjører. En dialog spør om du vil avbryte jobben og lukke | |
| 7.2 | Velg **Nei** (fortsett). Vinduet blir stående, og jobben fortsetter | |
| 7.3 | Lukk igjen og velg **Ja**. Vinduet lukkes innen noen sekunder, og ingen `FotoMaxxer.exe` står igjen i Oppgavebehandling | |
| 7.4 | Start på nytt og kjør **Oppdater alt**. Jobben fortsetter der den slapp, uten feil og uten å gjøre ferdig arbeid på nytt | |

## 8. Ny start og én instans

| # | Sjekk | Resultat |
| --- | --- | --- |
| 8.1 | Med FotoMaxxer åpen og minimert: start fra Start-menyen igjen. Det eksisterende vinduet kommer frem, og det åpnes ikke noe nytt vindu | |
| 8.2 | Oppgavebehandling viser bare én FotoMaxxer etter noen sekunder | |
| 8.3 | Lukk og start på nytt. Biblioteket, personene og albumene er der | |

## 9. Kommandolinja (`FotoMaxxer-cli.exe`)

Lukk vinduet først. Åpne PowerShell:

```powershell
$fmx = "$env:LOCALAPPDATA\Programs\FotoMaxxer\FotoMaxxer-cli.exe"
& $fmx --version
& $fmx source list
& $fmx settings list
& $fmx faces cluster
```

| # | Sjekk | Resultat |
| --- | --- | --- |
| 9.1 | `--version` skriver versjonen | |
| 9.2 | `source list` viser testmappa med æøå riktig skrevet. Det er samme datamappe som vinduet bruker | |
| 9.3 | `settings list` maskerer API-nøkkelen | |
| 9.4 | `faces cluster` fullfører uten feil | |

## 10. Avinstallering

| # | Sjekk | Resultat |
| --- | --- | --- |
| 10.1 | Med FotoMaxxer åpen: start avinstallering. Den nekter og ber deg lukke appen | |
| 10.2 | Lukk appen. **Innstillinger → Apper → FotoMaxxer → Avinstaller** fullfører | |
| 10.3 | `%LOCALAPPDATA%\Programs\FotoMaxxer` er borte, snarveien er borte fra Start-menyen, og oppføringen under Apper er borte | |
| 10.4 | **`%LOCALAPPDATA%\FotoMaxxer` ligger igjen urørt** med `fotomaxxer.db`, `derived\` og `logs\`, og det samme gjør `%USERPROFILE%\.config\fotomaxxer\config.json` | |
| 10.5 | Originalene og eksportmappa er urørt | |
| 10.6 | Installer på nytt. Biblioteket fra før (personer og album) kommer tilbake uten å skanne på nytt | |

## Valgfritt

| # | Sjekk | Resultat |
| --- | --- | --- |
| V1 | Datamappe fra Python-pakka: kopier en eksisterende `FotoMaxxer`-datamappe til `%LOCALAPPDATA%\FotoMaxxer` (og pekerfila), og start. Album, tagger, personnavn og låste ansikter er de samme | |
| V2 | Oppdatering: installer en nyere Setup.exe over den gamle. Det går uten avinstallering, og biblioteket består | |
| V3 | Treg eller gammel maskin (4–8 GB RAM, eldre CPU): «Oppdater alt» på 200 bilder fullfører | |

## Resultat

- [ ] Alt grønt: fila kan deles videre
- [ ] Avvik (beskriv under, med logg):
