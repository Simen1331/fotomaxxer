//! Filsystem-browse: list undermapper så GUI-et kan velge en kilde-mappe.
//!
//! Motparten til `api/routers/fs.py`. Read-only og bevisst enkelt: vi lister **kun
//! mapper**, hopper skjulte (`.`-prefiks), sorterer på navn uten hensyn til store/små
//! bokstaver (stabilt, som Pythons `sorted`), og lar `parent` peke ett nivå opp
//! (`null` på rota). Ingen skriving, ingen sletting.
//!
//! Windows: `Path.home()` er `%USERPROFILE%`, og på en stasjonsrot (`C:\`) er `parent`
//! `null` — som i Python. Det finnes ingen egen stasjonsliste (heller ikke i Python);
//! en annen stasjon nås ved å sende `?path=D:\`.

use std::path::Path;

use fotomaxxer_core::paths;

use super::Routes;
use crate::error::{ApiError, ApiResult, JsonResponse};
use crate::schemas::{FsEntry, FsList, FsListQuery};
use crate::state::blocking;
use crate::validate::Req;

pub fn routes() -> Routes {
    Routes::new().get("/fs/list", fs_list)
}

/// List undermapper i `path` (standard: brukerens hjemmemappe).
async fn fs_list(req: Req<(), FsListQuery>) -> ApiResult<JsonResponse<FsList>> {
    let path = req.query.path;
    blocking(move || {
        let base = match path.as_deref().filter(|p| !p.is_empty()) {
            Some(p) => paths::resolve(Path::new(p)),
            None => paths::resolve(&paths::home_dir()),
        };
        let shown = paths::to_string(&base);
        if !base.is_dir() {
            return Err(ApiError::http(
                400,
                "not_a_dir",
                format!("Ikke en mappe: {shown}"),
            ));
        }
        let denied = || ApiError::http(403, "denied", format!("Ingen tilgang: {shown}"));
        let read = std::fs::read_dir(&base).map_err(|e| match e.kind() {
            std::io::ErrorKind::PermissionDenied => denied(),
            _ => ApiError::internal(e),
        })?;
        let mut dirs = Vec::new();
        for entry in read {
            let entry = entry.map_err(|e| match e.kind() {
                std::io::ErrorKind::PermissionDenied => denied(),
                _ => ApiError::internal(e),
            })?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let full = base.join(entry.file_name());
            if full.is_dir() && !name.starts_with('.') {
                dirs.push((name.to_lowercase(), name, full));
            }
        }
        // Stabil sortering på nøkkelen, som `sorted(..., key=lambda p: p.name.lower())`.
        dirs.sort_by(|a, b| a.0.cmp(&b.0));
        let parent = base
            .parent()
            .filter(|p| *p != base.as_path())
            .map(paths::to_string);
        Ok(JsonResponse(FsList {
            path: shown,
            parent,
            entries: dirs
                .into_iter()
                .map(|(_, name, full)| FsEntry {
                    name,
                    path: paths::to_string(&full),
                })
                .collect(),
        }))
    })
    .await
}
