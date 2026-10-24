//! FotoMaxxer CLI — samme kommandoer, flagg og meldinger som `fotomaxxer/cli.py`.
//!
//! Kommandoene portes fase for fase (docs/rust-port-plan.md); CLI og API kaller de
//! samme `fotomaxxer_core`-funksjonene.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use fotomaxxer_core::geocode::Geocoder;
use fotomaxxer_core::{Storage, db, derive, faces, scan, settings, sources, storage, tagging};
use rusqlite::Connection;

#[derive(Parser)]
#[command(
    name = "fotomaxxer",
    version,
    about = "Lokal-først, bærbar foto-kuratering for trykte album."
)]
struct Cli {
    /// Pakke-rota (mappa med fotomaxxer.db). Standard: $FOTOMAXXER_HOME, ellers gjeldende mappe.
    #[arg(long, global = true, value_name = "DIR")]
    home: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Opprett pakke + database-skjema (idempotent).
    Init,
    /// Scan kilde(r) → images/files.
    Scan {
        /// Scan kun denne kilden.
        #[arg(long, value_name = "PATH", conflicts_with = "all")]
        source: Option<PathBuf>,
        /// Scan alle kilder.
        #[arg(long)]
        all: bool,
    },
    /// Lag nedskalerte arbeidskopier (1024 + 300).
    Derive {
        /// Antall parallelle arbeidere (standard: CPU-kjerner).
        #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
        workers: Option<u32>,
    },
    /// AI-merk pending bilder via OpenRouter vision.
    Tag {
        /// Maks antall bilder denne kjøringen (0/utelatt = alle pending).
        #[arg(long, value_name = "N", allow_negative_numbers = true)]
        limit: Option<i64>,
        /// Antall parallelle tråder (standard: 6).
        #[arg(long, value_name = "N", allow_negative_numbers = true)]
        workers: Option<i64>,
        /// Nullstill ai-state og merk på nytt (overskriver eksisterende ai_tags).
        #[arg(long)]
        retag: bool,
        /// Hopp over kostnads-bekreftelsen for store batcher.
        #[arg(long)]
        yes: bool,
    },
    /// Les/skriv bruker-konfig (nøkkel, modell, terskler).
    Settings {
        #[command(subcommand)]
        command: SettingsCommand,
    },
    /// Administrer kilde-mapper.
    Source {
        #[command(subcommand)]
        command: SourceCommand,
    },
    /// Tøm pakka for deling (fjern alle kilder + settings).
    PrepareHandoff,
    /// Ansikts-deteksjon og klynging til personer.
    Faces {
        #[command(subcommand)]
        command: FacesCommand,
    },
    /// Start det lokale web-GUI-et (axum-tjener med innebygd frontend).
    Serve {
        /// Bind-adresse (standard: 127.0.0.1).
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Port (standard: 8000).
        #[arg(long, default_value_t = 8000)]
        port: u16,
        /// Auto-reload ved kodeendring (utvikling).
        #[arg(long)]
        reload: bool,
    },
    /// Start FotoMaxxer i eget desktop-vindu (som fotomaxxer-gui).
    Desktop {
        /// Røyktest: ny rapportmappe (desktop-report.json).
        #[arg(long, value_name = "NY_RAPPORTMAPPE")]
        smoke: Option<PathBuf>,
        /// Røyktest: bilde med minst ett ansikt.
        #[arg(long, value_name = "BILDE")]
        smoke_image: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum FacesCommand {
    /// Detektér ansikter + embeddings på pending bilder.
    Detect {
        /// Antall parallelle analysatorer (standard: CPU-kjerner / 4, maks 2).
        #[arg(long, value_name = "N")]
        workers: Option<usize>,
        /// Ta også med bilder som tidligere feilet.
        #[arg(long)]
        retry_errors: bool,
    },
    /// Klyng ansikter til personer (inkrementell | full).
    Cluster {
        /// Full re-klynging av alt + re-mapping av navn (sjelden).
        #[arg(long)]
        full: bool,
        /// Cosinus-terskel for tilordning (standard: settings face_match_threshold).
        #[arg(long, value_name = "T")]
        threshold: Option<f64>,
    },
}

#[derive(Subcommand)]
enum SettingsCommand {
    /// Skriv en setting.
    Set {
        /// f.eks. openrouter_api_key, ai_model, face_match_threshold.
        key: String,
        /// Verdien.
        value: String,
    },
    /// Vis settings (api-nøkkel maskeres).
    List,
}

#[derive(Subcommand)]
enum SourceCommand {
    /// Legg til en kilde-mappe.
    Add {
        /// Absolutt sti til mappa.
        path: PathBuf,
    },
    /// List innlagte kilder.
    List,
    /// Fjern en kilde + dens app-data, aldri kilde-filer.
    Remove {
        /// Sti eller id til kilden.
        target: String,
    },
}

/// Kostnadsgrensen for `tag`: vis advarselen og spør på stdin. Slutt på input
/// (EOF) regnes som nei.
fn confirm_on_stdin(warning: &str) -> bool {
    println!("{warning}");
    print!("Fortsett? [j/N] ");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    match std::io::stdin().lock().read_line(&mut answer) {
        Ok(n) if n > 0 => tagging::is_yes(&answer),
        _ => false,
    }
}

/// Resolv pakke-rota og åpne DB-en med skjemaet på plass.
fn open(home: Option<&Path>) -> fotomaxxer_core::Result<(Storage, Connection)> {
    let store = Storage::new(&storage::resolve_root(home));
    let conn = db::connect(&store.db_path())?;
    db::init_db(&conn)?;
    Ok((store, conn))
}

fn run(cli: Cli) -> fotomaxxer_core::Result<ExitCode> {
    let home = cli.home.as_deref();
    match cli.command {
        Command::Init => {
            let root = storage::resolve_root(home);
            let store = Storage::new(&root);
            store.ensure_dirs()?;
            let conn = db::connect(&store.db_path())?;
            db::init_db(&conn)?;
            let tables = db::table_names(&conn)?;
            println!("Initialiserte FotoMaxxer-pakke i {}", root.display());
            println!("  database: {}", store.db_path().display());
            println!("  derived/: {}", store.derived_dir().display());
            println!("  tabeller ({}): {}", tables.len(), tables.join(", "));
        }
        Command::Scan { source, all } => {
            let (_store, conn) = open(home)?;
            let targets = if let Some(path) = source {
                match sources::get_by_path(&conn, &path)? {
                    Some(row) => vec![row],
                    None => {
                        eprintln!(
                            "Ukjent kilde: {} — legg den til med 'source add' først.",
                            path.display()
                        );
                        return Ok(ExitCode::from(1));
                    }
                }
            } else if all {
                sources::list_sources(&conn)?
            } else {
                eprintln!("Angi --source PATH eller --all.");
                return Ok(ExitCode::from(2));
            };
            if targets.is_empty() {
                println!("Ingen kilder å scanne.");
            }
            let mut geocoder = Geocoder::default();
            for row in targets {
                println!("Scanner #{}: {} …", row.id, row.root_path);
                let stats = scan::scan_source(&conn, &row, &mut geocoder, None)?;
                println!("  {}", stats.summary());
            }
        }
        Command::Derive { workers } => {
            let (store, conn) = open(home)?;
            let stats = derive::derive(&conn, &store, workers.map(|n| n as usize), None, None)?;
            println!("{}", stats.summary());
        }
        Command::Tag {
            limit,
            workers,
            retag,
            yes,
        } => {
            let (store, conn) = open(home)?;
            let opts = tagging::TagOptions {
                limit,
                workers,
                retag,
                assume_yes: yes,
                source_id: None,
            };
            match tagging::tag(&conn, &store, &opts, &mut confirm_on_stdin) {
                Ok(stats) => println!("{}", stats.summary()),
                Err(e @ fotomaxxer_core::Error::Tagging(_)) => {
                    eprintln!("Feil: {e}");
                    return Ok(ExitCode::from(1));
                }
                Err(e) => return Err(e),
            }
        }
        Command::Settings {
            command: SettingsCommand::Set { key, value },
        } => {
            let (_store, conn) = open(home)?;
            settings::set_setting(&conn, &key, &value)?;
            println!("Satt {key} = {}", settings::mask_value(&key, Some(&value)));
        }
        Command::Settings {
            command: SettingsCommand::List,
        } => {
            let (_store, conn) = open(home)?;
            let items = settings::all_settings(&conn)?;
            if items.is_empty() {
                println!("Ingen settings satt.");
            }
            for (key, value) in &items {
                println!("  {key} = {}", settings::mask_value(key, Some(value)));
            }
        }
        Command::Source {
            command: SourceCommand::Add { path },
        } => {
            let (_store, conn) = open(home)?;
            match sources::add(&conn, &path, "local", None) {
                Ok(row) => println!("La til kilde #{}: {}", row.id, row.root_path),
                Err(e @ fotomaxxer_core::Error::Source(_)) => {
                    eprintln!("Feil: {e}");
                    return Ok(ExitCode::from(1));
                }
                Err(e) => return Err(e),
            }
        }
        Command::Source {
            command: SourceCommand::List,
        } => {
            let (_store, conn) = open(home)?;
            let rows = sources::list_sources(&conn)?;
            if rows.is_empty() {
                println!("Ingen kilder lagt til ennå.");
            }
            for r in rows {
                let scanned = r.last_scanned_at.as_deref().unwrap_or("aldri");
                println!(
                    "#{}  {}  (filer={}, scannet={scanned})",
                    r.id, r.root_path, r.file_count
                );
            }
        }
        Command::Source {
            command: SourceCommand::Remove { target },
        } => {
            let (store, conn) = open(home)?;
            match sources::remove(&conn, &store, &target) {
                Ok(stats) => {
                    println!("Fjernet kilde. {}", stats.summary());
                    println!("Kilde-filer på disk er urørt.");
                }
                Err(e @ fotomaxxer_core::Error::Source(_)) => {
                    eprintln!("Feil: {e}");
                    return Ok(ExitCode::from(1));
                }
                Err(e) => return Err(e),
            }
        }
        Command::Faces {
            command:
                FacesCommand::Detect {
                    workers,
                    retry_errors,
                },
        } => {
            let (store, conn) = open(home)?;
            let stats = faces::detect(&conn, &store, workers, retry_errors, None)?;
            println!("{}", stats.summary());
        }
        Command::Faces {
            command: FacesCommand::Cluster { full, threshold },
        } => {
            let (_store, conn) = open(home)?;
            let stats = faces::cluster(&conn, full, threshold)?;
            println!("{}", stats.summary());
        }
        Command::Serve { host, port, reload } => return serve(home, &host, port, reload),
        Command::Desktop { smoke, smoke_image } => {
            return Ok(fotomaxxer_app::desktop::run(
                fotomaxxer_app::desktop::Args {
                    home: home.map(Path::to_path_buf),
                    smoke,
                    smoke_image,
                    relaunch_prefix: vec!["desktop".into()],
                },
            ));
        }
        Command::PrepareHandoff => {
            let (store, conn) = open(home)?;
            let stats = sources::prepare_for_handoff(&conn, &store)?;
            println!("Pakka er klar for deling. {}", stats.summary());
            println!("Settings tømt (inkl. evt. API-nøkkel). Kilde-filer på disk er urørt.");
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// `fotomaxxer serve`: samme flagg, standardverdier og første utskriftslinje som Python-
/// versjonen. Tjeneren bindes kun til loopback med mindre `--host` sier noe annet.
/// Ctrl-C (og SIGTERM) avslutter pent: nye jobber stenges ute, og en pågående jobb får
/// gjøre seg ferdig før prosessen avslutter med kode 0.
fn serve(
    home: Option<&Path>,
    host: &str,
    port: u16,
    reload: bool,
) -> fotomaxxer_core::Result<ExitCode> {
    if reload {
        // Som den frosne Python-appen: binæren har ingen kode å laste på nytt.
        eprintln!("Feil: --reload er bare tilgjengelig i utviklingsstarten.");
        return Ok(ExitCode::from(2));
    }
    if let Some(home) = home {
        // La --home virke for serve, som i Python: FOTOMAXXER_HOME peker på pakka.
        // SAFETY: ingen andre tråder finnes ennå (tokio-runtimen startes under).
        unsafe { std::env::set_var(storage::ENV_HOME, fotomaxxer_core::paths::resolve(home)) };
    }
    println!("FotoMaxxer GUI på http://{host}:{port}  (Ctrl-C for å stoppe)");
    let _ = std::io::stdout().flush();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let state = fotomaxxer_api::AppState::new(None)?;
        let listener = match tokio::net::TcpListener::bind((host, port)).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("Feil: kunne ikke lytte på {host}:{port}: {e}");
                return Ok(ExitCode::from(1));
            }
        };
        fotomaxxer_api::serve(listener, state, fotomaxxer_api::shutdown_signal()).await?;
        Ok(ExitCode::SUCCESS)
    })
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Feil: {e}");
            ExitCode::from(1)
        }
    }
}
