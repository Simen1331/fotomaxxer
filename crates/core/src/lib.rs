//! FotoMaxxer-pipelinen portert fra Python-pakka `fotomaxxer/`.
//!
//! Én modul per pipeline-steg / concern, som i Python-versjonen. Stegene bindes
//! sammen av SQLite-skjemaet og `processing_state`, ikke av modulstrukturen.
//! Funksjonene tar `(&Connection, &Storage, …)` og kalles uendret fra CLI og API.

pub mod cancel;
pub mod db;
pub mod derive;
pub mod error;
pub mod exif;
pub mod faces;
pub mod geocode;
pub mod imageio;
pub mod jpegdec;
pub mod jpegenc;
pub mod paths;
pub mod profiles;
pub mod pyjson;
pub mod resample;
pub mod resources;
pub mod scan;
pub mod settings;
mod simd;
pub mod sources;
pub mod storage;
pub mod tagging;

pub use error::{Error, Result};
pub use storage::Storage;
