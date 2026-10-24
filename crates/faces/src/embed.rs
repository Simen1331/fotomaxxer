//! ArcFace-embedding (`w600k_r50.onnx` fra `buffalo_l`).
//!
//! Speiler `insightface.utils.face_align.norm_crop` + `ArcFaceONNX.get_feat`:
//! likhetstransformasjon fra 5 nøkkelpunkter til ArcFace-malen, 112×112-warp,
//! normalisering `(x - 127.5) / 127.5`, og til slutt L2-normalisering.

use std::path::Path;

use image::RgbImage;
use ort::environment::Environment;
use ort::session::Session;
use ort::value::Tensor;

use crate::imgops::{to_nchw, warp_affine};
use crate::{EMBED_DIM, Error, session};

const CROP: u32 = 112;
const MEAN: f32 = 127.5;
const STD: f32 = 127.5;

/// ArcFace-malens nøkkelpunkter for en 112×112-crop.
const ARCFACE_DST: [[f64; 2]; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

pub struct Embedder {
    session: Session,
}

impl Embedder {
    pub fn new(
        env: &Environment,
        model: &Path,
        intra_threads: Option<usize>,
    ) -> Result<Self, Error> {
        Ok(Self {
            session: session(env, model, intra_threads)?,
        })
    }

    /// L2-normalisert embedding for ansiktet med nøkkelpunktene `kps`.
    pub fn embed(&mut self, img: &RgbImage, kps: &[[f32; 2]; 5]) -> Result<Vec<f32>, Error> {
        let crop = align(img, kps);
        let n = CROP as usize;
        let input = Tensor::from_array(([1usize, 3, n, n], to_nchw(&crop, MEAN, STD)))?;
        let outputs = self.session.run(ort::inputs![input])?;
        let (_, raw) = outputs[0].try_extract_tensor::<f32>()?;
        if raw.len() != EMBED_DIM {
            return Err(Error::Shape(format!(
                "ArcFace ga {} verdier, ventet {EMBED_DIM}",
                raw.len()
            )));
        }
        Ok(l2_normalize(raw))
    }
}

/// 112×112-crop justert etter nøkkelpunktene (som `face_align.norm_crop`).
pub fn align(img: &RgbImage, kps: &[[f32; 2]; 5]) -> RgbImage {
    let src = kps.map(|[x, y]| [x as f64, y as f64]);
    warp_affine(img, &similarity(&src, &ARCFACE_DST), CROP, CROP)
}

/// Minste kvadraters likhetstransformasjon (skala, rotasjon, translasjon) src → dst.
///
/// I 2D er dette det samme som Umeyama uten speiling (`skimage.SimilarityTransform`):
/// punktene sees som komplekse tall og `dst ≈ c·src + t` løses lukket.
fn similarity(src: &[[f64; 2]; 5], dst: &[[f64; 2]; 5]) -> [[f64; 3]; 2] {
    let mean = |p: &[[f64; 2]; 5]| {
        let (sx, sy) = p
            .iter()
            .fold((0.0, 0.0), |(ax, ay), [x, y]| (ax + x, ay + y));
        (sx / 5.0, sy / 5.0)
    };
    let (smx, smy) = mean(src);
    let (dmx, dmy) = mean(dst);
    let (mut num_re, mut num_im, mut den) = (0.0, 0.0, 0.0);
    for i in 0..5 {
        let (x, y) = (src[i][0] - smx, src[i][1] - smy);
        let (u, v) = (dst[i][0] - dmx, dst[i][1] - dmy);
        num_re += x * u + y * v;
        num_im += x * v - y * u;
        den += x * x + y * y;
    }
    let (a, b) = if den > 0.0 {
        (num_re / den, num_im / den)
    } else {
        (1.0, 0.0)
    };
    let tx = dmx - (a * smx - b * smy);
    let ty = dmy - (b * smx + a * smy);
    [[a, -b, tx], [b, a, ty]]
}

pub fn l2_normalize(v: &[f32]) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter().map(|x| x / norm).collect()
    } else {
        v.to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similarity_gjenfinner_kjent_transform() {
        // dst = 2·R(30°)·src + (5, -3)
        let (s, th) = (2.0f64, 30f64.to_radians());
        let (a, b) = (s * th.cos(), s * th.sin());
        let src = [[1.0, 2.0], [3.0, 1.0], [2.0, 4.0], [0.0, 0.0], [5.0, 5.0]];
        let dst = src.map(|[x, y]| [a * x - b * y + 5.0, b * x + a * y - 3.0]);
        let m = similarity(&src, &dst);
        let want = [[a, -b, 5.0], [b, a, -3.0]];
        for r in 0..2 {
            for c in 0..3 {
                assert!((m[r][c] - want[r][c]).abs() < 1e-9, "{m:?} != {want:?}");
            }
        }
    }

    #[test]
    fn l2_normalize_gir_lengde_en() {
        let v = l2_normalize(&[3.0, 4.0]);
        assert!((v[0] - 0.6).abs() < 1e-6 && (v[1] - 0.8).abs() < 1e-6);
    }
}
