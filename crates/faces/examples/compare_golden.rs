//! Fase 0-spike: sammenlign Rust-ansiktsanalysen med Python-fasiten.
//!
//! ```text
//! cargo run --release -p fotomaxxer-faces --example compare_golden -- \
//!     ~/dev/fotomaxxer-testlib/golden/faces.jsonl \
//!     ~/dev/fotomaxxer-testlib/py-golden/derived \
//!     ~/.insightface/models/buffalo_l
//! ```
//!
//! Derivatene dekodes med `fotomaxxer_core::imageio::open_rgb`, samme vei som
//! `faces detect`.
//!
//! Valgfritt 4. argument: sti der matchede Rust-embeddings skrives som JSONL
//! (`hash`, `index` = ansiktets plass i fasiten, `embedding`) for klyngesammenligning.
//!
//! Akseptansekrav (docs/rust-port-plan.md): samme antall ansikter, bbox-avvik ≤ 2 px
//! i derivatkoordinater, cosinus ≥ 0,99 mot Python-embedding for samme ansikt.

use std::io::BufRead;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use base64::Engine;
use fotomaxxer_core::imageio::open_rgb;
use fotomaxxer_faces::{Face, FaceAnalyzer};
use serde::Deserialize;

#[derive(Deserialize)]
struct GoldenImage {
    hash: String,
    faces: Vec<GoldenFace>,
}

#[derive(Deserialize)]
struct GoldenFace {
    bbox: [f32; 4],
    kps: [[f32; 2]; 5],
    score: f32,
    embedding: String,
}

struct Matched {
    hash: String,
    index: usize,
    embedding: Vec<f32>,
    cos: f32,
    bbox_diff: f32,
    kps_diff: f32,
    score: f32,
}

fn main() -> Result<()> {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let (golden, derived, models, dump) = match args.as_slice() {
        [g, d, m] => (g, d, m, None),
        [g, d, m, out] => (g, d, m, Some(out)),
        _ => bail!(
            "bruk: compare_golden <faces.jsonl> <derived-mappe> <modellmappe> [rust-embeddings.jsonl]"
        ),
    };

    let mut analyzer = FaceAnalyzer::new(models).context("laste modeller")?;
    let file = std::fs::File::open(golden).context("åpne fasit")?;

    let (mut images, mut py_total, mut rs_total) = (0usize, 0usize, 0usize);
    let mut matched: Vec<Matched> = Vec::new();
    let mut missing: Vec<(String, f32)> = Vec::new();
    let mut extra: Vec<(String, f32)> = Vec::new();
    let started = Instant::now();

    for line in std::io::BufReader::new(file).lines() {
        let g: GoldenImage = serde_json::from_str(&line?)?;
        let path = derived.join(format!("{}_1024.jpg", g.hash));
        let img = open_rgb(&path).with_context(|| format!("lese {}", path.display()))?;
        let faces = analyzer.analyze(&img)?;
        images += 1;
        py_total += g.faces.len();
        rs_total += faces.len();

        let mut used = vec![false; faces.len()];
        for (index, gf) in g.faces.iter().enumerate() {
            let best = faces
                .iter()
                .enumerate()
                .filter(|(i, _)| !used[*i])
                .map(|(i, f)| (i, iou(&gf.bbox, &f.detection.bbox)))
                .max_by(|a, b| a.1.total_cmp(&b.1));
            match best {
                Some((i, overlap)) if overlap >= 0.5 => {
                    used[i] = true;
                    matched.push(compare(&g.hash, index, gf, &faces[i])?);
                }
                _ => missing.push((g.hash.clone(), gf.score)),
            }
        }
        for (i, f) in faces.iter().enumerate() {
            if !used[i] {
                extra.push((g.hash.clone(), f.detection.score));
            }
        }
    }

    let elapsed = started.elapsed().as_secs_f64();
    if let Some(out) = dump {
        let mut text = String::new();
        for m in &matched {
            let bytes: Vec<u8> = m.embedding.iter().flat_map(|v| v.to_le_bytes()).collect();
            let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
            text += &serde_json::json!({"hash": m.hash, "index": m.index, "embedding": b64})
                .to_string();
            text.push('\n');
        }
        std::fs::write(out, text)?;
    }
    println!(
        "bilder={images} ansikter python={py_total} rust={rs_total} matchet={}",
        matched.len()
    );
    println!(
        "tid: {elapsed:.1} s ({:.0} ms/bilde)",
        elapsed * 1000.0 / images.max(1) as f64
    );
    println!(
        "mangler i rust (python-score): {:?}",
        missing.iter().map(|m| m.1).collect::<Vec<_>>()
    );
    println!(
        "ekstra i rust (rust-score):    {:?}",
        extra.iter().map(|m| m.1).collect::<Vec<_>>()
    );

    if matched.is_empty() {
        bail!("ingen matchede ansikter å sammenligne");
    }
    let mut cos: Vec<f32> = matched.iter().map(|m| m.cos).collect();
    cos.sort_by(f32::total_cmp);
    let pct = |p: f64| cos[((cos.len() - 1) as f64 * p).round() as usize];
    println!(
        "cosinus: min={:.4} p1={:.4} p5={:.4} median={:.4}  under 0,99: {}",
        cos[0],
        pct(0.01),
        pct(0.05),
        pct(0.5),
        cos.iter().filter(|&&c| c < 0.99).count()
    );
    let max_bbox = matched.iter().map(|m| m.bbox_diff).fold(0.0, f32::max);
    let max_kps = matched.iter().map(|m| m.kps_diff).fold(0.0, f32::max);
    let over_2px = matched.iter().filter(|m| m.bbox_diff > 2.0).count();
    println!(
        "bbox maks avvik={max_bbox:.2} px (over 2 px: {over_2px}), kps maks avvik={max_kps:.2} px"
    );

    matched.sort_by(|a, b| a.cos.total_cmp(&b.cos));
    println!("dårligste:");
    for m in matched.iter().take(8) {
        println!(
            "  {} cos={:.4} bbox={:.2}px kps={:.2}px score={:.3}",
            &m.hash[..12],
            m.cos,
            m.bbox_diff,
            m.kps_diff,
            m.score
        );
    }

    let pass = missing.is_empty() && extra.is_empty() && over_2px == 0 && cos[0] >= 0.99;
    println!("{}", if pass { "BESTÅTT" } else { "IKKE BESTÅTT" });
    Ok(())
}

fn compare(hash: &str, index: usize, g: &GoldenFace, f: &Face) -> Result<Matched> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(&g.embedding)?;
    let py: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    let cos = py.iter().zip(&f.embedding).map(|(a, b)| a * b).sum();
    let bbox_diff = g
        .bbox
        .iter()
        .zip(&f.detection.bbox)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);
    let kps_diff = g
        .kps
        .iter()
        .flatten()
        .zip(f.detection.kps.iter().flatten())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);
    Ok(Matched {
        hash: hash.to_string(),
        index,
        embedding: f.embedding.clone(),
        cos,
        bbox_diff,
        kps_diff,
        score: g.score,
    })
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let iw = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let ih = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let inter = iw * ih;
    let union = (a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - inter;
    if union > 0.0 { inter / union } else { 0.0 }
}
