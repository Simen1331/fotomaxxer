# FotoMaxxer

Lokal-først foto-kuratering for trykte album. FotoMaxxer indekserer bildemappene dine, henter metadata og stedsnavn, finner og grupperer ansikter lokalt og kan AI-merke bildene (valgfritt, via OpenRouter). Deretter velger du ut bilder til album og eksporterer originalene. Originalene endres aldri.

Alt kjører på din egen maskin uten sky og uten GPU. Databasen (`fotomaxxer.db`) og arbeidskopiene (`derived/`) ligger i en flyttbar datamappe.

## Last ned (Windows)

Hent `FotoMaxxer-Setup-<versjon>.exe` fra [Releases](https://github.com/Simen1331/fotomaxxer/releases). Installasjonen er per bruker, uten administrator, og trenger verken Python, Node eller en egen runtime. Programmet er ikke kodesignert, så SmartScreen spør første gang (**Flere detaljer → Kjør likevel**).

Se [docs/desktop-windows-install.md](docs/desktop-windows-install.md) for installering, oppdatering, avinstallering og feilsøking.

## Status

Rust-utgaven er under testing (`v0.1.0-rc`). Planen og fremdriften står i [docs/rust-port-plan.md](docs/rust-port-plan.md).

Repoet inneholder fortsatt den opprinnelige Python-utgaven (`fotomaxxer/`, `api/`). Den brukes som fasit for golden-testene i `tools/golden/` og fjernes i fase 8.

## Bygge fra kilde

Det trengs Rust (stable), Node 20 og systembibliotekene libheif og libde265. På Linux trengs også webkit2gtk-4.1 og gtk3 for vinduet.

```sh
cd web && npm ci && npm run build && cd ..
cargo build --release
cargo test --workspace
```

Dette gir to programmer:

- `target/release/fotomaxxer` er kommandolinja, med `fotomaxxer desktop` for vinduet.
- `target/release/fotomaxxer-gui` er vinduet alene.

Ansiktsgjenkjenning trenger `det_10g.onnx` og `w600k_r50.onnx` fra InsightFace `buffalo_l` i `models/` ved siden av exe-en, eller i mappa `FOTOMAXXER_MODELS` peker på.

Windows-bygget med statisk CRT, ONNX Runtime fra kilde og NSIS-installasjonsprogrammet er beskrevet i `.github/workflows/windows-release.yml` og `packaging/windows/`.

```sh
fotomaxxer --help
fotomaxxer source add /sti/til/bilder
fotomaxxer scan --all && fotomaxxer derive
fotomaxxer faces detect && fotomaxxer faces cluster
```

## Lisens

- **Koden i FotoMaxxer** er under [MIT-lisens](LICENSE).
- **Ansiktsmodellene** fra InsightFace (`buffalo_l`), som følger med i Windows-installasjonsprogrammet, er etter InsightFace sine vilkår bare til ikke-kommersiell forskningsbruk.
- **libheif og libde265** (LGPL-3.0) er lenket statisk inn i Windows-utgaven. Byggeskriptene i dette repoet gjør det mulig å bygge den på nytt med andre versjoner.
- **Tredjepartskomponentene** er listet i [packaging/windows/THIRD-PARTY.txt](packaging/windows/THIRD-PARTY.txt).
