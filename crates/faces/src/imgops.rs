//! Bildeoperasjoner som etterligner OpenCV-oppførselen insightface bygger på.
//!
//! Embeddingene må ligge tett på dem Python-versjonen har lagret, så skalering og
//! affin warp følger `cv2.resize(INTER_LINEAR)` og `cv2.warpAffine` — samme
//! pikselsenter-konvensjon, kantregler og 1/32-piksel-kvantisering av koordinater.

use image::RgbImage;

/// Fastpunktskala for vektene i `cv2.resize` (`INTER_RESIZE_COEF_SCALE`).
const COEF_SCALE: f32 = 2048.0;

/// `cv2.resize(src, (w, h))` med standard `INTER_LINEAR` (ingen antialias).
///
/// Bit-eksakt etterligning av OpenCVs `uint8`-sti: vektene avrundes til 11-bits
/// fastpunkt, horisontal pass summeres i heltall, og den vertikale passen bruker
/// samme skift-og-avrund-formel som `VResizeLinear<uchar, …>`.
pub fn resize_linear(src: &RgbImage, dst_w: u32, dst_h: u32) -> RgbImage {
    let (sw, sh) = src.dimensions();
    let xs = axis_taps(sw, dst_w);
    let ys = axis_taps(sh, dst_h);
    let raw = src.as_raw();
    let stride = sw as usize * 3;
    let row_len = dst_w as usize * 3;

    // Horisontal pass per kilderad: heltall skalert med 2048.
    let hpass = |y: usize| -> Vec<i32> {
        let row = &raw[y * stride..(y + 1) * stride];
        let mut out = vec![0i32; row_len];
        for (dx, &(x0, x1, a0, a1)) in xs.iter().enumerate() {
            for c in 0..3 {
                out[dx * 3 + c] = row[x0 * 3 + c] as i32 * a0 + row[x1 * 3 + c] as i32 * a1;
            }
        }
        out
    };

    let mut out = RgbImage::new(dst_w, dst_h);
    let buf: &mut [u8] = &mut out;
    let mut cache: Option<(usize, Vec<i32>, usize, Vec<i32>)> = None;
    for (dy, &(y0, y1, b0, b1)) in ys.iter().enumerate() {
        let (s0, s1) = match &cache {
            Some((c0, r0, c1, r1)) if *c0 == y0 && *c1 == y1 => (r0.clone(), r1.clone()),
            _ => (hpass(y0), hpass(y1)),
        };
        let dst = &mut buf[dy * row_len..(dy + 1) * row_len];
        for x in 0..row_len {
            let v = (((b0 * (s0[x] >> 4)) >> 16) + ((b1 * (s1[x] >> 4)) >> 16) + 2) >> 2;
            dst[x] = v.clamp(0, 255) as u8;
        }
        cache = Some((y0, s0, y1, s1));
    }
    out
}

/// Kildeindekser og fastpunktvekter per målpiksel langs én akse, som OpenCV regner
/// dem: halv-piksel-senter i `f32`, kantene klemt, vekter avrundet til `short`.
fn axis_taps(src_len: u32, dst_len: u32) -> Vec<(usize, usize, i32, i32)> {
    let scale = src_len as f64 / dst_len as f64;
    let last = src_len as i64 - 1;
    (0..dst_len)
        .map(|d| {
            let s = ((d as f64 + 0.5) * scale - 0.5) as f32;
            let mut i = s.floor() as i64;
            let mut f = s - i as f32;
            if i < 0 {
                i = 0;
                f = 0.0;
            }
            if i >= last {
                i = last;
                f = 0.0;
            }
            let i1 = (i + 1).min(last);
            let w0 = ((1.0 - f) * COEF_SCALE).round_ties_even() as i32;
            let w1 = (f * COEF_SCALE).round_ties_even() as i32;
            (i as usize, i1 as usize, w0, w1)
        })
        .collect()
}

/// `cv2.warpAffine(src, m, (w, h), borderValue=0)`: `m` avbilder kilde → mål.
pub fn warp_affine(src: &RgbImage, m: &[[f64; 3]; 2], dst_w: u32, dst_h: u32) -> RgbImage {
    // OpenCV inverterer m og sampler kilden for hver målpiksel.
    let [[a, b, c], [d, e, f]] = *m;
    let det = a * e - b * d;
    let det = if det != 0.0 { 1.0 / det } else { 0.0 };
    let (ia, ib, id, ie) = (e * det, -b * det, -d * det, a * det);
    let ic = -ia * c - ib * f;
    let iff = -id * c - ie * f;

    // Fastpunkt som i OpenCV: koordinater i 1/1024 (AB_BITS = 10) med avrundingsdelta,
    // skiftet ned til 1/32 piksel (INTER_BITS = 5). Bilineære vekter (32-a)(32-b)·32
    // summerer eksakt til 2^15 (INTER_REMAP_COEF_BITS), så ingen vektjustering trengs.
    const AB_SCALE: f64 = 1024.0;
    const ROUND_DELTA: i64 = 16;
    let (sw, sh) = (src.width() as i64, src.height() as i64);
    let raw = src.as_raw();
    let mut out = RgbImage::new(dst_w, dst_h);
    let buf: &mut [u8] = &mut out;
    let pixel = |x: i64, y: i64, ch: usize| -> i32 {
        if x < 0 || y < 0 || x >= sw || y >= sh {
            0
        } else {
            raw[((y * sw + x) * 3) as usize + ch] as i32
        }
    };
    let adelta: Vec<i64> = (0..dst_w)
        .map(|x| (ia * x as f64 * AB_SCALE).round_ties_even() as i64)
        .collect();
    let bdelta: Vec<i64> = (0..dst_w)
        .map(|x| (id * x as f64 * AB_SCALE).round_ties_even() as i64)
        .collect();
    for y in 0..dst_h as i64 {
        let x0_row = ((ib * y as f64 + ic) * AB_SCALE).round_ties_even() as i64 + ROUND_DELTA;
        let y0_row = ((ie * y as f64 + iff) * AB_SCALE).round_ties_even() as i64 + ROUND_DELTA;
        for x in 0..dst_w as i64 {
            let sx = (x0_row + adelta[x as usize]) >> 5;
            let sy = (y0_row + bdelta[x as usize]) >> 5;
            let (x0, y0) = (sx >> 5, sy >> 5);
            let (a, b) = ((sx & 31) as i32, (sy & 31) as i32);
            let w = [
                (32 - a) * (32 - b) * 32,
                a * (32 - b) * 32,
                (32 - a) * b * 32,
                a * b * 32,
            ];
            for ch in 0..3 {
                let sum = pixel(x0, y0, ch) * w[0]
                    + pixel(x0 + 1, y0, ch) * w[1]
                    + pixel(x0, y0 + 1, ch) * w[2]
                    + pixel(x0 + 1, y0 + 1, ch) * w[3];
                buf[((y * dst_w as i64 + x) * 3) as usize + ch] =
                    ((sum + (1 << 14)) >> 15).clamp(0, 255) as u8;
            }
        }
    }
    out
}

/// RGB-bilde → NCHW-tensor `(x - mean) / std`, batch 1.
pub fn to_nchw(img: &RgbImage, mean: f32, std: f32) -> Vec<f32> {
    let (w, h) = img.dimensions();
    let plane = (w * h) as usize;
    let mut data = vec![0.0f32; plane * 3];
    for (i, px) in img.pixels().enumerate() {
        for c in 0..3 {
            data[c * plane + i] = (px[c] as f32 - mean) / std;
        }
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_identitet_gir_samme_bilde() {
        let img = RgbImage::from_fn(7, 5, |x, y| image::Rgb([x as u8 * 30, y as u8 * 40, 7]));
        assert_eq!(resize_linear(&img, 7, 5), img);
    }

    #[test]
    fn warp_identitet_gir_samme_bilde() {
        let img = RgbImage::from_fn(9, 9, |x, y| image::Rgb([x as u8 * 20, y as u8 * 20, 99]));
        let m = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        assert_eq!(warp_affine(&img, &m, 9, 9), img);
    }

    #[test]
    fn warp_utenfor_kilden_er_svart() {
        let img = RgbImage::from_pixel(4, 4, image::Rgb([200, 200, 200]));
        let m = [[1.0, 0.0, 10.0], [0.0, 1.0, 10.0]];
        let out = warp_affine(&img, &m, 4, 4);
        assert!(out.pixels().all(|p| p.0 == [0, 0, 0]));
    }
}
