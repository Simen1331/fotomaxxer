//! HDBSCAN, sentroider og likheter mot sklearn-fasiten for de private ansikts-embeddingene.
//!
//! ```text
//! ~/dev/fotomaxxer/.venv/bin/python tools/golden/hdbscan_golden.py faces \
//!     ~/dev/fotomaxxer-testlib/golden/faces.jsonl ~/dev/fotomaxxer-testlib/golden/hdbscan.json
//! cargo run --release -p fotomaxxer-faces --example hdbscan_golden -- \
//!     ~/dev/fotomaxxer-testlib/golden/faces.jsonl ~/dev/fotomaxxer-testlib/golden/hdbscan.json
//! ```

use std::io::BufRead;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use base64::Engine;
use fotomaxxer_faces::{hdbscan, linalg};
use serde::Deserialize;

#[derive(Deserialize)]
struct GoldenImage {
    faces: Vec<GoldenFace>,
}

#[derive(Deserialize)]
struct GoldenFace {
    embedding: String,
}

#[derive(Deserialize)]
struct Fasit {
    count: usize,
    cases: Vec<Case>,
    centroid_labels: Vec<i64>,
    centroids: Vec<String>,
    sims: Vec<String>,
}

#[derive(Deserialize)]
struct Case {
    indices: Option<Vec<usize>>,
    mcs: usize,
    labels: Vec<i64>,
}

fn f32s(b64: &str) -> Result<Vec<f32>> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64)?;
    Ok(bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [faces_path, fasit_path] = args.as_slice() else {
        bail!("bruk: hdbscan_golden <faces.jsonl> <hdbscan.json>");
    };
    let mut embs: Vec<Vec<f32>> = Vec::new();
    for line in std::io::BufReader::new(std::fs::File::open(faces_path)?).lines() {
        let g: GoldenImage = serde_json::from_str(&line?)?;
        for f in g.faces {
            embs.push(f32s(&f.embedding)?);
        }
    }
    let fasit: Fasit = serde_json::from_slice(&std::fs::read(fasit_path).context("lese fasit")?)?;
    if fasit.count != embs.len() {
        bail!("fasit har {} embeddings, fila {}", fasit.count, embs.len());
    }

    let mut failures = 0;
    for case in &fasit.cases {
        let points: Vec<&[f32]> = match &case.indices {
            Some(idx) => idx.iter().map(|&i| embs[i].as_slice()).collect(),
            None => embs.iter().map(Vec::as_slice).collect(),
        };
        let started = Instant::now();
        let got = hdbscan::fit_predict(&points, case.mcs)?;
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let clusters = case
            .labels
            .iter()
            .filter(|&&l| l >= 0)
            .max()
            .map_or(0, |m| m + 1);
        let same = got == case.labels;
        if !same {
            failures += 1;
        }
        let diff = got.iter().zip(&case.labels).filter(|(a, b)| a != b).count();
        println!(
            "  [{}] n={:4} mcs={} klynger={clusters:3} avvik={diff} ({ms:.0} ms)",
            if same { "LIK " } else { "ULIK" },
            points.len(),
            case.mcs
        );
    }

    // Sentroider (float32-snitt + OpenBLAS-norm) og likheter (sdot) bit for bit.
    let cids: Vec<i64> = {
        let mut c: Vec<i64> = fasit
            .centroid_labels
            .iter()
            .copied()
            .filter(|&l| l >= 0)
            .collect();
        c.sort_unstable();
        c.dedup();
        c
    };
    let cents: Vec<Vec<f32>> = cids
        .iter()
        .map(|&c| {
            let members: Vec<&[f32]> = embs
                .iter()
                .zip(&fasit.centroid_labels)
                .filter(|(_, l)| **l == c)
                .map(|(e, _)| e.as_slice())
                .collect();
            linalg::centroid(&members)
        })
        .collect();
    let mut cent_ok = 0;
    for (got, want) in cents.iter().zip(&fasit.centroids) {
        cent_ok += usize::from(got == &f32s(want)?);
    }
    let mut sim_ok = 0;
    let mut sim_total = 0;
    for (e, want) in embs.iter().zip(&fasit.sims) {
        for (c, w) in cents.iter().zip(f32s(want)?) {
            sim_total += 1;
            sim_ok += usize::from(linalg::sdot(e, c) == w);
        }
    }
    println!(
        "sentroider bit-like: {cent_ok}/{}  likheter bit-like: {sim_ok}/{sim_total}",
        cents.len()
    );
    if cent_ok != cents.len() || sim_ok != sim_total {
        failures += 1;
    }
    println!(
        "{}",
        if failures == 0 {
            "IDENTISK".to_string()
        } else {
            format!("{failures} avvik")
        }
    );
    Ok(())
}
