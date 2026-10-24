//! Sammenlign Rust-EXIF med fasit fra Python/ExifTool (`tools/golden/exif_golden.py`).
//!
//! `exif_compare <exif.jsonl>` — skriver avvik per felt og eksempler.

use std::collections::BTreeMap;
use std::io::BufRead;
use std::path::Path;

use anyhow::Result;
use fotomaxxer_core::exif::{ExifData, extract};
use serde_json::{Value, json};

fn as_json(e: &ExifData) -> BTreeMap<&'static str, Value> {
    BTreeMap::from([
        ("taken_at", json!(e.taken_at)),
        ("year", json!(e.year)),
        ("season", json!(e.season)),
        ("width", json!(e.width)),
        ("height", json!(e.height)),
        ("camera_make", json!(e.camera_make)),
        ("camera_model", json!(e.camera_model)),
        ("gps_lat", json!(e.gps_lat)),
        ("gps_lon", json!(e.gps_lon)),
    ])
}

fn main() -> Result<()> {
    let golden = std::env::args()
        .nth(1)
        .expect("bruk: exif_compare <exif.jsonl>");
    let mut total = 0;
    let mut diffs: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for line in std::io::BufReader::new(std::fs::File::open(golden)?).lines() {
        let row: Value = serde_json::from_str(&line?)?;
        let path = row["path"].as_str().unwrap();
        let rust = as_json(&extract(Path::new(path)));
        total += 1;
        for (field, got) in &rust {
            let want = row["meta"].get(*field).cloned().unwrap_or(Value::Null);
            let same = match (&want, got) {
                (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
                (a, b) => a == b,
            };
            if !same {
                diffs
                    .entry(field)
                    .or_default()
                    .push(format!("{path}: python={want} rust={got}"));
            }
        }
    }
    println!("filer: {total}");
    for (field, list) in &diffs {
        println!("{field}: {} avvik", list.len());
        for d in list.iter().take(4) {
            println!("    {d}");
        }
    }
    println!(
        "{}",
        if diffs.is_empty() {
            "IDENTISK"
        } else {
            "AVVIK"
        }
    );
    Ok(())
}
