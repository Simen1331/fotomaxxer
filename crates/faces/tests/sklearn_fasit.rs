//! Klyngingen mot fasit fra Python (sklearn 1.9 / numpy 2.4 / OpenBLAS), laget med
//! `tools/golden/hdbscan_golden.py fixtures crates/faces/tests/fixtures`.
//! Kravet er bit-/etikett-likhet, ikke bare «omtrent likt».

use std::path::PathBuf;

use base64::Engine;
use fotomaxxer_faces::{hdbscan, linalg, npsort};
use serde::Deserialize;

fn fixture<T: for<'de> Deserialize<'de>>(name: &str) -> T {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap()
}

fn f32s(b64: &str) -> Vec<f32> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .unwrap();
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

#[derive(Deserialize)]
struct SortCase {
    ints: Vec<i64>,
    argsort: Vec<usize>,
}

#[test]
fn argsort_som_numpy() {
    let cases: Vec<SortCase> = fixture("npsort.json");
    for c in &cases {
        let a: Vec<f64> = c.ints.iter().map(|&v| v as f64 * 0.37).collect();
        assert_eq!(npsort::argsort(&a), c.argsort, "n={}", a.len());
    }
}

#[derive(Deserialize)]
struct HdbscanCase {
    dim: usize,
    grid: i64,
    points: Vec<i64>,
    labels: std::collections::BTreeMap<String, Vec<i64>>,
}

#[test]
fn hdbscan_som_sklearn() {
    let cases: Vec<HdbscanCase> = fixture("hdbscan.json");
    for c in &cases {
        let values: Vec<f32> = c.points.iter().map(|&v| v as f32 / c.grid as f32).collect();
        let points: Vec<&[f32]> = values.chunks(c.dim).collect();
        for (mcs, want) in &c.labels {
            let got = hdbscan::fit_predict(&points, mcs.parse().unwrap()).unwrap();
            assert_eq!(&got, want, "n={} dim={} mcs={mcs}", points.len(), c.dim);
        }
    }
}

#[derive(Deserialize)]
struct LinalgCase {
    vectors: String,
    groups: Vec<Vec<usize>>,
    centroids: Vec<String>,
    sims: Vec<Vec<f64>>,
}

#[test]
fn sentroider_og_likheter_som_numpy() {
    let c: LinalgCase = fixture("linalg.json");
    let values = f32s(&c.vectors);
    let vecs: Vec<&[f32]> = values.chunks(512).collect();
    let cents: Vec<Vec<f32>> = c
        .groups
        .iter()
        .map(|g| linalg::centroid(&g.iter().map(|&i| vecs[i]).collect::<Vec<_>>()))
        .collect();
    for (got, want) in cents.iter().zip(&c.centroids) {
        assert_eq!(got, &f32s(want));
    }
    for (v, sims) in vecs.iter().zip(&c.sims) {
        for (cent, &want) in cents.iter().zip(sims) {
            assert_eq!(linalg::sdot(v, cent), want as f32);
        }
    }
}
