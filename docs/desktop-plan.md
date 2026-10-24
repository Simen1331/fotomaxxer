# FotoMaxxer som selvstendig app for Linux og Windows

Målet er å distribuere FotoMaxxer med en executable som brukeren kan starte uten å installere Python, .NET, Node, ExifTool eller utviklerverktøy. Rust er et mulig virkemiddel, men er ikke et krav.

**Anbefaling: Behold Python-motoren og React-grensesnittet. Pakk motoren med PyInstaller og vis grensesnittet i et eget vindu med PySide6 og Qt WebEngine.** Verifiser først at hele bilde- og ansiktspipelinen fungerer i en pakket utgave på begge operativsystemer. En omskriving til Rust bør bare velges dersom denne utprøvingen viser konkrete problemer som pakking ikke løser.

Planen er basert på kildekoden og dokumentasjon undersøkt 7. oktober 2026. [Pakkeutprøvingen](desktop-packaging-experiment.md) har nå også Qt-vindu, launcher, enkeltinstans, avgrenset lokalt API og venting på aktive jobber ved lukking. Den er prøvd på Linux under Wayland og X11/XWayland. Windows-bygget er prøvd i CI (`windows-2022`, alle kontroller bestått; se [Windows-bygg i CI](desktop-packaging-experiment.md#windows-bygg-i-ci)). Rene VM-er (Windows 11, Ubuntu), jobbavbrudd og trygg datamappeflytting gjenstår; dette er ennå ikke en ferdig desktop-leveranse.

## Hva en selvstendig app betyr

Sluttbrukeren skal få `FotoMaxxer.exe` på Windows og en tilsvarende kjørbar app på Linux. Biblioteker og kjøremiljø kan følge med i appens egen mappe. Brukeren trenger ikke vite at de finnes, og de skal ikke installeres som et separat Python-miljø.

Det er tre forskjellige egenskaper:

- **Ingen separat runtime-installasjon:** Dette er kravet, og det kan oppfylles med eksisterende kode.
- **Kompilert programkode:** Nuitka eller en omskriving kan gi dette, men er ikke nødvendig for enkel distribusjon.
- **Nøyaktig én fysisk fil:** Dette er et eget pakkevalg. Modeller, database og brukerbilder gjør at appen uansett trenger datafiler under bruk.

PyInstaller inkluderer Python-tolk og avhengigheter. Nuitka kan produsere en Python-uavhengig distribusjon i standalone-modus, men den vanlige accelerated-modusen er ikke tilstrekkelig. Ingen av løsningene gjør alle native biblioteker og OS-avhengigheter borte. [PyInstaller](https://pyinstaller.org/en/stable/operating-mode.html), [Nuitka](https://nuitka.net/user-documentation/user-manual.html).

Arbeidsantakelsen er Windows 11 x64 og Linux x86-64, med Ubuntu 24.04 og brukerens Arch/Omarchy-miljø som første testmål. ARM og andre OS-versjoner utvides etter behov. Appen skal fungere lokalt uten nett for blaing, kuratering, eksport og ferdig installerte ansiktsmodeller. AI-merking og adresseoppslag bruker fortsatt nettjenester.

## Hva prosjektet allerede inneholder

Prosjektet er en lokal desktop-lignende applikasjon med webgrensesnitt. FastAPI kjører lokalt og kaller Python-funksjonene direkte. Det finnes ingen nødvendig ekstern applikasjonsserver.

| Del | Dagens løsning | Betydning for leveransen |
| --- | --- | --- |
| Grensesnitt | React, Vite, TanStack Query, Zustand og virtualiserte bildelister | Ferdigbygde HTML-, JS- og CSS-filer kan følge med appen; Node trengs bare ved bygging |
| API | FastAPI og uvicorn | Kan kjøre internt i appen med eksisterende API-kontrakt |
| Lagring | SQLite, skjema versjon 5, relative thumbnail-stier | Eksisterende databaser kan brukes uten formatbytte |
| Import | SHA-256, stat-cache, deduplisering og ExifTool | ExifTool må pakkes med eksplisitt |
| Bildebehandling | Pillow og pillow-heif, JPEG/PNG/HEIC/HEIF | Native kodeker må være med og prøves på rene maskiner |
| AI-merking | OpenRouter, profiler, kostnadsregistrering og retry | Kan beholdes; nøkkeloppsett er fortsatt nødvendig for denne funksjonen |
| Ansikter | InsightFace buffalo_l, ONNX Runtime CPU, NumPy og scikit-learn HDBSCAN | Største tekniske risiko ved både pakking og omskriving |
| Kuratering | Personnavn, sammenslåing, låste ansikter, album og sortering | Allerede implementert funksjonalitet som bør bevares |
| Eksport | Nummererte kopier av originalene | PDF-eksport er ikke implementert og inngår ikke i denne leveransen |
| Vedlikehold | Kildeopprydding, flytting av data og klargjøring for deling | Må testes også fra ferdigpakket app |

Statusavsnittene i README og deler av utviklingsdokumentasjonen er eldre enn implementasjonen. Både API, GUI og pipeline finnes allerede.

Windows-byggeskriptet pakker allerede en embedded Python-distribusjon, avhengigheter og frontend. Det betyr at prosjektet er et stykke på vei mot kravet. Det som mangler er blant annet en ordentlig executable som inngangspunkt, integrert førstegangsoppsett, styrt oppstart/avslutning, Linux-pakking og bekreftet funksjon på rene maskiner.

## Vurderte alternativer

| Alternativ | Hva gjenbrukes | Konsekvens | Vurdering |
| --- | --- | --- | --- |
| PyInstaller og Qt WebEngine | Hele motoren og React | Eget vindu, medfølgende Python og nettlesermotor; relativt stor pakke | **Anbefalt første løsning** |
| Nuitka standalone og Qt WebEngine | Hele motoren og React | Kompilering av Python-kode, mer byggearbeid; native biblioteker og modeller må fortsatt pakkes | Aktuelt hvis faktisk kompilering gir en målt fordel |
| Tauri og pakket Python som hjelpeprosess | Motoren og React | Rust-skall og to livssykluser; WebView2/WebKitGTK må håndteres | Mulig, men tilfører et ekstra teknologilag |
| Tauri og Rust-motor | React og datamodellen | Python-funksjonaliteten må porteres, særlig ML og bildebehandling | Aktuelt ved et eget mål om Rust eller vesentlig endret motor |
| Full native Rust-app med eksempelvis Iced | Primært datamodell og funksjonsspesifikasjon | Både motor, bildevisning og alle arbeidsflyter skrives om | Høyest arbeid og regresjonsrisiko |

Qt WebEngine kan vise eksisterende webinnhold i et applikasjonsvindu. Qt dokumenterer distribusjon med PyInstaller, samt et eget distribusjonsverktøy basert på Nuitka. Den konkrete kombinasjonen med FotoMaxxers ML-avhengigheter må likevel verifiseres. [QWebEngineView](https://doc.qt.io/qtforpython-6/PySide6/QtWebEngineWidgets/QWebEngineView.html), [Qt og PyInstaller](https://doc.qt.io/qtforpython-6/deployment/deployment-pyinstaller.html), [pyside6-deploy](https://doc.qt.io/qtforpython-6/deployment/deployment-pyside6-deploy.html).

Tauri bruker systemets webview: WebView2 på Windows og WebKitGTK på Linux. Windows-installasjonsprogrammet kan håndtere WebView2, men en løs executable kan ikke uten videre antas å fungere uten riktig runtime. Det gjør Tauri mindre direkte for akkurat kravet om en ferdig pakke uten manuelle forutsetninger. [Tauri-forutsetninger](https://v2.tauri.app/start/prerequisites/), [Windows-distribusjon](https://v2.tauri.app/distribute/windows-installer/).

Iced er et alternativ for et Rust-basert GUI. For dette prosjektet vurderer jeg gevinsten som liten sammenlignet med arbeidet med å gjenskape filtrering, virtualiserte bildegallerier, lightbox, ansiktsmarkeringer og albumsortering. [Iced](https://github.com/iced-rs/iced).

## Foreslått arkitektur

```text
FotoMaxxer executable
  ├─ Oppstart, logg, førstegangsoppsett og avslutning
  ├─ Qt-vindu med eksisterende React-grensesnitt
  └─ Lokal FastAPI-tjener på en tildelt loopback-port
       └─ Eksisterende Python-motor og bakgrunnsjobber
            ├─ SQLite og derived
            ├─ Medfølgende ExifTool og bildebiblioteker
            └─ Medfølgende ONNX Runtime og ansiktsmodeller
```

Qt kjører på hovedtråden. API-tjeneren får en kontrollert bakgrunnstråd uten utviklingsmodus eller reload. CPU-arbeid fortsetter i egne prosesser, etter at prosessoppstart i pakket app er verifisert. Vinduet åpner først innholdet når tjenerens helsesjekk svarer. Tjeneren binder til loopback og får en ledig port uten en sårbar «finn port, slipp port, bind på nytt»-sekvens.

En ny oppstart fokuserer det eksisterende vinduet for samme datamappe. Lukking under en jobb tilbyr å vente eller avbryte ved et trygt kontrollpunkt. Deretter stoppes tjener og arbeidere. Dagens daemon-tråd alene er ikke tilstrekkelig til dette.

API-et skal bare brukes av appen. Lokal tilgang avgrenses med sesjonstoken, kontroll av origin/host og blokkering av fremmed navigasjon i appvinduet. Dette er relevant fordi API-et blant annet kan flytte data og klargjøre biblioteket for deling.

## Avhengighetene som må løses eksplisitt

**Bildebehandling og ansikter.** Test pakket import og faktisk kjøring av pillow-heif, OpenCV, ONNX Runtime, NumPy, InsightFace og scikit-learn. Dynamiske importer, modellfiler, kodekplugins og DLL-/SO-filer kan kreve egne pakkehooks. Ikke godkjenn bygget bare fordi startvinduet åpner.

**Prosessoppstart.** Dagens derive og faces bruker ProcessPoolExecutor. Oppstart må ha et beskyttet hovedinngangspunkt og riktig støtte for pakkede arbeidsprosesser. PyInstaller krever sin freeze_support-håndtering på alle plattformer. Test at arbeidere ikke åpner nye vinduer, starter nye API-tjenere eller tolker interne argumenter som CLI-kommandoer. [PyInstaller om multiprocessing](https://pyinstaller.org/en/stable/common-issues-and-pitfalls.html).

**ExifTool.** Lever en fast versjon med hele dens nødvendige runtime og støttefiler. Dagens skript kopierer bare exe-filen og antar at det er tilstrekkelig. Linux-utgaven krever også en konkret strategi for Perl/runtime; et tilfeldig systeminstallert ExifTool skal ikke være en skjult forutsetning. Kall alltid den pakkede ressursen via eksplisitt sti.

**Modeller.** Velg en full pakke med modellene inkludert som første leveranse. Det gir ansiktsgjenkjenning uten førstegangs nedlasting. Bruk et modellmanifest med versjon, sjekksum og forventede filer. InsightFaces publiserte modellvilkår skiller seg fra kodelisensen, så rettighetene for den tiltenkte distribusjonen må avklares før modellene sendes med. [InsightFace-modeller](https://github.com/deepinsight/insightface/blob/master/model_zoo/README.md?plain=1).

**Ressursstier.** Skill installasjonsressurser fra skrivbare brukerdata. Dagens frontend-oppslag antar enten kildekodetre eller plasseringen av embedded python.exe. Det må erstattes av en resolver som også fungerer i ferdigpakket app.

**Datamappe.** Bevar eksisterende SQLite-format og relative avledede filer. Respekter eksplisitt datamappe, miljøvariabel og eksisterende pekerfil. Ved første GUI-start uten et eksisterende valg brukes en varig mappe under brukerprofilen, ikke gjeldende arbeidsmappe. En eventuell overgang til OS-standard konfigplassering skal lese den gamle pekerfilen først.

Ved flytting stoppes nye jobber og skriving før en konsistent databasekopi tas. Kopier til et tomt eller kontrollert mål, kontroller resultatet og bytt peker atomisk. Gammel mappe beholdes som standard. Aktive databaseforbindelser og thumbnail-servering må byttes samtidig; dagens statiske mount følger ikke automatisk en ny datamappe. Originalfiler som ligger på en annen maskin må kunne kobles til en ny kilderot, mens album og allerede lagrede miniatyrbilder fortsatt kan leses.

## Gjennomføring i små leveranser

Hver endring skal kunne bygges og prøves uten at den eksisterende utviklingsstarten slutter å fungere. Produksjonsbiblioteket brukes ikke som testdata.

| Trinn | Konkrete endringer | Ferdig når |
| --- | --- | --- |
| 1 | Etabler testbibliotek med JPEG, PNG, HEIC/HEIF, EXIF-rotasjon, duplikater, korrupte filer og samtykkede ansiktsbilder | Forventede resultater og nåværende avvik er dokumentert |
| 2 | Lås Python- og pakkeversjoner for begge plattformer; lag bygging av frontend med låst avhengighetsfil | Byggene kan gjenskapes på rene byggemaskiner |
| 3 | Lag et minimalt PyInstaller-bygg i mappemodus med pipeline og datafiler | Scan, derive og faces fungerer uten system-Python på begge OS |
| 4 | Legg til eksplisitt ressursresolver og komplette ExifTool-/modellressurser | Bygget virker fra vilkårlig arbeidsmappe, også med mellomrom og æøå |
| 5 | Innfør korrekt hovedinngangspunkt og pakket multiprocessing | Ingen rekursiv oppstart; flere bilder behandles uten heng |
| 6 | Samle førstegangsoppsett, datamappevalg og logging i launcher | Én oppstart oppretter nødvendige data; feil er synlige uten konsoll |
| 7 | Legg til Qt-vindu, kontrollert lokal tjener og venting på readiness | React-grensesnittet åpnes i eget vindu og alle ressurser lastes |
| 8 | Legg til enkeltinstans, trygg lukking og avgrensning av lokalt API | Omstart og lukking under jobb har definert, testet oppførsel |
| 9 | Rett flytteflyt og oppdatering av database-/thumbnail-referanser | Biblioteket virker umiddelbart etter flytting; originalene er urørt |
| 10 | Lag Windows-installer og portabel mappe med executable | Ren Windows-maskin trenger ingen manuell runtime-installasjon |
| 11 | Lag Linux-pakke, først testbar mappe og deretter AppImage hvis egnet | Valgte Linux-mål starter og behandler bilder uten ekstra språk-runtime |
| 12 | Kjør hele brukerflyten fra distribusjonspakkene; dokumenter installasjon og oppgradering | Alle akseptansekriterier nedenfor er oppfylt |

Trinn 3 er beslutningspunktet. Hvis en avhengighet ikke kan pakkes pålitelig, undersøkes en konkret alternativ pakkehook eller Nuitka standalone før en omskriving velges. Det bør ikke brukes tid på nytt GUI-design før motoren er bevist distribuerbar.

## Bygging og distribusjon

Bygg Windows på Windows og Linux på Linux. Python, Node og kompilatorer er kun byggeavhengigheter. PyInstaller-utdata er plattformspesifikke. Begynn med en mappe med executable og støttefiler; dette gjør manglende biblioteker enklere å finne. Onefile kan vurderes senere, men innebærer utpakking ved oppstart og er lite attraktivt med store modeller. [PyInstaller-distribusjon](https://pyinstaller.org/en/stable/operating-mode.html).

Qt WebEngine gir en større pakke, men lar oss levere nettlesermotoren sammen med appen. Ta med Qt-plugins, WebEngine-hjelpeprosessen, ressurser og nødvendige redistribuerbare biblioteker. Kontroller lisensfilene for den faktiske pakken. Appens installasjonsmappe skal kunne erstattes ved oppgradering uten å miste databasen.

Linux-pakken må bygges mot en definert kompatibilitetsbaseline og testes under både Wayland og X11 der disse støttes. AppImage er en mulig beholder, ikke en garanti for at enhver Linux-distribusjon har riktige systembiblioteker. Test også grafikkdrivere, font-/dialogintegrasjon og oppstart uten et utviklingsmiljø.

Release-jobben produserer versjonerte pakker, sjekksummer, dependency-/modellmanifest og testresultater. Mål faktisk installasjonsstørrelse, oppstartstid og minnebruk; unngå antakelsen om at kompilering automatisk gjør ML-pakken liten eller rask.

## Tester og akseptansekriterier

De eksisterende lagringskontrollene bestod **18 av 18** under analysen. Ende-til-ende-kontrollen fikk **32 av 39**: derive feilet i dette begrensede miljøet, og en separat kontroll viste at standard prosesspool-oppstart ble blokkert med PermissionError ved opprettelse av forkserver-socket. Flere thumbnail-kontroller feilet som følge av manglende derive-resultater. I tillegg forventer testen at gammel database slettes ved flytting uten slettingsflagget, mens implementasjonen og lagringstesten krever at den beholdes. Full grønn ende-til-ende-status er derfor ikke etablert.

Før distribusjon skal testforventningen samordnes med regelen om å beholde gammel mappe, og hele testen kjøres i et miljø som tillater arbeidsprosessene. Ansikter, GUI og pakkede native biblioteker trenger egen testdekning utover disse eksisterende skriptene.

Leveransen er ferdig når:

- Appen starter på rene Windows- og Linux-maskiner uten Python, .NET, Node eller ExifTool installert for appen.
- Oppstart krever ett brukerinngrep og viser et eget vindu uten et permanent konsollvindu.
- Et eksisterende bibliotek kan åpnes med samme album, tagger, personnavn og låste ansikter.
- JPEG, PNG og HEIC/HEIF får korrekte miniatyrbilder og orientering; én korrupt fil stopper ikke resten.
- Scan er idempotent, duplikater håndteres og originalfilenes innhold er uendret.
- Lokal ansiktsdeteksjon og klynging fungerer uten nett etter installasjon av full pakke.
- AI-kall testes med en falsk leverandør for feil, retry, kostnad og profilvalg; en liten ekte test kjøres bare med eksplisitt autorisert nøkkel og kostnad.
- Eksport bevarer albumrekkefølge, rapporterer utilgjengelige originaler og har en definert regel for navnekollisjoner.
- Oppgradering, omstart, datamappeflytting og klargjøring for deling fungerer fra den distribuerte appen.
- Lukking under jobb etterlater en konsistent database og ingen foreldreløse bakgrunnsprosesser.

## Dersom Rust senere blir ønskelig

Behold React med Tauri og flytt motoren trinnvis: SQLite og lagring først, så bibliotek/album/profiler, scan og eksport, bildebehandling, AI-merking og til slutt ansikter. Skill en GUI-uavhengig Rust-kjerne fra desktop-adapteren. Migrer klientens API-kall samlet til Tauri-kommandoer og gi bildene en egen ressurskanal; relative HTTP-bildestier kan ikke forventes å virke uendret. [Tauri-kommandoer](https://v2.tauri.app/develop/calling-rust/).

Rust kan bruke rusqlite mot eksisterende database, ort mot ONNX Runtime og libheif for HEIC. Dette er kandidater som må prøves sammen, ikke en ferdig bevist erstatning. [rusqlite](https://docs.rs/rusqlite/latest/rusqlite/), [ort](https://github.com/pykeio/ort), [libheif](https://github.com/strukturag/libheif).

Den dyreste delen blir å erstatte InsightFaces preprocessing, deteksjon, landmark-basert justering og embeddings, og å få klynging til å bevare manuelle valg. Eksisterende embeddings er 512-dimensjonale normaliserte float32-vektorer. Samme dimensjon betyr ikke at nye modeller produserer kompatible vektorer. Sammenlign resultatene mot et fast referansesett før gamle og nye embeddings blandes. HDBSCAN-varianter kan også gi ulike grupper og må evalueres på faktisk personkuratering.

Et helt native GUI kan eventuelt bygges etter at denne kjernen er stabil. Det oppfyller ikke distribusjonskravet bedre nok til å begrunne omskrivingen nå.

## Omfang og innsats

Første leveranse omfatter dagens funksjoner i en selvstendig desktop-pakke. PDF-eksport, ny AI-modell, GPU-støtte, ny database, skyfunksjoner og et redesignet grensesnitt holdes utenfor.

Som et grovt planleggingsanslag for én utvikler: sett av **2–5 arbeidsdager til pakkeutprøvingen**, deretter **2–4 uker til launcher, eget vindu, distribusjon og stabilisering** dersom utprøvingen lykkes. Ukjente native avhengigheter og Linux-kompatibilitet kan øke dette. En Rust-motor er et arbeid over flere uker til måneder, med ytterligere GUI-arbeid for full native omskriving. Estimatene er vurderinger, ikke målte leveransetider.

Første anbefalte handling er derfor et lite, komplett pakkeeksperiment på begge OS: start executable, åpne ett HEIC-bilde, lag thumbnails, finn og klyng ansikter, og avslutt rent. Det avgjør om den rimeligste veien faktisk oppfyller kravet.
