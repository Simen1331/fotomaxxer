//! Ansikter: deteksjon + ArcFace-embedding (portert fra `fotomaxxer/faces.py`).
//!
//! Bruker kun de to `buffalo_l`-modellene koden faktisk trenger (`det_10g.onnx`
//! og `w600k_r50.onnx`). Embeddingene er L2-normaliserte `f32` (cosinus = prikkprodukt)
//! og skal være utbyttbare med dem Python-versjonen har lagret.
//!
//! Klyngingen (HDBSCAN + sentroider) ligger her som ren algoritme uten database;
//! `fotomaxxer-core` står for orkestreringen mot `faces`/`persons`.

pub mod detect;
pub mod embed;
pub mod hdbscan;
pub mod imgops;
pub mod linalg;
pub mod npsort;

use std::path::Path;
use std::sync::Mutex;

use image::RgbImage;
use ort::environment::Environment;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;

pub use detect::{Detection, Detector};
pub use embed::Embedder;

pub const EMBED_DIM: usize = 512;
pub const DET_MODEL: &str = "det_10g.onnx";
pub const REC_MODEL: &str = "w600k_r50.onnx";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("ONNX Runtime: {0}")]
    Ort(#[from] ort::Error),
    #[error("uventet modell-output: {0}")]
    Shape(String),
}

/// Et ferdig analysert ansikt: deteksjon + embedding.
#[derive(Debug, Clone)]
pub struct Face {
    pub detection: Detection,
    pub embedding: Vec<f32>,
}

/// Detektor + embedder lastet fra en modellmappe (som `FaceAnalysis` i insightface).
pub struct FaceAnalyzer {
    _env: Environment,
    detector: Detector,
    embedder: Embedder,
}

/// ONNX Runtime tillater bare ett miljø per prosess (og `ort` kan ikke lage et nytt
/// etter at det første er sluppet), så alle analysatorer deler dette.
static ENVIRONMENT: Mutex<Option<Environment>> = Mutex::new(None);

fn environment() -> Result<Environment, Error> {
    let mut guard = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(env) = guard.as_ref() {
        return Ok(env.clone());
    }
    let env = ort::init().build()?;
    *guard = Some(env.clone());
    Ok(env)
}

impl FaceAnalyzer {
    pub fn new(model_dir: &Path) -> Result<Self, Error> {
        Self::with_threads(model_dir, None)
    }

    /// Som [`FaceAnalyzer::new`], men med et fast antall intra-op-tråder per modell
    /// (`None` = ONNX Runtime-standarden, alle fysiske kjerner). Brukes når flere
    /// analysatorer kjører parallelt, så de ikke konkurrerer om de samme kjernene.
    pub fn with_threads(model_dir: &Path, intra_threads: Option<usize>) -> Result<Self, Error> {
        let env = environment()?;
        Ok(Self {
            detector: Detector::new(&env, &model_dir.join(DET_MODEL), intra_threads)?,
            embedder: Embedder::new(&env, &model_dir.join(REC_MODEL), intra_threads)?,
            _env: env,
        })
    }

    /// Alle ansikter i bildet, sortert synkende på deteksjons-score.
    pub fn analyze(&mut self, img: &RgbImage) -> Result<Vec<Face>, Error> {
        self.detector
            .detect(img)?
            .into_iter()
            .map(|detection| {
                let embedding = self.embedder.embed(img, &detection.kps)?;
                Ok(Face {
                    detection,
                    embedding,
                })
            })
            .collect()
    }
}

/// ONNX-sesjon med full grafoptimalisering, som onnxruntime-standarden i Python.
fn session(
    env: &Environment,
    model: &Path,
    intra_threads: Option<usize>,
) -> Result<Session, Error> {
    let mut builder = Session::builder(env)?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(ort::Error::from)?;
    if let Some(n) = intra_threads {
        builder = builder.with_intra_threads(n).map_err(ort::Error::from)?;
    }
    Ok(builder.commit_from_file(model)?)
}
