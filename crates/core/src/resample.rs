//! Pillow-ekvivalent `Image.thumbnail((t, t), LANCZOS)` for 8-bits RGB.
//!
//! Derive-stegets thumbnails skal ligne Python-versjonens så mye som mulig, så dette er
//! en direkte port av Pillow 12 (`Image.thumbnail`/`Image.resize` i Python og
//! `libImaging/Reduce.c` + `libImaging/Resample.c` i C), ikke en «tilsvarende» skalering:
//!
//! 1. Målstørrelse fra `preserve_aspect_ratio` (avrunding mot best bevart aspekt), aldri
//!    oppskalering.
//! 2. `reducing_gap=2.0`: først heltalls boksreduksjon (`Image.reduce`) med faktor
//!    `int(kilde / mål / 2)` per akse, med Pillows fastpunkt-deling og egne kantblokker.
//! 3. Lanczos (støtte 3) på resten, med fraksjonell boks fra reduksjonen, to pass
//!    (horisontalt, så vertikalt) og 22-bits fastpunktkoeffisienter.
//!
//! Gitt samme inndata-piksler gir dette bit-identisk resultat med Pillow.

use image::RgbImage;

/// Pillows `reducing_gap` i `thumbnail` (standardverdien).
const REDUCING_GAP: f64 = 2.0;
/// Lanczos-filterets støtte (`LANCZOS = {lanczos_filter, 3.0}`).
const LANCZOS_SUPPORT: f64 = 3.0;
/// `PRECISION_BITS = 32 - 8 - 2` i Resample.c.
const PRECISION_BITS: u32 = 22;

/// Pillows `preserve_aspect_ratio` for et kvadratisk mål `t × t`.
///
/// `None` når bildet allerede er innenfor (`thumbnail` gjør da ingenting).
pub fn thumbnail_size(width: u32, height: u32, target: u32) -> Option<(u32, u32)> {
    let (x, y) = (target, target);
    if x >= width && y >= height {
        return None;
    }
    let aspect = f64::from(width) / f64::from(height);
    let (xf, yf) = (f64::from(x), f64::from(y));
    if xf / yf >= aspect {
        let nx = round_aspect(yf * aspect, |n| (aspect - n / yf).abs());
        Some((nx, y))
    } else {
        let ny = round_aspect(xf / aspect, |n| {
            if n == 0.0 {
                0.0
            } else {
                (aspect - xf / n).abs()
            }
        });
        Some((x, ny))
    }
}

/// `max(min(floor(n), ceil(n), key=key), 1)`: ved likhet vinner `floor` (første minimum).
fn round_aspect(number: f64, key: impl Fn(f64) -> f64) -> u32 {
    let (lo, hi) = (number.floor(), number.ceil());
    let pick = if key(hi) < key(lo) { hi } else { lo };
    pick.max(1.0) as u32
}

/// `copy.thumbnail((target, target), Image.LANCZOS)`. `None` = uendret (allerede innenfor).
pub fn thumbnail(img: &RgbImage, target: u32) -> Option<RgbImage> {
    let size = thumbnail_size(img.width(), img.height(), target)?;
    if img.dimensions() == size {
        return None;
    }
    Some(resize_with_reducing_gap(img, size))
}

/// `Image.resize(size, LANCZOS, reducing_gap=2.0)` med boks = hele bildet.
fn resize_with_reducing_gap(img: &RgbImage, size: (u32, u32)) -> RgbImage {
    let (w, h) = img.dimensions();
    // factor = int((box[2] - box[0]) / size[0] / reducing_gap) or 1
    let factor_x = ((f64::from(w) / f64::from(size.0) / REDUCING_GAP) as u32).max(1);
    let factor_y = ((f64::from(h) / f64::from(size.1) / REDUCING_GAP) as u32).max(1);

    let reduced;
    let (src, bx) = if factor_x > 1 || factor_y > 1 {
        // `_get_safe_box` blir hele bildet når boksen er hele bildet.
        reduced = reduce(img, factor_x, factor_y);
        let bx = [
            0.0,
            0.0,
            (f64::from(w) / f64::from(factor_x)) as f32,
            (f64::from(h) / f64::from(factor_y)) as f32,
        ];
        (&reduced, bx)
    } else {
        (img, [0.0, 0.0, w as f32, h as f32])
    };

    let (sw, sh) = src.dimensions();
    if u64::from(sh) > u64::from(sw) * 100 && size.1 < sh {
        // Svært høye bilder skaleres i to steg (vertikalt først), som i Pillow.
        let tall = resize_box(src, (sw, size.1), [0.0, bx[1], sw as f32, bx[3]]);
        return resize_box(&tall, size, [bx[0], 0.0, bx[2], size.1 as f32]);
    }
    resize_box(src, size, bx)
}

/// `ImagingObject._resize` (C): beskjæring når boksen er heltallig og like stor som
/// målet, ellers `ImagingResample` med Lanczos.
fn resize_box(img: &RgbImage, (xsize, ysize): (u32, u32), b: [f32; 4]) -> RgbImage {
    if b[0] - b[0].trunc() == 0.0
        && b[2] - b[0] == xsize as f32
        && b[1] - b[1].trunc() == 0.0
        && b[3] - b[1] == ysize as f32
    {
        return image::imageops::crop_imm(img, b[0] as u32, b[1] as u32, xsize, ysize).to_image();
    }
    resample(img, xsize, ysize, b)
}

/// `division_UINT32(divider, 8)`: Pillows fastpunkt-resiprok, regnet i `float` som i C.
fn reduce_multiplier(count: u32) -> u32 {
    let max_dividend = (1u32 << 8).wrapping_mul(count);
    let max_int: f32 = 4_294_967_296.0;
    (max_int / max_dividend as f32) as u32
}

/// Snittet av `count` piksler med Pillows avrunding: `((sum + count/2) * mult) >> 24`.
#[inline]
fn reduce_value(sum: u32, count: u32, mult: u32) -> u8 {
    ((sum + count / 2).wrapping_mul(mult) >> 24) as u8
}

/// `Image.reduce((fx, fy))`: boksnitt over `fx × fy`-blokker. Siste kolonne/rad tar
/// med det som er igjen (`ImagingReduceCorners`), så utdata er `ceil(w/fx) × ceil(h/fy)`.
pub fn reduce(img: &RgbImage, fx: u32, fy: u32) -> RgbImage {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let (fx, fy) = (fx as usize, fy as usize);
    let (out_w, out_h) = (w.div_ceil(fx), h.div_ceil(fy));
    let src = img.as_raw();
    let mut out = vec![0u8; out_w * out_h * 3];
    // Kolonnesummer over blokkradene, så summer over `fx` kolonner per utpiksel.
    let mut colsum = vec![0u32; w * 3];
    for (oy, out_row) in out.chunks_exact_mut(out_w * 3).enumerate() {
        let rows = fy.min(h - oy * fy);
        colsum.fill(0);
        for row in src.chunks_exact(w * 3).skip(oy * fy).take(rows) {
            add_row(&mut colsum, row);
        }
        let mult_full = reduce_multiplier((fx * rows) as u32);
        for (ox, px) in out_row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
            let cols = fx.min(w - ox * fx);
            let count = (cols * rows) as u32;
            let mult = if cols == fx {
                mult_full
            } else {
                reduce_multiplier(count)
            };
            let mut sum = [0u32; 3];
            for c in colsum[ox * fx * 3..(ox * fx + cols) * 3].as_chunks::<3>().0 {
                sum[0] += c[0];
                sum[1] += c[1];
                sum[2] += c[2];
            }
            for c in 0..3 {
                px[c] = reduce_value(sum[c], count, mult);
            }
        }
    }
    RgbImage::from_raw(out_w as u32, out_h as u32, out).expect("bufferstørrelsen stemmer")
}

#[inline]
fn add_row(sum: &mut [u32], row: &[u8]) {
    for (s, &v) in sum.iter_mut().zip(row) {
        *s += u32::from(v);
    }
}

/// Pillows `lanczos_filter`: avkuttet sinc, `sinc(x) * sinc(x / 3)` for `-3 <= x < 3`.
fn lanczos(x: f64) -> f64 {
    fn sinc(x: f64) -> f64 {
        if x == 0.0 {
            return 1.0;
        }
        let x = x * std::f64::consts::PI;
        x.sin() / x
    }
    if (-3.0..3.0).contains(&x) {
        sinc(x) * sinc(x / 3.0)
    } else {
        0.0
    }
}

/// Koeffisienter for én akse (`precompute_coeffs` + `normalize_coeffs_8bpc`).
struct Coeffs {
    ksize: usize,
    /// `(xmin, antall)` per utdata-posisjon.
    bounds: Vec<(usize, usize)>,
    /// `ksize` fastpunktvekter per utdata-posisjon.
    weights: Vec<i32>,
}

fn precompute_coeffs(in_size: u32, in0: f32, in1: f32, out_size: u32) -> Coeffs {
    // `(double)(in1 - in0)`: differansen regnes i float, som i C.
    let scale = f64::from(in1 - in0) / f64::from(out_size);
    let filterscale = scale.max(1.0);
    let support = LANCZOS_SUPPORT * filterscale;
    let ksize = support.ceil() as usize * 2 + 1;

    let mut bounds = Vec::with_capacity(out_size as usize);
    let mut weights = vec![0i32; out_size as usize * ksize];
    let mut k = vec![0f64; ksize];
    for xx in 0..out_size as usize {
        let center = f64::from(in0) + (xx as f64 + 0.5) * scale;
        let ss = 1.0 / filterscale;
        // `(int)` i C trunkerer mot null.
        let xmin = ((center - support + 0.5) as i64).max(0);
        let xmax = ((center + support + 0.5) as i64).min(i64::from(in_size)) - xmin;
        let n = xmax.max(0) as usize;
        let mut ww = 0.0;
        for (x, kx) in k.iter_mut().enumerate().take(n) {
            let w = lanczos(((x as i64 + xmin) as f64 - center + 0.5) * ss);
            *kx = w;
            ww += w;
        }
        let row = &mut weights[xx * ksize..(xx + 1) * ksize];
        for (x, kx) in k.iter().enumerate().take(n) {
            let v = if ww != 0.0 { kx / ww } else { *kx };
            let scaled = v * f64::from(1u32 << PRECISION_BITS);
            row[x] = if v < 0.0 {
                (-0.5 + scaled) as i32
            } else {
                (0.5 + scaled) as i32
            };
        }
        bounds.push((xmin as usize, n));
    }
    Coeffs {
        ksize,
        bounds,
        weights,
    }
}

/// `clip8`: `in >> PRECISION_BITS` klemt til 0..=255.
#[inline]
fn clip8(v: i32) -> u8 {
    (v >> PRECISION_BITS).clamp(0, 255) as u8
}

const HALF: i32 = 1 << (PRECISION_BITS - 1);

/// `ImagingResample` med Lanczos: horisontalt pass over radene som trengs, så vertikalt.
fn resample(img: &RgbImage, xsize: u32, ysize: u32, b: [f32; 4]) -> RgbImage {
    let (in_w, in_h) = img.dimensions();
    let need_h = xsize != in_w || b[0] != 0.0 || b[2] != xsize as f32;
    let need_v = ysize != in_h || b[1] != 0.0 || b[3] != ysize as f32;

    let horiz = precompute_coeffs(in_w, b[0], b[2], xsize);
    let mut vert = precompute_coeffs(in_h, b[1], b[3], ysize);
    let ybox_first = vert.bounds[0].0;
    let ybox_last = {
        let (lo, n) = vert.bounds[ysize as usize - 1];
        lo + n
    };

    let mut cur: Option<RgbImage> = None;
    if need_h {
        for bound in &mut vert.bounds {
            bound.0 -= ybox_first;
        }
        let rows = ybox_last - ybox_first;
        let mut out = vec![0u8; xsize as usize * rows * 3];
        let src = img.as_raw();
        let stride = in_w as usize * 3;
        for (yy, out_row) in out.chunks_exact_mut(xsize as usize * 3).enumerate() {
            let row = &src[(yy + ybox_first) * stride..(yy + ybox_first + 1) * stride];
            for (xx, px) in out_row.as_chunks_mut::<3>().0.iter_mut().enumerate() {
                let (xmin, n) = horiz.bounds[xx];
                let k = &horiz.weights[xx * horiz.ksize..xx * horiz.ksize + n];
                let (mut s0, mut s1, mut s2) = (HALF, HALF, HALF);
                for (x, &kx) in k.iter().enumerate() {
                    let p = (xmin + x) * 3;
                    s0 += i32::from(row[p]) * kx;
                    s1 += i32::from(row[p + 1]) * kx;
                    s2 += i32::from(row[p + 2]) * kx;
                }
                px[0] = clip8(s0);
                px[1] = clip8(s1);
                px[2] = clip8(s2);
            }
        }
        cur = Some(RgbImage::from_raw(xsize, rows as u32, out).expect("bufferstørrelsen stemmer"));
    }

    if need_v {
        let src_img = cur.as_ref().unwrap_or(img);
        let width = src_img.width() as usize;
        let src = src_img.as_raw();
        let stride = width * 3;
        let mut out = vec![0u8; stride * ysize as usize];
        let mut acc = vec![0i32; stride];
        for (yy, out_row) in out.chunks_exact_mut(stride).enumerate() {
            let (ymin, n) = vert.bounds[yy];
            let k = &vert.weights[yy * vert.ksize..yy * vert.ksize + n];
            acc.fill(HALF);
            for (y, &ky) in k.iter().enumerate() {
                let row = &src[(ymin + y) * stride..(ymin + y + 1) * stride];
                for (a, &v) in acc.iter_mut().zip(row) {
                    *a += i32::from(v) * ky;
                }
            }
            for (o, &a) in out_row.iter_mut().zip(&acc) {
                *o = clip8(a);
            }
        }
        return RgbImage::from_raw(width as u32, ysize, out).expect("bufferstørrelsen stemmer");
    }
    cur.unwrap_or_else(|| img.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Fasit fra Pillow 12.2 (`Image.new("RGB", (w, h)).thumbnail((t, t))` → `.size`).
    #[test]
    fn thumbnail_size_som_pillow() {
        type Case = ((u32, u32), u32, Option<(u32, u32)>);
        let cases: &[Case] = &[
            ((5472, 3648), 1024, Some((1024, 683))),
            ((3648, 5472), 1024, Some((683, 1024))),
            ((5472, 3648), 300, Some((300, 200))),
            ((4000, 3000), 1024, Some((1024, 768))),
            ((4000, 2672), 1024, Some((1024, 684))),
            ((4000, 2672), 300, Some((300, 200))),
            ((2390, 40), 1024, Some((1024, 17))),
            ((2390, 40), 300, Some((300, 5))),
            ((1590, 340), 300, Some((300, 64))),
            ((402, 874), 300, Some((138, 300))),
            ((402, 874), 1024, None),
            ((1024, 1024), 1024, None),
            ((1025, 1024), 1024, Some((1024, 1023))),
            ((1000, 1), 300, Some((300, 1))),
            ((1, 1000), 300, Some((1, 300))),
            ((10, 20000), 1024, Some((1, 1024))),
            ((3840, 2160), 1024, Some((1024, 576))),
            ((1667, 401), 300, Some((300, 72))),
            ((2458, 1164), 1024, Some((1024, 485))),
            ((3385, 1541), 300, Some((300, 137))),
            ((737, 1495), 300, Some((148, 300))),
            ((1206, 1318), 1024, Some((937, 1024))),
            ((1030, 10), 1024, Some((1024, 10))),
            ((3, 1000), 300, Some((1, 300))),
            ((333, 1000), 300, Some((100, 300))),
            ((1, 200), 300, None),
            ((2, 601), 300, Some((1, 300))),
            ((641, 2), 300, Some((300, 1))),
            ((1366, 768), 300, Some((300, 169))),
            ((768, 1366), 1024, Some((576, 1024))),
        ];
        for &((w, h), t, want) in cases {
            assert_eq!(thumbnail_size(w, h, t), want, "{w}x{h} → {t}");
        }
    }

    #[test]
    fn reduce_multiplikator_som_pillow() {
        // Potenser av to er eksakte (2x2 = >> 2, 4x4 = >> 4 i Pillows spesialvarianter).
        assert_eq!(reduce_multiplier(4), 1 << 22);
        assert_eq!(reduce_multiplier(16), 1 << 20);
        assert_eq!(reduce_multiplier(9), 1_864_135);
        assert_eq!(reduce_multiplier(3), 5_592_405);
        assert_eq!(reduce_multiplier(7), 2_396_745);
        assert_eq!(reduce_multiplier(49), 342_392);
        // Fullt hvitt og fullt svart overlever snittet uendret.
        for n in 1..=64 {
            let m = reduce_multiplier(n);
            assert_eq!(reduce_value(255 * n, n, m), 255, "n={n}");
            assert_eq!(reduce_value(0, n, m), 0, "n={n}");
        }
    }

    #[test]
    fn reduce_tar_med_kantblokker() {
        // 5x3 redusert med 2x2 → 3x2; siste kolonne/rad snitter det som er igjen.
        let img = RgbImage::from_fn(5, 3, |x, y| image::Rgb([(x * 10 + y) as u8, 0, 255]));
        let out = reduce(&img, 2, 2);
        assert_eq!(out.dimensions(), (3, 2));
        // (0,0): piksler 0,1,10,11 → (22 + 2) >> 2 = 6
        assert_eq!(out.get_pixel(0, 0)[0], 6);
        // høyre kant (x=4): 40, 41 → (81 + 1) / 2 = 41
        assert_eq!(out.get_pixel(2, 0)[0], 41);
        // hjørnet (4,2): 42
        assert_eq!(out.get_pixel(2, 1)[0], 42);
        assert_eq!(out.get_pixel(1, 1)[2], 255);
    }

    #[test]
    fn ensfarget_bilde_forblir_ensfarget() {
        let img = RgbImage::from_pixel(5472, 3648, image::Rgb([200, 100, 7]));
        let out = thumbnail(&img, 1024).unwrap();
        assert_eq!(out.dimensions(), (1024, 683));
        assert!(out.pixels().all(|p| p.0 == [200, 100, 7]));
    }

    fn sha256(data: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// Fasit fra Pillow 12.2: `thumbnail((t, t), LANCZOS)` av (et utsnitt av)
    /// tests/fixtures/astronaut.png → SHA-256 av RGB-pikslene. Dekker ren Lanczos
    /// (512 → 300), reduksjon 2×2 + Lanczos (→ 100) og kantblokker ved odde mål.
    #[test]
    fn thumbnail_er_bitlik_pillow() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/astronaut.png");
        let src = image::open(path).unwrap().to_rgb8();
        let cases = [
            (
                (512, 512),
                300,
                (300, 300),
                "fcd32b27fc713bfdac4cc67d71b65acb1c35a68ecfdc3052b0f766a4d7baccfe",
            ),
            (
                (512, 512),
                100,
                (100, 100),
                "a2caca0d4e281c74bab3b41c14e7eee09d3759118936a151a301e6f031dc2d84",
            ),
            (
                (511, 333),
                100,
                (100, 65),
                "6d19d352387c80baa57e8f996f3f5bdc38539dde84741a9b96fe22d8710e0279",
            ),
            (
                (333, 511),
                64,
                (42, 64),
                "734670592a9728697ad7a990155de7e6755eb1188ab5baab5ecc2bd25d8fc1be",
            ),
        ];
        for ((w, h), t, size, want) in cases {
            let crop = image::imageops::crop_imm(&src, 0, 0, w, h).to_image();
            let out = thumbnail(&crop, t).unwrap();
            assert_eq!(out.dimensions(), size, "{w}x{h} → {t}");
            assert_eq!(sha256(out.as_raw()), want, "{w}x{h} → {t}");
        }
    }

    #[test]
    fn lite_bilde_skaleres_ikke_opp() {
        let img = RgbImage::new(200, 100);
        assert!(thumbnail(&img, 300).is_none());
    }
}
