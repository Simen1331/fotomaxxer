//! AI-steg: ett vision-kall per bilde via OpenRouter på 1024-derivatet.
//!
//! Leverandøren ligger bak ett lite grensesnitt (`VisionProvider`) — kun
//! `OpenRouterProvider` finnes nå, en lokal modell (f.eks. Ollama) kan plugges inn
//! senere uten å røre kall-stedet. Leverandøren velges av `settings` (nøkkel +
//! modell), aldri `.env`. Nettverksbundet → trådpool (ikke prosesser som `derive`),
//! eksponentiell backoff på 429/5xx. Idempotent og myk per bilde: én råtten respons
//! stopper ikke batchen.
//!
//! Til forskjell fra alt annet i FotoMaxxer treffer dette steget et betalt API.
//! `tag` teller pending og krever bekreftelse over en terskel med mindre
//! `assume_yes` er satt.
//!
//! Portert fra `fotomaxxer/tagging.py` med samme prompt (tegn for tegn), samme
//! HTTP-kropp (byte for byte, som `requests` lager den), samme tolerante parsing og
//! samme feiltekster per bilde (`"Type: melding"` med Pythons unntaksnavn).
//!
//! Testkrok: miljøvariabelen `FOTOMAXXER_OPENROUTER_URL` overstyrer endepunktet,
//! slik at testene kan kjøre mot en lokal, falsk tjener. Den er bare ment for tester.
//!
//! Bevisste avvik fra Python (alle utenfor normal drift):
//! - Negativt `workers` gir `Error::Tagging` i stedet for `ValueError` med traceback.
//! - En fatal DB-feil midt i kjøringen stopper utdelingen av nye kall (Python gjør
//!   ferdig alle køede, betalte kall før feilen slipper ut).
//! - JSON som bare Python godtar (`NaN`, `Infinity`, `1e400`, ensomme surrogater,
//!   nøsting over 128 nivåer) gir `JSONDecodeError` her. Heltall utenfor `u64` blir
//!   flyttall, og `person_count` utenfor SQLite INTEGER blir `NULL` (Python krasjer).
//! - Nettverksfeil får Python-lignende typenavn (`ConnectionError`, `ReadTimeout`),
//!   men ureq sin feiltekst.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use base64::Engine;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, params, params_from_iter};
use serde_json::{Number, Value, json};

use crate::error::{Error, Result};
use crate::storage::Storage;
use crate::{cancel, pyjson, settings};

pub const DEFAULT_WORKERS: usize = 6;
/// Batcher over dette antallet krever eksplisitt bekreftelse (`--yes` hopper over).
pub const CONFIRM_THRESHOLD: usize = 200;

pub const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
/// Overstyrer `OPENROUTER_URL`. Kun for tester mot en falsk tjener.
pub const URL_ENV: &str = "FOTOMAXXER_OPENROUTER_URL";
const MAX_ATTEMPTS: u32 = 5;
const TEMPERATURE: f64 = 0.2;
/// Som `requests.post(..., timeout=90)`: grense for tilkobling og for hver lesing/skriving.
const TIMEOUT: Duration = Duration::from_secs(90);

const MISSING_KEY: &str =
    "Mangler OpenRouter-nøkkel. Kjør: fotomaxxer settings set openrouter_api_key <key>";

const PROMPT: &str = concat!(
    "Du analyserer ett privat familiebilde for et fotoalbum-arkiv. Svar KUN med JSON,\n",
    "ingen forklaring, på dette formatet:\n",
    "{\n",
    "  \"scene\": \"<kort scene, f.eks. strand, skog, kjøkken>\",\n",
    "  \"event_type\": \"<hverdag|bursdag|jul|ferie|bryllup|tur|fest|annet>\",\n",
    "  \"season\": \"<vinter|vår|sommer|høst|null>\",\n",
    "  \"indoor_outdoor\": \"<ute|inne|null>\",\n",
    "  \"person_count\": <heltall, antall synlige personer>,\n",
    "  \"description\": \"<1–2 setninger på norsk om hva bildet viser>\",\n",
    "  \"tags\": [\"<3–8 frie stikkord på norsk>\"]\n",
    "}\n",
    "Bruk null der du er usikker. Ikke gjett på navn eller identitet.",
);

const TAGS_LINE: &str = "  \"tags\": [\"<3–8 frie stikkord på norsk>\"]\n";

/// Bygg vision-prompten ut fra en tagging-profil. Tre stier:
///
/// 1. Tom fokus → eksakt grunnprompten (bit for bit) — standardstien bevart.
/// 2. Fokus, ingen kategorier → berik flate tags + description, hev tak 3–8 → 3–12.
/// 3. Fokus + kategorier → be modellen bøtte stikkord per kategori (`tags` blir et
///    objekt nøklet på kategoriene), grunnform/entall, 0–4 per kategori.
///
/// `event_type`-lista og resten av JSON-skjemaet er uendret i alle stier: kun frie
/// tags + description berikes.
pub fn build_prompt(focus: Option<&str>, categories: &[String]) -> String {
    let Some(focus) = focus.map(py_strip).filter(|f| !f.is_empty()) else {
        return PROMPT.to_string();
    };
    let cats: Vec<&str> = categories
        .iter()
        .map(|c| py_strip(c))
        .filter(|c| !c.is_empty())
        .collect();

    if cats.is_empty() {
        let directive = format!(
            "Fokusområde: {focus}.\n\
             Når motiver innen dette området er i bildet, gi flere og mer spesifikke stikkord\n\
             (materiale, teknikk, stil, periode der relevant) og en rikere description. Er fokusområdet\n\
             ikke representert i bildet, merk helt som vanlig.\n\n"
        );
        let body = PROMPT.replace(
            "\"<3–8 frie stikkord på norsk>\"",
            "\"<3–12 frie stikkord på norsk>\"",
        );
        return directive + &body;
    }

    let cat_list = cats.join(", ");
    let cat_json = cats
        .iter()
        .map(|c| format!("\"{c}\": [\"<grunnform>\"]"))
        .collect::<Vec<_>>()
        .join(", ");
    let directive = format!(
        "Fokusområde: {focus}.\n\
         Når motiver innen dette området er i bildet, gi spesifikke stikkord gruppert i disse \
         kategoriene: {cat_list}.\n\
         Bruk grunnform (entall, ikke bøyning), 0–4 stikkord per kategori, kun der det er dekning i\n\
         bildet. Er fokusområdet ikke representert i bildet, merk helt som vanlig.\n\n"
    );
    let body = PROMPT.replace(TAGS_LINE, &format!("  \"tags\": {{ {cat_json} }}\n"));
    directive + &body
}

/// Resultatet av ett vision-kall, normalisert.
#[derive(Debug, Clone, PartialEq)]
pub struct VisionResult {
    pub scene: Option<String>,
    pub event_type: Option<String>,
    pub season: Option<String>,
    pub indoor_outdoor: Option<String>,
    pub person_count: Option<i64>,
    pub description: Option<String>,
    /// Flate navn (dedup, rekkefølge bevart).
    pub tags: Vec<String>,
    /// Navn → kategori (kun de kategoriserte).
    pub tag_categories: HashMap<String, String>,
    /// Hele leverandørsvaret, lagres som `ai_tags.raw_json`.
    pub raw: Value,
}

/// Feil for ett bilde. Lagres som `processing_state.error` på formen `"Type: melding"`
/// med Pythons unntaksnavn (`TaggingError`, `JSONDecodeError`, `KeyError` …), slik at
/// feiltekstene blir de samme som i Python-versjonen der det lar seg gjøre.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageError {
    pub kind: String,
    pub message: String,
}

impl ImageError {
    pub fn new(kind: &str, message: impl Into<String>) -> Self {
        Self {
            kind: kind.to_string(),
            message: message.into(),
        }
    }
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

impl std::error::Error for ImageError {}

/// En vision-leverandør. Kalles fra flere arbeidstråder samtidig og rører aldri DB-en.
pub trait VisionProvider: Send + Sync {
    /// Lagres i `ai_tags.provider`, f.eks. `openrouter:google/gemini-2.5-flash`.
    fn name(&self) -> &str;

    fn tag(
        &self,
        image_path: &Path,
        focus: Option<&str>,
        categories: &[String],
    ) -> Result<VisionResult, ImageError>;
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TagStats {
    pub pending: usize,
    pub done: usize,
    pub errors: usize,
    /// `true` hvis brukeren avbrøt ved kostnadsgrensen.
    pub skipped: bool,
}

impl TagStats {
    pub fn summary(&self) -> String {
        if self.skipped {
            return format!("avbrutt (pending={}, ingen kall gjort)", self.pending);
        }
        format!(
            "pending={} done={} feil={}",
            self.pending, self.done, self.errors
        )
    }
}

// --- Leverandør ------------------------------------------------------------

/// Vision-tagging via OpenRouter chat-completions med bilde-input.
pub struct OpenRouterProvider {
    api_key: String,
    model: String,
    name: String,
    url: String,
    agent: ureq::Agent,
    /// Første ventetid i backoff (1 s som i Python; kortere i enhetstester).
    backoff: Duration,
}

impl OpenRouterProvider {
    /// Endepunktet er `OPENROUTER_URL`, eller `FOTOMAXXER_OPENROUTER_URL` i tester.
    pub fn new(api_key: &str, model: &str) -> Self {
        let url = std::env::var(URL_ENV)
            .ok()
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| OPENROUTER_URL.to_string());
        Self::with_url(api_key, model, &url)
    }

    pub fn with_url(api_key: &str, model: &str, url: &str) -> Self {
        let agent = ureq::Agent::config_builder()
            // 4xx/5xx er vanlige svar her; backoff-logikken leser status og kropp selv.
            .http_status_as_error(false)
            .timeout_connect(Some(TIMEOUT))
            .timeout_per_read(Some(TIMEOUT))
            .timeout_per_write(Some(TIMEOUT))
            .build()
            .into();
        Self {
            api_key: api_key.to_string(),
            model: model.to_string(),
            name: format!("openrouter:{model}"),
            url: url.to_string(),
            agent,
            backoff: Duration::from_secs(1),
        }
    }

    #[cfg(test)]
    fn with_backoff(mut self, backoff: Duration) -> Self {
        self.backoff = backoff;
        self
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// POST med eksponentiell backoff (1 s, doblende, 5 forsøk). Som i Python prøves
    /// alt på nytt: 429/5xx, andre HTTP-feil, nettverksfeil og ugyldig JSON-kropp
    /// (alt det `requests` regner som `RequestException`).
    fn post_with_backoff(&self, body: &[u8]) -> Result<Value, ImageError> {
        let mut delay = self.backoff;
        let mut attempt = 1;
        loop {
            match self.post_once(body) {
                Ok(v) => return Ok(v),
                Err(e) if attempt >= MAX_ATTEMPTS => return Err(e),
                Err(_) => {
                    std::thread::sleep(delay);
                    delay *= 2;
                    attempt += 1;
                }
            }
        }
    }

    fn post_once(&self, body: &[u8]) -> Result<Value, ImageError> {
        let mut resp = self
            .agent
            .post(&self.url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .header("X-Title", "FotoMaxxer")
            .send(body)
            .map_err(transport_error)?;
        let status = resp.status();
        let bytes = resp.body_mut().read_to_vec().map_err(transport_error)?;
        let text = String::from_utf8_lossy(&bytes);
        let code = status.as_u16();
        if code == 429 || code >= 500 {
            let head: String = text.chars().take(200).collect();
            return Err(ImageError::new(
                "TaggingError",
                format!("HTTP {code}: {head}"),
            ));
        }
        if code >= 400 {
            // `resp.raise_for_status()` (5xx er håndtert over).
            return Err(ImageError::new(
                "HTTPError",
                format!(
                    "{code} Client Error: {} for url: {}",
                    status.canonical_reason().unwrap_or(""),
                    self.url
                ),
            ));
        }
        py_json_loads(&text)
    }
}

impl VisionProvider for OpenRouterProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn tag(
        &self,
        image_path: &Path,
        focus: Option<&str>,
        categories: &[String],
    ) -> Result<VisionResult, ImageError> {
        let data_url = encode_image(image_path)?;
        let payload = build_payload(&self.model, &build_prompt(focus, categories), &data_url);
        // `requests.post(json=…)` serialiserer med `json.dumps` (ensure_ascii=True).
        let body = pyjson::dumps_ascii(&payload);
        let raw = self.post_with_backoff(body.as_bytes())?;
        parse_response(raw)
    }
}

/// Request-kroppen til chat-completions (nøkkelrekkefølge som i Python).
pub fn build_payload(model: &str, prompt: &str, data_url: &str) -> Value {
    json!({
        "model": model,
        "temperature": TEMPERATURE,
        "response_format": {"type": "json_object"},
        "usage": {"include": true}, // be OpenRouter returnere faktisk kost i usage.cost
        "messages": [
            {
                "role": "user",
                "content": [
                    {"type": "text", "text": prompt},
                    {"type": "image_url", "image_url": {"url": data_url}},
                ],
            }
        ],
    })
}

fn transport_error(e: ureq::Error) -> ImageError {
    let kind = match e {
        ureq::Error::Timeout(ureq::Timeout::Connect) => "ConnectTimeout",
        ureq::Error::Timeout(_) => "ReadTimeout",
        _ => "ConnectionError",
    };
    ImageError::new(kind, e.to_string())
}

fn encode_image(image_path: &Path) -> Result<String, ImageError> {
    let bytes = std::fs::read(image_path).map_err(|e| os_error(&e, image_path))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(format!("data:image/jpeg;base64,{b64}"))
}

/// Pythons `OSError`-tekst: `[Errno 13] Permission denied: '/sti'`.
fn os_error(e: &std::io::Error, path: &Path) -> ImageError {
    use std::io::ErrorKind;
    let kind = match e.kind() {
        ErrorKind::NotFound => "FileNotFoundError",
        ErrorKind::PermissionDenied => "PermissionError",
        ErrorKind::IsADirectory => "IsADirectoryError",
        _ => "OSError",
    };
    let message = match e.raw_os_error() {
        Some(errno) => {
            let text = e.to_string();
            let desc = text
                .strip_suffix(&format!(" (os error {errno})"))
                .unwrap_or(&text)
                .to_string();
            format!(
                "[Errno {errno}] {desc}: {}",
                py_repr_str(&path.to_string_lossy())
            )
        }
        None => e.to_string(),
    };
    ImageError::new(kind, message)
}

/// `raw["choices"][0]["message"]["content"]` → tolerant parse → `VisionResult`.
pub fn parse_response(raw: Value) -> Result<VisionResult, ImageError> {
    let content = py_getitem(&raw, Key::Str("choices"))
        .and_then(|v| py_getitem(&v, Key::Index(0)))
        .and_then(|v| py_getitem(&v, Key::Str("message")))
        .and_then(|v| py_getitem(&v, Key::Str("content")))?;
    let Value::String(content) = content else {
        return Err(ImageError::new(
            "AttributeError",
            format!("'{}' object has no attribute 'strip'", py_type(&content)),
        ));
    };
    let parsed = parse_json(&content)?;
    to_result(&parsed, raw)
}

/// Tolerant JSON-parse: strip ev. kodegjerder, finn ytre `{…}`.
pub fn parse_json(content: &str) -> Result<Value, ImageError> {
    let mut text = py_strip(content);
    if text.starts_with("```") {
        // ```json … ``` eller ``` … ```
        text = if text.matches("```").count() >= 2 {
            text.split("```").nth(1).unwrap_or("")
        } else {
            text.trim_matches('`')
        };
        let lead = py_lstrip(text);
        if lead
            .get(..4)
            .is_some_and(|p| p.eq_ignore_ascii_case("json"))
        {
            text = &lead[4..];
        }
    }
    let text = py_strip(text);
    match py_json_loads(text) {
        Ok(v) => Ok(v),
        Err(first) => match (text.find('{'), text.rfind('}')) {
            (Some(start), Some(end)) if end > start => py_json_loads(&text[start..=end]),
            _ => Err(first),
        },
    }
}

/// Map en (muligens ufullstendig) JSON-verdi til `VisionResult`. Manglende → null.
pub fn to_result(parsed: &Value, raw: Value) -> Result<VisionResult, ImageError> {
    let Value::Object(obj) = parsed else {
        return Err(ImageError::new(
            "AttributeError",
            format!("'{}' object has no attribute 'get'", py_type(parsed)),
        ));
    };
    let (tags, tag_categories) = parse_tags(obj.get("tags"));
    Ok(VisionResult {
        scene: opt_str(obj.get("scene")),
        event_type: opt_str(obj.get("event_type")),
        season: opt_str(obj.get("season")),
        indoor_outdoor: opt_str(obj.get("indoor_outdoor")),
        person_count: person_count(obj.get("person_count")),
        description: opt_str(obj.get("description")),
        tags,
        tag_categories,
        raw,
    })
}

/// Pythons `_str`: `str(v).strip()`, men `None`, tom og `"null"` (uansett case) → `None`.
fn opt_str(v: Option<&Value>) -> Option<String> {
    let v = v.filter(|v| !v.is_null())?;
    let s = py_str(v);
    let s = py_strip(&s);
    (!s.is_empty() && s.to_lowercase() != "null").then(|| s.to_string())
}

/// Pythons `int(pc)` med `ValueError`/`TypeError` → `None`.
fn person_count(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Null | Value::Array(_) | Value::Object(_) => None,
        Value::Bool(b) => Some(i64::from(*b)),
        Value::String(s) => py_int(s),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                return Some(i);
            }
            if n.is_u64() {
                return None; // Python: for stort for SQLite INTEGER
            }
            let t = n.as_f64()?.trunc();
            (t.is_finite() && t >= i64::MIN as f64 && t < i64::MAX as f64).then_some(t as i64)
        }
    }
}

/// Normaliser `tags` til (flate navn, navn→kategori). Tåler begge formene:
///
/// - flat liste `["eik", "dreid"]` (ingen/eldre profil) → ingen kategorier.
/// - kategori-objekt `{"materiale": ["eik"], "teknikk": ["dreid"]}` → kategori per tag.
///
/// Navn lowercases; kategori strippes men beholder profil-casing. Dedup på navn,
/// første forekomst (og dens kategori) vinner. Som Pythons `isinstance(t, (str, int))`
/// teller strenger, heltall og bool (men ikke flyttall) som navn.
pub fn parse_tags(raw_tags: Option<&Value>) -> (Vec<String>, HashMap<String, String>) {
    fn push(pairs: &mut Vec<(String, Option<String>)>, t: &Value, category: Option<&str>) {
        let s = match t {
            Value::String(s) => s.clone(),
            Value::Number(n) if !n.is_f64() => n.to_string(),
            Value::Bool(_) => py_str(t),
            _ => return,
        };
        let s = py_strip(&s);
        if !s.is_empty() {
            pairs.push((s.to_lowercase(), category.map(str::to_string)));
        }
    }

    let mut pairs: Vec<(String, Option<String>)> = Vec::new();
    match raw_tags {
        Some(Value::Object(map)) => {
            for (cat, names) in map {
                let category = Some(py_strip(cat)).filter(|c| !c.is_empty());
                match names {
                    Value::Array(items) => {
                        for t in items {
                            push(&mut pairs, t, category);
                        }
                    }
                    other => push(&mut pairs, other, category),
                }
            }
        }
        Some(Value::Array(items)) => {
            for t in items {
                push(&mut pairs, t, None);
            }
        }
        _ => {}
    }

    let mut seen = HashSet::new();
    let mut tags = Vec::new();
    let mut tag_categories = HashMap::new();
    for (name, cat) in pairs {
        if seen.insert(name.clone()) {
            if let Some(cat) = cat {
                tag_categories.insert(name.clone(), cat);
            }
            tags.push(name);
        }
    }
    (tags, tag_categories)
}

/// Profilens `categories`-JSON → liste (Pythons `str(c).strip()` per element).
/// Tom/ugyldig/ikke-liste → `[]`.
pub fn parse_categories(raw: Option<&str>) -> Vec<String> {
    let Some(raw) = raw.filter(|s| !s.is_empty()) else {
        return Vec::new();
    };
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Array(items)) => items
            .iter()
            .map(|c| py_strip(&py_str(c)).to_string())
            .filter(|c| !c.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// Plukk faktisk kost (USD) fra OpenRouter-svarets `usage.cost`. `None` hvis ukjent.
pub fn extract_cost(raw: &Value) -> Option<f64> {
    let cost = raw.as_object()?.get("usage")?.as_object()?.get("cost")?;
    match cost {
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::Number(n) => n.as_f64(),
        Value::String(s) => py_float(s),
        _ => None,
    }
}

/// Bygg den aktive leverandøren fra settings. Feiler tydelig hvis nøkkelen mangler.
pub fn make_provider(conn: &Connection) -> Result<OpenRouterProvider> {
    let api_key = settings::get_setting(conn, "openrouter_api_key")?
        .filter(|k| !k.is_empty())
        .ok_or_else(|| Error::Tagging(MISSING_KEY.into()))?;
    let model = settings::get_setting(conn, "ai_model")?
        .filter(|m| !m.is_empty())
        .or_else(|| settings::default_for("ai_model").map(str::to_string))
        .unwrap_or_default();
    Ok(OpenRouterProvider::new(&api_key, &model))
}

// --- Steg-kjøring -----------------------------------------------------------

/// Valg for én kjøring (CLI-flaggene + `source_id` fra API-et).
#[derive(Debug, Clone, Default)]
pub struct TagOptions {
    /// Maks antall bilder; `None`/0 = alle pending.
    pub limit: Option<i64>,
    /// Antall parallelle tråder; `None`/0 = `DEFAULT_WORKERS`.
    pub workers: Option<i64>,
    /// Nullstill ai-state og merk på nytt.
    pub retag: bool,
    /// Hopp over kostnadsgrensen.
    pub assume_yes: bool,
    /// Begrens til bilder med en fil i denne kilden.
    pub source_id: Option<i64>,
}

/// Svar på «Fortsett? [j/N]»: `j/ja/y/yes` (trimmet, uansett case) fortsetter.
pub fn is_yes(answer: &str) -> bool {
    matches!(
        py_strip(answer).to_lowercase().as_str(),
        "j" | "ja" | "y" | "yes"
    )
}

#[derive(Debug, Clone, PartialEq)]
struct Candidate {
    image_id: i64,
    content_hash: String,
    focus: Option<String>,
    categories: Option<String>,
}

/// Tekstverdi fra en kolonne (tall gjøres om til tekst, BLOB/NULL → `None`).
fn text_column(v: ValueRef<'_>) -> Option<String> {
    match v {
        ValueRef::Text(t) => Some(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Integer(i) => Some(i.to_string()),
        ValueRef::Real(f) => Some(pyjson::float_repr(f)),
        ValueRef::Null | ValueRef::Blob(_) => None,
    }
}

fn candidate_rows(
    conn: &Connection,
    limit: Option<i64>,
    source_id: Option<i64>,
) -> Result<Vec<Candidate>> {
    // Effektiv profil per bilde: kildens profile_id (dedup: ikke-tom, ellers laveste
    // source_id) → globalt default_profile_id → ingen. Gjenbrukt for focus + categories.
    let eff_pid = "COALESCE(
                   (SELECT s.profile_id FROM files f
                      JOIN sources s ON s.id = f.source_id
                     WHERE f.image_id = i.id AND s.profile_id IS NOT NULL
                     ORDER BY f.source_id ASC LIMIT 1),
                   (SELECT CAST(value AS INTEGER) FROM settings WHERE key = 'default_profile_id')
               )";
    let mut sql = format!(
        "
        SELECT i.id AS image_id, i.content_hash AS content_hash,
               (SELECT p.focus      FROM tagging_profiles p WHERE p.id = {eff_pid}) AS focus,
               (SELECT p.categories FROM tagging_profiles p WHERE p.id = {eff_pid}) AS categories
        FROM images i
        JOIN processing_state pa ON pa.image_id = i.id AND pa.stage = 'ai'     AND pa.status = 'pending'
        JOIN processing_state pd ON pd.image_id = i.id AND pd.stage = 'derive' AND pd.status = 'done'
        JOIN derived d           ON d.image_id  = i.id AND d.size = '1024'
    "
    );
    let mut params: Vec<i64> = Vec::new();
    if let Some(sid) = source_id {
        sql.push_str(
            "        AND EXISTS (SELECT 1 FROM files f WHERE f.image_id = i.id AND f.source_id = ?)\n",
        );
        params.push(sid);
    }
    sql.push_str("        ORDER BY i.id");
    // 0 / None = alle pending.
    if let Some(limit) = limit.filter(|l| *l != 0) {
        sql.push_str("\n        LIMIT ?");
        params.push(limit);
    }
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(params), |r| {
        Ok(Candidate {
            image_id: r.get("image_id")?,
            content_hash: r.get("content_hash")?,
            focus: text_column(r.get_ref("focus")?),
            categories: text_column(r.get_ref("categories")?),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Nullstill ai-state til pending for bilder med ferdig derive (re-tagging).
///
/// `source_id` begrenser nullstillingen til den kilden, så en per-mappe retag ikke
/// nullstiller andre mappers ai-state.
fn reset_for_retag(conn: &Connection, source_id: Option<i64>) -> Result<()> {
    let mut sql = String::from(
        "
        UPDATE processing_state SET status = 'pending', error = NULL,
               updated_at = datetime('now')
        WHERE stage = 'ai' AND status IN ('done', 'error')
          AND image_id IN (
              SELECT image_id FROM processing_state WHERE stage = 'derive' AND status = 'done'
          )
    ",
    );
    let mut params: Vec<i64> = Vec::new();
    if let Some(sid) = source_id {
        sql.push_str(" AND image_id IN (SELECT image_id FROM files WHERE source_id = ?)");
        params.push(sid);
    }
    conn.execute(&sql, params_from_iter(params))?;
    Ok(())
}

/// Én jobb for en arbeidstråd: arbeidskopi-sti + resolvert profil.
struct Job {
    image_id: i64,
    path: PathBuf,
    focus: Option<String>,
    categories: Vec<String>,
}

/// Merk pending bilder via OpenRouter → `ai_tags` + `tags`/`image_tags`.
///
/// Idempotent: `done`-bilder hoppes over. `retag` nullstiller ai-state først.
/// Per-bilde-feil → `status='error'`, jobben fortsetter. `source_id` begrenser til
/// bilder med en fil i den kilden (per-mappe-kjøring). Over `CONFIRM_THRESHOLD`
/// bilder uten `assume_yes` kalles `confirm` med advarselen; `false` avbryter.
pub fn tag(
    conn: &Connection,
    storage: &Storage,
    opts: &TagOptions,
    confirm: &mut dyn FnMut(&str) -> bool,
) -> Result<TagStats> {
    let provider = make_provider(conn)?; // feiler tidlig hvis nøkkelen mangler
    tag_with_provider(conn, storage, &provider, opts, confirm)
}

/// Som `tag`, men med en gitt leverandør (lokal modell, tester).
pub fn tag_with_provider(
    conn: &Connection,
    storage: &Storage,
    provider: &dyn VisionProvider,
    opts: &TagOptions,
    confirm: &mut dyn FnMut(&str) -> bool,
) -> Result<TagStats> {
    if opts.retag {
        reset_for_retag(conn, opts.source_id)?;
    }

    let rows = candidate_rows(conn, opts.limit, opts.source_id)?;
    let mut stats = TagStats {
        pending: rows.len(),
        ..TagStats::default()
    };
    if rows.is_empty() {
        return Ok(stats);
    }

    // Kostnadsgrense: dette treffer et betalt API.
    if rows.len() > CONFIRM_THRESHOLD && !opts.assume_yes {
        let warning = format!(
            "Skal merke {} bilder via {} — dette treffer et betalt API og koster ekte penger \
             (per-bilde-pris varierer med modell).",
            rows.len(),
            provider.name()
        );
        if !confirm(&warning) {
            stats.skipped = true;
            return Ok(stats);
        }
    }

    let workers = match opts.workers {
        None | Some(0) => DEFAULT_WORKERS,
        Some(n) if n > 0 => usize::try_from(n).unwrap_or(usize::MAX),
        Some(_) => {
            return Err(Error::Tagging("max_workers must be greater than 0".into()));
        }
    };

    // Per bilde: arbeidskopi-sti + resolvert profil (fokus + kategorier), så en global
    // «Oppdater alt» over flere kilder respekterer hvert bildes profil.
    let jobs: Vec<Job> = rows
        .into_iter()
        .map(|r| Job {
            image_id: r.image_id,
            path: storage.derived_path(&r.content_hash, "1024"),
            categories: parse_categories(r.categories.as_deref()),
            focus: r.focus,
        })
        .collect();

    run_pool(conn, provider, &jobs, workers, &mut stats)?;
    Ok(stats)
}

/// Arbeidstrådene tar jobber i rekkefølge og gjør bare nettverkskall; hovedtråden
/// skriver hvert resultat til DB-en (og committer) etter hvert som det kommer inn.
fn run_pool(
    conn: &Connection,
    provider: &dyn VisionProvider,
    jobs: &[Job],
    workers: usize,
    stats: &mut TagStats,
) -> Result<()> {
    let next = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let (tx, rx) = mpsc::channel::<(i64, Result<VisionResult, ImageError>)>();
    std::thread::scope(|scope| {
        for _ in 0..workers.min(jobs.len()) {
            let tx = tx.clone();
            let (next, stop) = (&next, &stop);
            scope.spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let Some(job) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) else {
                        break;
                    };
                    let result = tag_one(job, provider);
                    if tx.send((job.image_id, result)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);

        let mut outcome = Ok(());
        let mut cancelled = false;
        for (image_id, result) in rx {
            if outcome.is_err() {
                continue; // fatal DB-feil: la trådene bli ferdige, ingen nye kall
            }
            let written = match result {
                Ok(res) => persist(conn, image_id, provider.name(), &res).map(|()| stats.done += 1),
                // Nettverk ga opp etter retry, manglende fil, parse-feil …
                Err(e) => set_state(conn, image_id, "error", Some(&e.to_string()))
                    .map(|()| stats.errors += 1),
            };
            if let Err(e) = written {
                stop.store(true, Ordering::Relaxed);
                outcome = Err(e);
            } else if cancel::requested() {
                // Trygt punkt: ingen nye kall, men svar som kommer (og er betalt for)
                // skrives fortsatt. Resten står som pending til neste kjøring.
                stop.store(true, Ordering::Relaxed);
                cancelled = true;
            }
        }
        if cancelled && outcome.is_ok() {
            outcome = Err(Error::Cancelled);
        }
        outcome
    })
}

/// Trådarbeider: ren av DB. Kaller leverandøren på 1024-derivatet med resolvert profil.
fn tag_one(job: &Job, provider: &dyn VisionProvider) -> Result<VisionResult, ImageError> {
    if !job.path.exists() {
        let name = job
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        return Err(ImageError::new(
            "TaggingError",
            format!("mangler 1024-derivat: {name}"),
        ));
    }
    provider.tag(&job.path, job.focus.as_deref(), &job.categories)
}

/// Skriv `ai_tags` + `tags`/`image_tags` + sett ai-state `done`. Erstatter ev. eldre rad.
fn persist(
    conn: &Connection,
    image_id: i64,
    provider_name: &str,
    result: &VisionResult,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM ai_tags WHERE image_id = ?", [image_id])?;
    tx.execute("DELETE FROM image_tags WHERE image_id = ?", [image_id])?;
    tx.execute(
        "INSERT INTO ai_tags (image_id, provider, scene, event_type, season_ai, \
         indoor_outdoor, person_count, description, raw_json, cost) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            image_id,
            provider_name,
            result.scene,
            result.event_type,
            result.season,
            result.indoor_outdoor,
            result.person_count,
            result.description,
            pyjson::dumps(&result.raw),
            extract_cost(&result.raw),
        ],
    )?;
    let mut seen = HashSet::new();
    for name in &result.tags {
        if !seen.insert(name) {
            continue; // dedup, bevar rekkefølge
        }
        // Kategori er global per tag, sist ikke-null vinner (COALESCE) — en senere
        // flat merking (kategori=NULL) visker ikke ut en allerede satt kategori.
        tx.execute(
            "INSERT INTO tags (name, category) VALUES (?, ?) \
             ON CONFLICT(name) DO UPDATE SET category = COALESCE(excluded.category, tags.category)",
            params![name, result.tag_categories.get(name)],
        )?;
        let tag_id: i64 =
            tx.query_row("SELECT id FROM tags WHERE name = ?", [name], |r| r.get(0))?;
        tx.execute(
            "INSERT INTO image_tags (image_id, tag_id) VALUES (?, ?) \
             ON CONFLICT(image_id, tag_id) DO NOTHING",
            params![image_id, tag_id],
        )?;
    }
    set_state(&tx, image_id, "done", None)?;
    tx.commit()?;
    Ok(())
}

fn set_state(conn: &Connection, image_id: i64, status: &str, error: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE processing_state SET status = ?, error = ?, updated_at = datetime('now') \
         WHERE image_id = ? AND stage = 'ai'",
        params![status, error, image_id],
    )?;
    Ok(())
}

// --- Python-semantikk --------------------------------------------------------
//
// Svarene fra modellen er vilkårlig JSON. Python-versjonen bruker `str(v)`,
// `int(v)`, `float(v)`, `v[k]` og `json.loads` direkte på verdiene; funksjonene under
// gjenskaper det de gir (inkludert feiltekstene), så databasen blir den samme.

/// Pythons `str.strip()`: Unicode-mellomrom pluss `\x1c`–`\x1f`.
fn py_is_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

fn py_strip(s: &str) -> &str {
    s.trim_matches(py_is_space)
}

fn py_lstrip(s: &str) -> &str {
    s.trim_start_matches(py_is_space)
}

/// `type(v).__name__` for en JSON-verdi slik `json.loads` lager den.
fn py_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// `str(v)`.
fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => py_repr(other),
    }
}

/// `repr(v)`.
fn py_repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => py_number(n),
        Value::String(s) => py_repr_str(s),
        Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(py_repr).collect();
            format!("[{}]", inner.join(", "))
        }
        Value::Object(map) => {
            let inner: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{}: {}", py_repr_str(k), py_repr(v)))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
    }
}

fn py_number(n: &Number) -> String {
    match n.as_f64() {
        Some(f) if n.is_f64() => pyjson::float_repr(f),
        _ => n.to_string(),
    }
}

/// Tilnærmet `str.isprintable()` for ikke-ASCII: kontrolltegn, separatorer,
/// formattegn og privat bruk er ikke skrivbare.
fn py_isprintable(c: char) -> bool {
    !c.is_control()
        && !matches!(c as u32,
            0xA0 | 0xAD | 0x600..=0x605 | 0x61C | 0x6DD | 0x70F | 0x890..=0x891 | 0x8E2
            | 0x1680 | 0x180E | 0x2000..=0x200F | 0x2028..=0x202F | 0x205F..=0x206F
            | 0x3000 | 0xE000..=0xF8FF | 0xFEFF | 0xFFF9..=0xFFFB | 0xFFFE..=0xFFFF
            | 0xE0001 | 0xE0020..=0xE007F | 0xF0000..)
}

/// `repr(s)` for en streng: enkle anførselstegn med mindre strengen har `'` og ikke `"`.
fn py_repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        let u = c as u32;
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            _ if u < 0x20 || u == 0x7f => out.push_str(&format!("\\x{u:02x}")),
            c if c.is_ascii() || py_isprintable(c) => out.push(c),
            _ if u <= 0xff => out.push_str(&format!("\\x{u:02x}")),
            _ if u <= 0xffff => out.push_str(&format!("\\u{u:04x}")),
            _ => out.push_str(&format!("\\U{u:08x}")),
        }
    }
    out.push(quote);
    out
}

/// Fjern understreker mellom sifre (`1_000`), som Pythons `int()`/`float()` tillater.
/// `None` hvis en understrek står et annet sted.
fn py_underscores(s: &str) -> Option<String> {
    let chars: Vec<char> = s.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if *c == '_' {
            let before = i > 0 && chars[i - 1].is_ascii_digit();
            let after = chars.get(i + 1).is_some_and(|n| n.is_ascii_digit());
            if !(before && after) {
                return None;
            }
        }
    }
    Some(chars.into_iter().filter(|c| *c != '_').collect())
}

/// Pythons `int(s)` for en streng (base 10). `None` ved `ValueError` eller verdier
/// utenfor SQLite INTEGER.
fn py_int(s: &str) -> Option<i64> {
    let t = py_strip(s);
    let (negative, digits) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit() || c == '_') {
        return None;
    }
    let digits = py_underscores(digits)?;
    let value: i128 = digits.parse().ok()?;
    i64::try_from(if negative { -value } else { value }).ok()
}

/// Pythons `float(s)` for en streng.
fn py_float(s: &str) -> Option<f64> {
    let t = py_strip(s);
    let t = if t.contains('_') {
        py_underscores(t)?
    } else {
        t.to_string()
    };
    t.parse().ok()
}

enum Key<'a> {
    Str(&'a str),
    Index(usize),
}

/// `v[key]` med Pythons feiltyper og -tekster.
fn py_getitem(v: &Value, key: Key<'_>) -> Result<Value, ImageError> {
    match (v, key) {
        (Value::Object(map), Key::Str(k)) => map
            .get(k)
            .cloned()
            .ok_or_else(|| ImageError::new("KeyError", py_repr_str(k))),
        (Value::Object(_), Key::Index(i)) => Err(ImageError::new("KeyError", i.to_string())),
        (Value::Array(_), Key::Str(_)) => Err(ImageError::new(
            "TypeError",
            "list indices must be integers or slices, not str",
        )),
        (Value::Array(items), Key::Index(i)) => items
            .get(i)
            .cloned()
            .ok_or_else(|| ImageError::new("IndexError", "list index out of range")),
        (Value::String(_), Key::Str(_)) => Err(ImageError::new(
            "TypeError",
            "string indices must be integers, not 'str'",
        )),
        (Value::String(s), Key::Index(i)) => s
            .chars()
            .nth(i)
            .map(|c| Value::String(c.to_string()))
            .ok_or_else(|| ImageError::new("IndexError", "string index out of range")),
        (other, _) => Err(ImageError::new(
            "TypeError",
            format!("'{}' object is not subscriptable", py_type(other)),
        )),
    }
}

/// `json.loads(text)`: serde_json gjør selve parsingen; ved feil gjenskapes Pythons
/// `JSONDecodeError`-tekst.
fn py_json_loads(text: &str) -> Result<Value, ImageError> {
    serde_json::from_str(text).map_err(|e| {
        ImageError::new(
            "JSONDecodeError",
            python_json_error(text).unwrap_or_else(|| e.to_string()),
        )
    })
}

enum JsonFail {
    /// `StopIteration(pos)` → «Expecting value».
    Stop(usize),
    Msg(&'static str, usize),
}

/// Grense for nøsting i feilsøket (serde_json gir opp ved 128 nivåer uansett).
const JSON_MAX_DEPTH: usize = 512;

/// Feilteksten Pythons `json.loads(text)` gir, eller `None` hvis Python godtar teksten
/// (f.eks. `NaN`, som serde_json avviser). Følger C-skanneren i CPython (`_json.c`):
/// samme meldinger og samme posisjoner (i tegn, ikke byte).
pub(crate) fn python_json_error(text: &str) -> Option<String> {
    let s: Vec<char> = text.chars().collect();
    let fail = |msg: &str, pos: usize| Some(format_json_error(&s, msg, pos));
    if s.first() == Some(&'\u{feff}') {
        return fail("Unexpected UTF-8 BOM (decode using utf-8-sig)", 0);
    }
    match scan_once(&s, skip_ws(&s, 0), 0) {
        Err(JsonFail::Stop(pos)) => fail("Expecting value", pos),
        Err(JsonFail::Msg(msg, pos)) => fail(msg, pos),
        Ok(end) => {
            let end = skip_ws(&s, end);
            if end != s.len() {
                fail("Extra data", end)
            } else {
                None
            }
        }
    }
}

fn format_json_error(s: &[char], msg: &str, pos: usize) -> String {
    let before = &s[..pos.min(s.len())];
    let lineno = before.iter().filter(|c| **c == '\n').count() + 1;
    let colno = match before.iter().rposition(|c| *c == '\n') {
        Some(nl) => pos - nl,
        None => pos + 1,
    };
    format!("{msg}: line {lineno} column {colno} (char {pos})")
}

fn skip_ws(s: &[char], mut i: usize) -> usize {
    while i < s.len() && matches!(s[i], ' ' | '\t' | '\n' | '\r') {
        i += 1;
    }
    i
}

fn scan_once(s: &[char], idx: usize, depth: usize) -> Result<usize, JsonFail> {
    if idx >= s.len() {
        return Err(JsonFail::Stop(idx));
    }
    if depth > JSON_MAX_DEPTH {
        // Python ville gitt RecursionError; vi har ingen bedre tekst enn serde sin.
        return Ok(s.len());
    }
    let starts = |word: &str| {
        let mut i = idx;
        word.chars().all(|w| {
            let ok = s.get(i) == Some(&w);
            i += 1;
            ok
        })
    };
    match s[idx] {
        '"' => return scan_string(s, idx + 1),
        '{' => return scan_object(s, idx + 1, depth + 1),
        '[' => return scan_array(s, idx + 1, depth + 1),
        'n' if starts("null") => return Ok(idx + 4),
        't' if starts("true") => return Ok(idx + 4),
        'f' if starts("false") => return Ok(idx + 5),
        'N' if starts("NaN") => return Ok(idx + 3),
        'I' if starts("Infinity") => return Ok(idx + 8),
        '-' if starts("-Infinity") => return Ok(idx + 9),
        _ => {}
    }
    scan_number(s, idx)
}

fn scan_number(s: &[char], start: usize) -> Result<usize, JsonFail> {
    let end_idx = s.len() - 1;
    let digit = |i: usize| s[i].is_ascii_digit();
    let mut idx = start;
    if s[idx] == '-' {
        idx += 1;
        if idx > end_idx {
            return Err(JsonFail::Stop(start));
        }
    }
    if ('1'..='9').contains(&s[idx]) {
        idx += 1;
        while idx <= end_idx && digit(idx) {
            idx += 1;
        }
    } else if s[idx] == '0' {
        idx += 1;
    } else {
        return Err(JsonFail::Stop(start));
    }
    if idx < end_idx && s[idx] == '.' && digit(idx + 1) {
        idx += 2;
        while idx <= end_idx && digit(idx) {
            idx += 1;
        }
    }
    if idx < end_idx && (s[idx] == 'e' || s[idx] == 'E') {
        let e_start = idx;
        idx += 1;
        if idx < end_idx && (s[idx] == '-' || s[idx] == '+') {
            idx += 1;
        }
        while idx <= end_idx && digit(idx) {
            idx += 1;
        }
        if !digit(idx - 1) {
            idx = e_start;
        }
    }
    Ok(idx)
}

/// Strengen starter rett etter `"` på `end`. Returnerer posisjonen etter avsluttende `"`.
fn scan_string(s: &[char], mut end: usize) -> Result<usize, JsonFail> {
    let begin = end - 1;
    let len = s.len();
    loop {
        let mut next = end;
        let mut found = None;
        while next < len {
            let c = s[next];
            if c == '"' || c == '\\' {
                found = Some(c);
                break;
            }
            if (c as u32) <= 0x1f {
                return Err(JsonFail::Msg("Invalid control character at", next));
            }
            next += 1;
        }
        let Some(c) = found else {
            return Err(JsonFail::Msg("Unterminated string starting at", begin));
        };
        next += 1;
        if c == '"' {
            return Ok(next);
        }
        if next == len {
            return Err(JsonFail::Msg("Unterminated string starting at", begin));
        }
        if s[next] != 'u' {
            end = next + 1;
            if !matches!(s[next], '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't') {
                return Err(JsonFail::Msg("Invalid \\escape", end - 2));
            }
        } else {
            next += 1;
            end = next + 4;
            if end > len {
                return Err(JsonFail::Msg("Invalid \\uXXXX escape", next - 1));
            }
            if !s[next..end].iter().all(char::is_ascii_hexdigit) {
                return Err(JsonFail::Msg("Invalid \\uXXXX escape", end - 5));
            }
            // Et påfølgende `\uXXXX` (surrogatpar) gir samme feilposisjon om det
            // sjekkes her eller i neste runde, så det trengs ingen egen behandling.
        }
    }
}

fn scan_object(s: &[char], idx: usize, depth: usize) -> Result<usize, JsonFail> {
    let at = |i: usize, c: char| i < s.len() && s[i] == c;
    let mut idx = skip_ws(s, idx);
    if !at(idx, '}') {
        loop {
            if !at(idx, '"') {
                return Err(JsonFail::Msg(
                    "Expecting property name enclosed in double quotes",
                    idx,
                ));
            }
            idx = skip_ws(s, scan_string(s, idx + 1)?);
            if !at(idx, ':') {
                return Err(JsonFail::Msg("Expecting ':' delimiter", idx));
            }
            idx = skip_ws(s, idx + 1);
            idx = skip_ws(s, scan_once(s, idx, depth)?);
            if at(idx, '}') {
                break;
            }
            if !at(idx, ',') {
                return Err(JsonFail::Msg("Expecting ',' delimiter", idx));
            }
            let comma = idx;
            idx = skip_ws(s, idx + 1);
            if at(idx, '}') {
                return Err(JsonFail::Msg(
                    "Illegal trailing comma before end of object",
                    comma,
                ));
            }
        }
    }
    Ok(idx + 1)
}

fn scan_array(s: &[char], idx: usize, depth: usize) -> Result<usize, JsonFail> {
    let at = |i: usize, c: char| i < s.len() && s[i] == c;
    let mut idx = skip_ws(s, idx);
    if !at(idx, ']') {
        loop {
            idx = skip_ws(s, scan_once(s, idx, depth)?);
            if at(idx, ']') {
                break;
            }
            if !at(idx, ',') {
                return Err(JsonFail::Msg("Expecting ',' delimiter", idx));
            }
            let comma = idx;
            idx = skip_ws(s, idx + 1);
            if at(idx, ']') {
                return Err(JsonFail::Msg(
                    "Illegal trailing comma before end of array",
                    comma,
                ));
            }
        }
    }
    Ok(idx + 1)
}

#[cfg(test)]
mod tests;
