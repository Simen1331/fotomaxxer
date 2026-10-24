//! Feilsøking: sammenlign Rust-mellomsteg (dekoding, 640-input, 112-crop) med Python-dump.
//! Bruk: dump_stages <derived-mappe> <stage-mappe med meta.json + py_*.rgb>

use std::path::PathBuf;

use anyhow::Result;
use fotomaxxer_core::imageio::open_rgb;
use fotomaxxer_faces::embed::align;
use fotomaxxer_faces::imgops::resize_linear;
use image::RgbImage;

fn diff(name: &str, a: &[u8], b: &[u8]) {
    let n = a.len().min(b.len());
    let d: Vec<i32> = (0..n).map(|i| (a[i] as i32 - b[i] as i32).abs()).collect();
    let max = d.iter().max().copied().unwrap_or(0);
    let mean = d.iter().map(|&x| x as f64).sum::<f64>() / n.max(1) as f64;
    let ne = d.iter().filter(|&&x| x > 0).count();
    println!(
        "{name}: len {}/{} maks={max} snitt={mean:.4} ulike={:.1}%",
        a.len(),
        b.len(),
        100.0 * ne as f64 / n as f64
    );
}

fn main() -> Result<()> {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let (derived, stages) = (&args[0], &args[1]);
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(stages.join("meta.json"))?)?;
    let hash = meta["hash"].as_str().unwrap();
    let img = open_rgb(&derived.join(format!("{hash}_1024.jpg")))?;
    diff(
        "dekodet",
        img.as_raw(),
        &std::fs::read(stages.join("py_decoded.rgb"))?,
    );

    // Isoler skaleringen: bruk Pythons dekodede bilde som input.
    let py_img = RgbImage::from_raw(
        img.width(),
        img.height(),
        std::fs::read(stages.join("py_decoded.rgb"))?,
    )
    .unwrap();
    let (w, h) = img.dimensions();
    let r = h as f32 / w as f32;
    let (nw, nh) = if r > 1.0 {
        ((640.0 / r) as u32, 640)
    } else {
        (640, (640.0 * r) as u32)
    };
    let mut det = RgbImage::new(640, 640);
    image::imageops::replace(&mut det, &resize_linear(&py_img, nw, nh), 0, 0);
    diff(
        "640-input (py-dekodet)",
        det.as_raw(),
        &std::fs::read(stages.join("py_det.rgb"))?,
    );

    for (i, k) in meta["kps"].as_array().unwrap().iter().enumerate() {
        let kps: [[f32; 2]; 5] = serde_json::from_value(k.clone())?;
        let py = std::fs::read(stages.join(format!("py_crop{i}.rgb")))?;
        diff(
            &format!("crop{i} (py-dekodet, py-kps)"),
            align(&py_img, &kps).as_raw(),
            &py,
        );
        diff(
            &format!("crop{i} (rs-dekodet, py-kps)"),
            align(&img, &kps).as_raw(),
            &py,
        );
    }
    Ok(())
}
