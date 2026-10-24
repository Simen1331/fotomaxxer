//! `fotomaxxer-gui` (`FotoMaxxer.exe` i Windows-pakka): desktop-modus uten konsollvindu.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

#[derive(Parser)]
#[command(
    name = "FotoMaxxer",
    version,
    about = "Start FotoMaxxer i eget desktop-vindu."
)]
struct GuiArgs {
    /// Datamappe; eksisterende miljøvariabel og pekerfil respekteres.
    #[arg(long, value_name = "DIR")]
    home: Option<PathBuf>,
    /// Røyktest: ny rapportmappe (desktop-report.json).
    #[arg(long, value_name = "NY_RAPPORTMAPPE")]
    smoke: Option<PathBuf>,
    /// Røyktest: bilde med minst ett ansikt.
    #[arg(long, value_name = "BILDE")]
    smoke_image: Option<PathBuf>,
}

fn main() -> ExitCode {
    let args = GuiArgs::parse();
    fotomaxxer_app::desktop::run(fotomaxxer_app::desktop::Args {
        home: args.home,
        smoke: args.smoke,
        smoke_image: args.smoke_image,
        relaunch_prefix: Vec::new(),
    })
}
