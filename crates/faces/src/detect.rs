//! SCRFD-ansiktsdetektor (`det_10g.onnx` fra `buffalo_l`).
//!
//! Speiler `insightface.model_zoo.scrfd.SCRFD` med `det_size=(640, 640)`:
//! aspektbevarende skalering øverst til venstre i 640×640, normalisering
//! `(x - 127.5) / 128`, ankre for stride 8/16/32 (2 per punkt), terskel 0,5 og NMS 0,4.

use std::path::Path;

use image::RgbImage;
use ort::environment::Environment;
use ort::session::Session;
use ort::value::Tensor;

use crate::imgops::{resize_linear, to_nchw};
use crate::{Error, session};

const INPUT_SIZE: u32 = 640;
const STRIDES: [usize; 3] = [8, 16, 32];
const NUM_ANCHORS: usize = 2;
const MEAN: f32 = 127.5;
const STD: f32 = 128.0;
pub const DET_THRESH: f32 = 0.5;
const NMS_THRESH: f32 = 0.4;

/// Ett detektert ansikt i koordinatene til bildet som ble analysert.
#[derive(Debug, Clone, PartialEq)]
pub struct Detection {
    /// `[x1, y1, x2, y2]`
    pub bbox: [f32; 4],
    /// Øyne, nese, munnviker (venstre → høyre).
    pub kps: [[f32; 2]; 5],
    pub score: f32,
}

pub struct Detector {
    session: Session,
}

impl Detector {
    pub fn new(
        env: &Environment,
        model: &Path,
        intra_threads: Option<usize>,
    ) -> Result<Self, Error> {
        Ok(Self {
            session: session(env, model, intra_threads)?,
        })
    }

    pub fn detect(&mut self, img: &RgbImage) -> Result<Vec<Detection>, Error> {
        let (w, h) = img.dimensions();
        if w == 0 || h == 0 {
            return Ok(Vec::new());
        }
        let im_ratio = h as f32 / w as f32;
        let (new_w, new_h) = if im_ratio > 1.0 {
            ((INPUT_SIZE as f32 / im_ratio) as u32, INPUT_SIZE)
        } else {
            (INPUT_SIZE, (INPUT_SIZE as f32 * im_ratio) as u32)
        };
        let det_scale = new_h as f32 / h as f32;

        let resized = resize_linear(img, new_w.max(1), new_h.max(1));
        let mut padded = RgbImage::new(INPUT_SIZE, INPUT_SIZE);
        image::imageops::replace(&mut padded, &resized, 0, 0);

        let n = INPUT_SIZE as usize;
        let input = Tensor::from_array(([1usize, 3, n, n], to_nchw(&padded, MEAN, STD)))?;
        let outputs = self.session.run(ort::inputs![input])?;

        let mut dets = Vec::new();
        for (level, &stride) in STRIDES.iter().enumerate() {
            let (_, scores) = outputs[level].try_extract_tensor::<f32>()?;
            let (_, boxes) = outputs[level + STRIDES.len()].try_extract_tensor::<f32>()?;
            let (_, kps) = outputs[level + 2 * STRIDES.len()].try_extract_tensor::<f32>()?;
            let side = n / stride;
            let s = stride as f32;
            for (k, &score) in scores.iter().enumerate() {
                if score < DET_THRESH {
                    continue;
                }
                let cell = k / NUM_ANCHORS;
                let cx = (cell % side * stride) as f32;
                let cy = (cell / side * stride) as f32;
                let b = &boxes[k * 4..k * 4 + 4];
                let p = &kps[k * 10..k * 10 + 10];
                let mut kp = [[0.0f32; 2]; 5];
                for (m, pt) in kp.iter_mut().enumerate() {
                    *pt = [
                        (cx + p[2 * m] * s) / det_scale,
                        (cy + p[2 * m + 1] * s) / det_scale,
                    ];
                }
                dets.push(Detection {
                    bbox: [
                        (cx - b[0] * s) / det_scale,
                        (cy - b[1] * s) / det_scale,
                        (cx + b[2] * s) / det_scale,
                        (cy + b[3] * s) / det_scale,
                    ],
                    kps: kp,
                    score,
                });
            }
        }
        dets.sort_by(|a, b| b.score.total_cmp(&a.score));
        Ok(nms(dets, NMS_THRESH))
    }
}

/// Grådig NMS med insightface sin `+1`-arealkonvensjon. Input sortert synkende på score.
fn nms(dets: Vec<Detection>, thresh: f32) -> Vec<Detection> {
    let area = |d: &Detection| (d.bbox[2] - d.bbox[0] + 1.0) * (d.bbox[3] - d.bbox[1] + 1.0);
    let mut keep: Vec<Detection> = Vec::new();
    let mut suppressed = vec![false; dets.len()];
    for i in 0..dets.len() {
        if suppressed[i] {
            continue;
        }
        let a = &dets[i];
        for j in i + 1..dets.len() {
            if suppressed[j] {
                continue;
            }
            let b = &dets[j];
            let iw = (a.bbox[2].min(b.bbox[2]) - a.bbox[0].max(b.bbox[0]) + 1.0).max(0.0);
            let ih = (a.bbox[3].min(b.bbox[3]) - a.bbox[1].max(b.bbox[1]) + 1.0).max(0.0);
            let inter = iw * ih;
            if inter / (area(a) + area(b) - inter) > thresh {
                suppressed[j] = true;
            }
        }
        keep.push(a.clone());
    }
    keep
}

#[cfg(test)]
mod tests {
    use super::*;

    fn det(bbox: [f32; 4], score: f32) -> Detection {
        Detection {
            bbox,
            kps: [[0.0; 2]; 5],
            score,
        }
    }

    #[test]
    fn nms_fjerner_overlapp_og_beholder_separate() {
        let dets = vec![
            det([0.0, 0.0, 10.0, 10.0], 0.9),
            det([1.0, 1.0, 11.0, 11.0], 0.8),
            det([50.0, 50.0, 60.0, 60.0], 0.7),
        ];
        let kept = nms(dets, NMS_THRESH);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].score, 0.9);
        assert_eq!(kept[1].score, 0.7);
    }
}
