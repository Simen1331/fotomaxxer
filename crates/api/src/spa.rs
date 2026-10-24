//! Statiske filer: SPA-en (`web/dist`), `/assets` og `/derived` — som `api/app.py`.
//!
//! **Innbygging.** `web/dist` bakes inn i binæren med `rust-embed` ([`WebDist`]). I
//! release-bygg ligger filene i binæren; i debug-bygg leses de fra `web/dist` på disk
//! ved hver forespørsel (så `npm run build` slår igjennom uten å bygge Rust på nytt).
//! Mangler `web/dist` ved bygging, bygger craten likevel (`allow_missing`), og tjeneren
//! oppfører seg som Python uten `web/dist`: ingen SPA, ukjente stier gir 404.
//!
//! **Oppførsel som Starlette** (`StaticFiles` for `/derived` og `/assets`, `FileResponse`
//! for SPA-fallbacken):
//! - Headere: `content-type` fra Pythons `mimetypes` (`text/*` får `; charset=utf-8`,
//!   ukjent → `application/octet-stream`), `accept-ranges: bytes`, `content-length`,
//!   `last-modified` og `etag`. Ingen `cache-control` (Python sender heller ingen).
//! - `/derived` og `/assets` svarer 304 på `If-None-Match`/`If-Modified-Since`, og godtar
//!   `HEAD`. Andre metoder gir 405 uten `allow`, manglende fil 404 `Not Found`.
//! - SPA-fallbacken (`GET /{full_path:path}`): `api`/`derived`-stier gir 404 «ukjent
//!   endepunkt», en eksisterende fil i `dist` serveres, ellers `index.html`. Andre
//!   metoder enn `GET` (også `HEAD`) gir 405 med `allow: GET`.
//!
//! **ETag.** For `/derived` regnes den eksakt som Starlette:
//! `md5(f"{st_mtime}-{st_size}")`, så den er lik mellom Python og Rust. For innebygde
//! filer finnes ingen `st_mtime` med desimaler; der er ETag de 32 første heks-sifrene
//! av filens SHA-256, og `last-modified` er byggetidens filtid (hele sekunder).
//! Range-forespørsler (206) støttes ikke; hele filen sendes alltid.

use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use md5::{Digest, Md5};

use crate::error::ApiError;
use crate::state::AppState;

/// `web/dist`, innebygd i release-binæren.
#[derive(rust_embed::Embed)]
#[folder = "../../web/dist/"]
#[allow_missing = true]
pub struct WebDist;

/// En statisk fil klar til å serveres.
#[derive(Debug, Clone)]
pub struct StaticFile {
    pub data: Cow<'static, [u8]>,
    pub content_type: String,
    pub last_modified: Option<SystemTime>,
    /// Med fnutter, som Starlette: `"…"`.
    pub etag: String,
}

/// Kilde for SPA-filene (innebygd i binæren, eller i minnet i tester).
pub trait SpaSource: Send + Sync {
    /// Filen på relativ sti (`index.html`, `assets/index-….js`).
    fn get(&self, path: &str) -> Option<StaticFile>;
    /// Finnes `assets/`-mappa (Python monterer `/assets` bare da)?
    fn has_assets(&self) -> bool;
}

/// Den innebygde `web/dist`.
pub struct EmbeddedSpa;

impl EmbeddedSpa {
    /// `Some` hvis `web/dist` ble bakt inn (Pythons `dist.is_dir()`).
    pub fn detect() -> Option<Self> {
        WebDist::iter().next().map(|_| Self)
    }
}

impl SpaSource for EmbeddedSpa {
    fn get(&self, path: &str) -> Option<StaticFile> {
        let file = WebDist::get(path)?;
        let hash = file.metadata.sha256_hash();
        let etag = format!(
            "\"{}\"",
            hash[..16]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        Some(StaticFile {
            data: file.data,
            content_type: content_type_for(path),
            last_modified: file
                .metadata
                .last_modified()
                .map(|s| UNIX_EPOCH + Duration::from_secs(s)),
            etag,
        })
    }

    fn has_assets(&self) -> bool {
        WebDist::iter().any(|p| p.starts_with("assets/"))
    }
}

/// SPA i minnet (tester).
pub struct MemorySpa(pub Vec<(String, Vec<u8>)>);

impl SpaSource for MemorySpa {
    fn get(&self, path: &str) -> Option<StaticFile> {
        let (name, data) = self.0.iter().find(|(p, _)| p == path)?;
        Some(StaticFile {
            data: Cow::Owned(data.clone()),
            content_type: content_type_for(name),
            last_modified: Some(UNIX_EPOCH),
            etag: format!("\"{}\"", md5_hex(data)),
        })
    }

    fn has_assets(&self) -> bool {
        self.0.iter().any(|(p, _)| p.starts_with("assets/"))
    }
}

/// `mimetypes.guess_type` (Python 3.14) for filtypene en Vite-build og `derived/`
/// inneholder, med Starlettes `; charset=utf-8` på `text/*`.
pub fn content_type_for(path: &str) -> String {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let mime = match ext.as_str() {
        "html" | "htm" => "text/html",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "txt" => "text/plain",
        "xml" => "text/xml",
        "csv" => "text/csv",
        "json" => "application/json",
        "webmanifest" => "application/manifest+json",
        "wasm" => "application/wasm",
        "pdf" => "application/pdf",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "heic" => "image/heic",
        "ico" => "image/vnd.microsoft.icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        _ => "application/octet-stream",
    };
    if mime.starts_with("text/") {
        format!("{mime}; charset=utf-8")
    } else {
        mime.to_string()
    }
}

fn md5_hex(data: &[u8]) -> String {
    Md5::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Starlettes ETag for en fil på disk: `md5(str(st_mtime) + "-" + str(st_size))`.
pub fn starlette_etag(mtime: f64, size: u64) -> String {
    let base = format!("{}-{size}", fotomaxxer_core::pyjson::float_repr(mtime));
    format!("\"{}\"", md5_hex(base.as_bytes()))
}

/// Les en fil fra `derived/` som Starlette (`None` = finnes ikke / ikke en vanlig fil).
async fn disk_file(path: PathBuf) -> Option<StaticFile> {
    let meta = tokio::fs::metadata(&path).await.ok()?;
    if !meta.is_file() {
        return None;
    }
    let data = tokio::fs::read(&path).await.ok()?;
    let mtime = fotomaxxer_core::scan::mtime_float(&meta);
    Some(StaticFile {
        etag: starlette_etag(mtime, meta.len()),
        last_modified: meta.modified().ok(),
        content_type: content_type_for(&path.to_string_lossy()),
        data: Cow::Owned(data),
    })
}

/// Bygg svaret: alle headere, og tom kropp for `HEAD`.
fn file_response(file: StaticFile, head: bool) -> Response {
    let mut headers = HeaderMap::new();
    let mut set = |name: header::HeaderName, value: &str| {
        if let Ok(v) = HeaderValue::from_str(value) {
            headers.insert(name, v);
        }
    };
    set(header::CONTENT_TYPE, &file.content_type);
    set(header::ACCEPT_RANGES, "bytes");
    set(header::CONTENT_LENGTH, &file.data.len().to_string());
    if let Some(lm) = file.last_modified {
        set(header::LAST_MODIFIED, &httpdate::fmt_http_date(lm));
    }
    set(header::ETAG, &file.etag);
    let body = if head {
        Body::empty()
    } else {
        Body::from(file.data.into_owned())
    };
    (StatusCode::OK, headers, body).into_response()
}

/// `StaticFiles.is_not_modified`.
fn is_not_modified(file: &StaticFile, req: &HeaderMap) -> bool {
    if let Some(inm) = req.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) {
        return inm
            .split(',')
            .map(|t| t.trim().trim_start_matches("W/"))
            .any(|t| t == file.etag);
    }
    let ims = req
        .get(header::IF_MODIFIED_SINCE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| httpdate::parse_http_date(v).ok());
    match (ims, file.last_modified) {
        // Last-Modified sendes med hele sekunder; sammenlign på samme oppløsning.
        (Some(ims), Some(lm)) => {
            let lm = httpdate::parse_http_date(&httpdate::fmt_http_date(lm)).unwrap_or(lm);
            ims >= lm
        }
        _ => false,
    }
}

/// `StaticFiles`: 304, `HEAD` og 405 for andre metoder.
fn static_response(file: Option<StaticFile>, method: &Method, req: &HeaderMap) -> Response {
    let Some(file) = file else {
        return ApiError::not_found().into_response();
    };
    if is_not_modified(&file, req) {
        let mut resp = StatusCode::NOT_MODIFIED.into_response();
        if let Ok(v) = HeaderValue::from_str(&file.etag) {
            resp.headers_mut().insert(header::ETAG, v);
        }
        return resp;
    }
    file_response(file, method == Method::HEAD)
}

/// Løs `rel` mot en virtuell mappe `base` (komponentene til `web/dist/…`) slik
/// `realpath(join(base, rel))` ville gjort, og gi stien relativt til `root_len` første
/// komponenter av `base`. `None` hvis resultatet havner utenfor `base` (Starlette
/// sjekker `commonpath`, og `resolve().is_relative_to(dist)` for SPA-fallbacken).
fn resolve_within(base: &[&str], root_len: usize, rel: &str) -> Option<String> {
    let mut parts: Vec<String> = base.iter().map(|s| s.to_string()).collect();
    for comp in Path::new(rel).components() {
        match comp {
            Component::Normal(c) => parts.push(c.to_str()?.to_string()),
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
            Component::ParentDir => {
                parts.pop()?;
            }
        }
    }
    if parts.len() < base.len() || parts[..base.len()] != base[..] {
        return None;
    }
    Some(parts[root_len..].join("/"))
}

/// Prosentdekod en sti som uvicorn gjør før Starlette ser den.
pub fn percent_decode(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            )
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Stien etter et monteringsprefiks (`/derived/…`, `/assets/…`), dekodet.
fn mounted_path(req: &Request, prefix: &str) -> String {
    let decoded = percent_decode(req.uri().path());
    decoded
        .strip_prefix(prefix)
        .unwrap_or("")
        .trim_start_matches('/')
        .to_string()
}

fn static_method_ok(method: &Method) -> bool {
    method == Method::GET || method == Method::HEAD
}

/// `GET|HEAD /derived/<fil>` fra datamappa slik den var ved oppstart.
pub async fn derived(State(state): State<AppState>, req: Request) -> Response {
    if !static_method_ok(req.method()) {
        return ApiError::method_not_allowed(None).into_response();
    }
    let rel = mounted_path(&req, "/derived");
    let root = state.derived_dir();
    // `realpath(join(derived, rel))` må ligge i `derived/` (også via `..` og symlenker).
    let file = match (canonical(&root.join(&rel)), canonical(root)) {
        (Some(f), Some(r)) if f.starts_with(&r) && f != r => disk_file(f).await,
        _ => None,
    };
    static_response(file, req.method(), req.headers())
}

fn canonical(p: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(p).ok()
}

/// `GET|HEAD /assets/<fil>` fra den innebygde `web/dist/assets`.
pub async fn assets(State(state): State<AppState>, req: Request) -> Response {
    if !static_method_ok(req.method()) {
        return ApiError::method_not_allowed(None).into_response();
    }
    let rel = mounted_path(&req, "/assets");
    let file = match (
        resolve_within(&["web", "dist", "assets"], 2, &rel),
        state.spa(),
    ) {
        (Some(rel), Some(spa)) if rel != "assets" => spa.get(&rel),
        _ => None,
    };
    static_response(file, req.method(), req.headers())
}

/// Alt som ikke traff en rute: SPA-fallbacken når `web/dist` finnes, ellers 404.
pub async fn fallback(State(state): State<AppState>, req: Request) -> Response {
    let Some(spa) = state.spa() else {
        return ApiError::not_found().into_response();
    };
    if req.method() != Method::GET {
        return ApiError::method_not_allowed(Some("GET")).into_response();
    }
    let full_path = percent_decode(req.uri().path());
    let full_path = full_path.strip_prefix('/').unwrap_or(&full_path);
    if matches!(full_path, "api" | "derived")
        || full_path.starts_with("api/")
        || full_path.starts_with("derived/")
    {
        return ApiError::http(404, "not_found", "ukjent endepunkt").into_response();
    }
    if !full_path.is_empty()
        && let Some(rel) = resolve_within(&["web", "dist"], 2, full_path)
        && let Some(file) = spa.get(&rel)
    {
        return file_response(file, false);
    }
    match spa.get("index.html") {
        Some(index) => file_response(index, false),
        None => ApiError::http(404, "not_found", "SPA ikke bygget").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_som_pythons_mimetypes() {
        assert_eq!(content_type_for("index.html"), "text/html; charset=utf-8");
        assert_eq!(
            content_type_for("assets/index-BJACLs9J.js"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(content_type_for("a.css"), "text/css; charset=utf-8");
        assert_eq!(content_type_for("abc_300.jpg"), "image/jpeg");
        assert_eq!(content_type_for("x.ico"), "image/vnd.microsoft.icon");
        assert_eq!(content_type_for("x.map"), "application/octet-stream");
    }

    #[test]
    fn etag_som_starlette() {
        // Fasit: hashlib.md5(b"1791363313.5466635-799").hexdigest()
        assert_eq!(
            starlette_etag(1_791_363_313.546_663_5, 799),
            format!("\"{}\"", md5_hex(b"1791363313.5466635-799"))
        );
        assert_eq!(
            starlette_etag(1_700_000_000.0, 5),
            format!("\"{}\"", md5_hex(b"1700000000.0-5"))
        );
    }

    #[test]
    fn stier_kan_ikke_rømme() {
        let assets = ["web", "dist", "assets"];
        assert_eq!(
            resolve_within(&assets, 2, "a/./b").as_deref(),
            Some("assets/a/b")
        );
        assert_eq!(
            resolve_within(&assets, 2, "a/../b").as_deref(),
            Some("assets/b")
        );
        assert_eq!(
            resolve_within(&assets, 2, "../assets/x.css").as_deref(),
            Some("assets/x.css")
        );
        assert_eq!(resolve_within(&assets, 2, "../index.html"), None);
        let dist = ["web", "dist"];
        assert_eq!(
            resolve_within(&dist, 2, "../dist/index.html").as_deref(),
            Some("index.html")
        );
        assert_eq!(resolve_within(&dist, 2, "../../x"), None);
        assert_eq!(percent_decode("/a%20b/%C3%A6%2"), "/a b/æ%2");
    }
}
