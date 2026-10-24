//! Felles feiltype for pipelinen.
//!
//! Brukerrettede feil (`Source`, `Profile`, `Schema`) bærer ferdig norsk tekst som
//! CLI og API viser uendret — samme meldinger som `SourceError`/`ProfileError` i Python.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// Databasen har en skjemaversjon appen ikke kjenner.
    #[error("{0}")]
    Schema(String),
    #[error("{0}")]
    Source(String),
    #[error("{0}")]
    Profile(String),
    /// Ugyldig ansikts-/personoperasjon (Pythons `ValueError` i `faces.py`; API-et gir 400/404).
    #[error("{0}")]
    Faces(String),
    /// Ansiktsmodellene mangler eller kan ikke lastes.
    #[error("{0}")]
    Models(String),
    /// Konfig-/oppsettsfeil som stopper hele AI-merkingen (Pythons `TaggingError`).
    #[error("{0}")]
    Tagging(String),
    /// Jobben ble avbrutt ved et trygt punkt (se [`crate::cancel`]).
    #[error("avbrutt")]
    Cancelled,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
