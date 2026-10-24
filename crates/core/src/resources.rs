//! Skrivebeskyttede appressurser, uavhengig av datamappe og arbeidsmappe.
//!
//! Ansiktsmodellene (`det_10g.onnx` + `w600k_r50.onnx` fra insightface `buffalo_l`)
//! leveres i en mappe `models/` ved siden av programfila. Rekkefølge for oppslag:
//!
//! 1. `FOTOMAXXER_MODELS` (miljøvariabel) — brukes alltid når den er satt.
//! 2. `<mappa til exe>/models` — den pakkede leveransen.
//! 3. `~/.insightface/models/buffalo_l` — der insightface selv laster ned modellene
//!    (utviklingsmaskiner med Python-versjonen installert).
//!
//! Den første mappa som finnes, velges. Mangler en modellfil der, gis en tydelig feil
//! i stedet for å lete videre, så en ufullstendig pakke ikke stille bruker noe annet.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use fotomaxxer_faces::{DET_MODEL, REC_MODEL};

use crate::error::{Error, Result};
use crate::paths;

pub const ENV_MODELS: &str = "FOTOMAXXER_MODELS";
/// Modellfilene ansiktssteget trenger.
pub const FACE_MODEL_FILES: [&str; 2] = [DET_MODEL, REC_MODEL];

/// Mappa med ansiktsmodellene, eller en norsk feilmelding om hva som mangler.
pub fn face_model_dir() -> Result<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    find_model_dir(
        std::env::var_os(ENV_MODELS),
        exe_dir.as_deref(),
        &paths::home_dir(),
    )
}

/// Oppslaget med eksplisitte innganger (testbart uten å røre prosessmiljøet).
pub fn find_model_dir(
    env: Option<OsString>,
    exe_dir: Option<&Path>,
    home: &Path,
) -> Result<PathBuf> {
    if let Some(dir) = env.filter(|v| !v.is_empty()) {
        return validate(PathBuf::from(dir), &format!("{ENV_MODELS} peker på"));
    }
    let mut candidates = Vec::new();
    if let Some(exe_dir) = exe_dir {
        candidates.push(exe_dir.join("models"));
    }
    candidates.push(home.join(".insightface").join("models").join("buffalo_l"));
    match candidates.iter().find(|d| d.is_dir()) {
        Some(dir) => validate(dir.clone(), "Modellmappa"),
        None => Err(Error::Models(format!(
            "Fant ikke ansiktsmodellene ({}). Leter i: {}. Legg dem i mappa «models» \
             ved siden av programmet, eller sett {ENV_MODELS} til mappa som har dem.",
            FACE_MODEL_FILES.join(", "),
            candidates
                .iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn validate(dir: PathBuf, what: &str) -> Result<PathBuf> {
    let missing: Vec<&str> = FACE_MODEL_FILES
        .iter()
        .copied()
        .filter(|name| !dir.join(name).is_file())
        .collect();
    if missing.is_empty() {
        Ok(dir)
    } else {
        Err(Error::Models(format!(
            "Ansiktsmodeller mangler: {}. {what} {}, men der finnes ikke {}.",
            missing.join(", "),
            dir.display(),
            missing.join(" eller ")
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn with_models(dir: &Path, files: &[&str]) {
        fs::create_dir_all(dir).unwrap();
        for f in files {
            fs::write(dir.join(f), b"onnx").unwrap();
        }
    }

    #[test]
    fn rekkefolge_env_exe_insightface() {
        let tmp = tempfile::tempdir().unwrap();
        let (env_dir, exe, home) = (
            tmp.path().join("env"),
            tmp.path().join("app"),
            tmp.path().join("home"),
        );
        let insight = home.join(".insightface/models/buffalo_l");
        with_models(&insight, &FACE_MODEL_FILES);
        // Ingen exe/models → insightface-mappa.
        assert_eq!(find_model_dir(None, Some(&exe), &home).unwrap(), insight);
        // exe/models finnes → den vinner.
        with_models(&exe.join("models"), &FACE_MODEL_FILES);
        assert_eq!(
            find_model_dir(None, Some(&exe), &home).unwrap(),
            exe.join("models")
        );
        // Miljøvariabelen slår alt.
        with_models(&env_dir, &FACE_MODEL_FILES);
        assert_eq!(
            find_model_dir(Some(env_dir.clone().into()), Some(&exe), &home).unwrap(),
            env_dir
        );
        // Tom miljøvariabel ignoreres.
        assert_eq!(
            find_model_dir(Some(OsString::new()), Some(&exe), &home).unwrap(),
            exe.join("models")
        );
    }

    #[test]
    fn tydelig_feil_naar_modeller_mangler() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("app");
        with_models(&exe.join("models"), &[DET_MODEL]);
        let err = find_model_dir(None, Some(&exe), tmp.path())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("w600k_r50.onnx") && !err.contains("det_10g.onnx,"),
            "{err}"
        );
        assert!(err.starts_with("Ansiktsmodeller mangler"), "{err}");

        let none = find_model_dir(None, Some(&tmp.path().join("x")), &tmp.path().join("h"))
            .unwrap_err()
            .to_string();
        assert!(none.starts_with("Fant ikke ansiktsmodellene"), "{none}");
        assert!(none.contains(ENV_MODELS), "{none}");
    }
}
