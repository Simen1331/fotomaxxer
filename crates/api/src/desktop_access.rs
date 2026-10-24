//! Tilgangsport for den interne desktop-tjeneren — motparten til `api/desktop_access.py`.
//!
//! Brukes bare når appen bygges med [`crate::state::DesktopConfig`] (desktop-modus);
//! `serve` bruker den ikke. En forespørsel slippes gjennom når
//!
//! - `X-FotoMaxxer-Session` **eller** sesjons-cookien ([`cookie_name`]) er lik
//!   sesjonstokenet (konstant-tids sammenligning),
//! - `Host` er lik origin uten `http://`,
//! - `Origin` mangler eller er lik origin, og
//! - `Sec-Fetch-Site` mangler eller er `none`/`same-origin`.
//!
//! Ellers 403. Skrivende metoder (alt utenom `GET`/`HEAD`/`OPTIONS`) får 503 mens
//! tjeneren avslutter, og `POST /api/maintenance/data-dir` gir alltid 409 i desktop-modus.
//! Svarene har formen `{"detail": {"error": "desktop_access", "message": …}}`.
//! Websocket-oppgraderinger støttes ikke av tjeneren og behandles som vanlige forespørsler.
//!
//! **Cookie i stedet for header (Rust).** Python la headeren på hver forespørsel med en
//! Qt-interceptor. wry kan ikke legge headere på underforespørsler, så vinduet (og
//! nettleseren i reservemodus) åpner først [`SESSION_PATH`]`?code=<engangskode>`. Med en
//! gyldig engangskode ([`DesktopConfig::add_launch_code`]) svarer tjeneren `303` til `/`
//! med `Set-Cookie: <navn>=<token>; HttpOnly; SameSite=Strict; Path=/`. Koden kan bare
//! brukes én gang. Host- og `Sec-Fetch-Site`-kravene gjelder også her; ellers 403.

use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::error::ApiError;
use crate::state::{AppState, DesktopConfig};

pub const SESSION_HEADER: &str = "x-fotomaxxer-session";

/// Stien som bytter en engangskode mot sesjons-cookien.
pub const SESSION_PATH: &str = "/__fotomaxxer/session";

/// Navnet på sesjons-cookien. Porten er med fordi cookies ikke skiller på port, så to
/// instanser (to datamapper) i samme nettleser ikke overskriver hverandres cookie.
pub fn cookie_name(origin: &str) -> String {
    let port = origin.rsplit(':').next().unwrap_or("");
    format!("fotomaxxer_session_{port}")
}

/// Verdien av cookien `name` i `Cookie`-headerne (siste treff vinner).
fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a [u8]> {
    let mut found = None;
    for value in headers.get_all("cookie") {
        for pair in value.as_bytes().split(|&b| b == b';') {
            let pair = pair.trim_ascii();
            if let Some(v) = pair
                .strip_prefix(name.as_bytes())
                .and_then(|rest| rest.strip_prefix(b"="))
            {
                found = Some(v);
            }
        }
    }
    found
}

/// Siste verdi av en header som bytes (Python bygger en `dict` av headerlisten).
fn last<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a [u8]> {
    headers
        .get_all(name)
        .iter()
        .next_back()
        .map(|v| v.as_bytes())
}

/// `hmac.compare_digest`: lik lengde og likt innhold, uten tidlig avbrudd.
fn compare_digest(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Host-, Origin- og `Sec-Fetch-Site`-kravene (uten tokenet).
fn same_origin(headers: &HeaderMap, origin: &str) -> bool {
    let host = origin.strip_prefix("http://").unwrap_or(origin);
    last(headers, "host") == Some(host.as_bytes())
        && last(headers, "origin").unwrap_or(origin.as_bytes()) == origin.as_bytes()
        && matches!(
            last(headers, "sec-fetch-site").unwrap_or(b"none"),
            b"none" | b"same-origin"
        )
}

/// Avgjør om forespørselen er fra desktop-vinduet: tokenet i headeren eller i
/// sesjons-cookien, og samme origin.
pub fn is_allowed(headers: &HeaderMap, token: &str, origin: &str) -> bool {
    let header_ok = compare_digest(
        last(headers, SESSION_HEADER).unwrap_or(b""),
        token.as_bytes(),
    );
    let cookie_ok = compare_digest(
        cookie(headers, &cookie_name(origin)).unwrap_or(b""),
        token.as_bytes(),
    );
    (header_ok | cookie_ok) && same_origin(headers, origin)
}

/// `GET SESSION_PATH?code=…`: engangskode → sesjons-cookie og `303` til `/`.
fn session_bootstrap(req: &Request, cfg: &DesktopConfig) -> Response {
    let code = req
        .uri()
        .query()
        .into_iter()
        .flat_map(|q| form_urlencoded::parse(q.as_bytes()))
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default();
    if *req.method() != Method::GET
        || !same_origin(req.headers(), &cfg.origin)
        || !cfg.take_launch_code(&code)
    {
        return ApiError::http(403, "desktop_access", "Ugyldig eller brukt oppstartslenke.")
            .into_response();
    }
    let cookie = format!(
        "{}={}; HttpOnly; SameSite=Strict; Path=/",
        cookie_name(&cfg.origin),
        cfg.token
    );
    Response::builder()
        .status(303)
        .header("location", "/")
        .header("set-cookie", cookie)
        .header("cache-control", "no-store")
        .body(axum::body::Body::empty())
        .unwrap_or_else(|_| ApiError::internal("svar").into_response())
}

/// Mellomvaren. Legges ytterst (før rutingen), som Starlettes `add_middleware`.
pub async fn desktop_access(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(cfg) = state.desktop() else {
        return next.run(req).await;
    };
    if req.uri().path() == SESSION_PATH {
        return session_bootstrap(&req, cfg);
    }
    let (status, message) = if !is_allowed(req.headers(), &cfg.token, &cfg.origin) {
        (403, "Tilgang til appen krever en gyldig desktop-sesjon.")
    } else if !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS)
        && state.runner().is_closing()
    {
        (503, "Appen avslutter. Vent til pågående arbeid er ferdig.")
    } else if req.method() == Method::POST && req.uri().path() == "/api/maintenance/data-dir" {
        // Flytting trenger en egen leveranse med quiescence og bytte av mounts.
        (
            409,
            "Flytting av datamappa er ikke tilgjengelig i denne desktop-utprøvingen.",
        )
    } else {
        return next.run(req).await;
    };
    ApiError::http(status, "desktop_access", message).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    const ORIGIN: &str = "http://127.0.0.1:40000";

    #[test]
    fn token_host_origin_og_fetch_site() {
        let ok = [(SESSION_HEADER, "hemmelig"), ("host", "127.0.0.1:40000")];
        assert!(is_allowed(&headers(&ok), "hemmelig", ORIGIN));
        let mut with = ok.to_vec();
        with.push(("origin", ORIGIN));
        with.push(("sec-fetch-site", "same-origin"));
        assert!(is_allowed(&headers(&with), "hemmelig", ORIGIN));

        for bad in [
            vec![("host", "127.0.0.1:40000")],
            vec![(SESSION_HEADER, "feil"), ("host", "127.0.0.1:40000")],
            vec![(SESSION_HEADER, "hemmelig"), ("host", "localhost:40000")],
            vec![
                (SESSION_HEADER, "hemmelig"),
                ("host", "127.0.0.1:40000"),
                ("origin", "http://evil.example"),
            ],
            vec![
                (SESSION_HEADER, "hemmelig"),
                ("host", "127.0.0.1:40000"),
                ("sec-fetch-site", "cross-site"),
            ],
        ] {
            assert!(!is_allowed(&headers(&bad), "hemmelig", ORIGIN), "{bad:?}");
        }
    }

    #[test]
    fn cookie_i_stedet_for_header() {
        let name = cookie_name(ORIGIN);
        assert_eq!(name, "fotomaxxer_session_40000");
        let good = format!("annen=1; {name}=hemmelig; tredje=x");
        let ok = [("cookie", good.as_str()), ("host", "127.0.0.1:40000")];
        assert!(is_allowed(&headers(&ok), "hemmelig", ORIGIN));
        // Samme krav til Host, Origin og Sec-Fetch-Site som med headeren.
        let wrong_port = "fotomaxxer_session_40001=hemmelig".to_string();
        let wrong = format!("{name}=feil");
        let prefix = format!("x{name}=hemmelig");
        for bad in [
            vec![("cookie", wrong.as_str()), ("host", "127.0.0.1:40000")],
            vec![("cookie", wrong_port.as_str()), ("host", "127.0.0.1:40000")],
            vec![("cookie", prefix.as_str()), ("host", "127.0.0.1:40000")],
            vec![("cookie", good.as_str()), ("host", "localhost:40000")],
            vec![
                ("cookie", good.as_str()),
                ("host", "127.0.0.1:40000"),
                ("origin", "http://evil.example"),
            ],
            vec![
                ("cookie", good.as_str()),
                ("host", "127.0.0.1:40000"),
                ("sec-fetch-site", "cross-site"),
            ],
        ] {
            assert!(!is_allowed(&headers(&bad), "hemmelig", ORIGIN), "{bad:?}");
        }
        // Feil header + riktig cookie er nok (hver for seg sammenlignes konstant-tid).
        let both = [
            (SESSION_HEADER, "feil"),
            ("cookie", good.as_str()),
            ("host", "127.0.0.1:40000"),
        ];
        assert!(is_allowed(&headers(&both), "hemmelig", ORIGIN));
    }

    #[test]
    fn siste_header_vinner() {
        let h = headers(&[
            (SESSION_HEADER, "feil"),
            (SESSION_HEADER, "hemmelig"),
            ("host", "127.0.0.1:40000"),
        ]);
        assert!(is_allowed(&h, "hemmelig", ORIGIN));
    }
}
