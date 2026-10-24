//! Fase 0-røyktest (kjøres i CI på Windows og Linux): dekod JPEG/PNG/HEIC og finn
//! ansikter med de innebygde modellene. Feiler hvis et bilde ikke gir minst ett ansikt.
//!
//! `smoke <modellmappe> <bilde>...`

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Result, bail};
use fotomaxxer_core::imageio::open_rgb;
use fotomaxxer_faces::FaceAnalyzer;

fn main() -> Result<()> {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let Some((models, images)) = args.split_first() else {
        bail!("bruk: smoke <modellmappe> <bilde>...");
    };
    let started = Instant::now();
    let mut analyzer = FaceAnalyzer::new(models)?;
    println!(
        "modeller lastet på {:.2} s",
        started.elapsed().as_secs_f64()
    );
    for path in images {
        let img = open_rgb(path)?;
        let t = Instant::now();
        let faces = analyzer.analyze(&img)?;
        println!(
            "{}: {}x{} → {} ansikt(er) på {} ms",
            path.display(),
            img.width(),
            img.height(),
            faces.len(),
            t.elapsed().as_millis()
        );
        if faces.is_empty() {
            bail!("fant ingen ansikter i {}", path.display());
        }
    }
    println!("OK");
    Ok(())
}
