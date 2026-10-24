//! FastAPI-kompatible feilsvar.
//!
//! | Python                                   | Rust                       | Svar                                   |
//! | ---------------------------------------- | -------------------------- | -------------------------------------- |
//! | `HTTPException(status, detail=…)`        | [`ApiError::http`]         | `{"detail": …}` med samme status       |
//! | `RequestValidationError` (pydantic v2)   | [`ApiError::Validation`]   | 422 `{"detail": [{"type","loc",…}]}`   |
//! | Starlettes 404/405 fra ruteren           | [`ApiError::not_found`] …  | `{"detail":"Not Found"}` / `… Allowed` |
//! | Ufanget unntak (`ServerErrorMiddleware`) | [`ApiError::Internal`]     | 500 `text/plain` «Internal Server Error» |
//!
//! Alle JSON-kropper er kompakte og UTF-8. Svar med `response_model` i FastAPI går via
//! pydantic, som skriver flyttall på sin egen måte (`0.0000535`, `1e-7`); feil og
//! `dict`-svar går via `json.dumps` med Pythons `repr` (`5.35e-05`, `1e-07`). Derfor to
//! svartyper: [`JsonResponse`] (pydantic) og [`DictResponse`] (`json.dumps`).

use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::{Value, json};

/// Svar fra et endepunkt med `response_model` i FastAPI: samme bytes som pydantics
/// `model_dump_json()` (kompakt, UTF-8, pydantic-flyttall), `application/json`.
#[derive(Debug, Clone)]
pub struct JsonResponse<T>(pub T);

impl<T: Serialize> IntoResponse for JsonResponse<T> {
    fn into_response(self) -> Response {
        json_body(fotomaxxer_core::pyjson::dumps_pydantic(&self.0))
    }
}

/// Svar fra et endepunkt som returnerer en `dict` uten `response_model` (og feilsvar):
/// `json.dumps(…, ensure_ascii=False, separators=(",", ":"))` med Pythons `repr` for flyttall.
#[derive(Debug, Clone)]
pub struct DictResponse<T>(pub T);

impl<T: Serialize> IntoResponse for DictResponse<T> {
    fn into_response(self) -> Response {
        json_body(fotomaxxer_core::pyjson::dumps_compact(&self.0))
    }
}

fn json_body(body: String) -> Response {
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        )],
        body,
    )
        .into_response()
}

/// Resultattypen alle håndterere returnerer.
pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug)]
pub enum ApiError {
    /// `HTTPException(status_code, detail, headers)`.
    Http {
        status: StatusCode,
        detail: Value,
        headers: Vec<(HeaderName, HeaderValue)>,
    },
    /// `RequestValidationError`: listen med pydantic-feil (`type`, `loc`, `msg`, `input`, ev. `ctx`).
    Validation(Vec<Value>),
    /// Ufanget feil. Teksten logges til stderr; klienten får bare «Internal Server Error».
    Internal(String),
}

impl ApiError {
    /// `HTTPException(status, detail={"error": error, "message": message})` — formen
    /// alle FotoMaxxer-ruterne bruker, og som frontend leser `detail.message` fra.
    pub fn http(status: u16, error: &str, message: impl Into<String>) -> Self {
        Self::detail(status, json!({"error": error, "message": message.into()}))
    }

    /// `HTTPException(status, detail=detail)` med vilkårlig `detail`.
    pub fn detail(status: u16, detail: Value) -> Self {
        Self::Http {
            status: StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            detail,
            headers: Vec::new(),
        }
    }

    /// Starlettes 404 fra ruteren eller `StaticFiles`: `{"detail":"Not Found"}`.
    pub fn not_found() -> Self {
        Self::detail(404, Value::String("Not Found".into()))
    }

    /// Starlettes 405. `allow` er metodene til den første ruten som matchet stien
    /// (`StaticFiles` sender ingen `allow`-header).
    pub fn method_not_allowed(allow: Option<&str>) -> Self {
        let mut headers = Vec::new();
        if let Some(allow) = allow.and_then(|a| HeaderValue::from_str(a).ok()) {
            headers.push((header::ALLOW, allow));
        }
        Self::Http {
            status: StatusCode::METHOD_NOT_ALLOWED,
            detail: Value::String("Method Not Allowed".into()),
            headers,
        }
    }

    /// FastAPIs `HTTPException(400, "There was an error parsing the body")` (f.eks.
    /// kropp som ikke er gyldig UTF-8).
    pub fn body_parse_error() -> Self {
        Self::detail(
            400,
            Value::String("There was an error parsing the body".into()),
        )
    }

    pub fn internal(message: impl std::fmt::Display) -> Self {
        Self::Internal(message.to_string())
    }

    pub fn status(&self) -> StatusCode {
        match self {
            Self::Http { status, .. } => *status,
            Self::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<fotomaxxer_core::Error> for ApiError {
    fn from(e: fotomaxxer_core::Error) -> Self {
        Self::Internal(e.to_string())
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Internal(format!("database: {e}"))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            Self::Http {
                status,
                detail,
                headers,
            } => {
                let mut resp = (status, DictResponse(json!({ "detail": detail }))).into_response();
                let map: &mut HeaderMap = resp.headers_mut();
                for (name, value) in headers {
                    map.insert(name, value);
                }
                resp
            }
            Self::Validation(errors) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                DictResponse(json!({ "detail": errors })),
            )
                .into_response(),
            Self::Internal(message) => {
                eprintln!("Feil i forespørsel: {message}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    [(
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("text/plain; charset=utf-8"),
                    )],
                    "Internal Server Error",
                )
                    .into_response()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    async fn render(e: ApiError) -> (u16, Option<String>, String, Option<String>) {
        let resp = e.into_response();
        let status = resp.status().as_u16();
        let ctype = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap().to_string());
        let allow = resp
            .headers()
            .get(header::ALLOW)
            .map(|v| v.to_str().unwrap().to_string());
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            ctype,
            String::from_utf8(body.to_vec()).unwrap(),
            allow,
        )
    }

    #[tokio::test]
    async fn http_exception_som_fastapi() {
        let (status, ctype, body, _) =
            render(ApiError::http(404, "not_found", "Ukjent kilde: 77")).await;
        assert_eq!(status, 404);
        assert_eq!(ctype.as_deref(), Some("application/json"));
        assert_eq!(
            body,
            r#"{"detail":{"error":"not_found","message":"Ukjent kilde: 77"}}"#
        );
    }

    #[tokio::test]
    async fn ruterfeil_som_starlette() {
        let (status, _, body, allow) = render(ApiError::not_found()).await;
        assert_eq!((status, body.as_str()), (404, r#"{"detail":"Not Found"}"#));
        assert_eq!(allow, None);
        let (status, _, body, allow) = render(ApiError::method_not_allowed(Some("GET"))).await;
        assert_eq!(
            (status, body.as_str()),
            (405, r#"{"detail":"Method Not Allowed"}"#)
        );
        assert_eq!(allow.as_deref(), Some("GET"));
        let (_, _, body, _) = render(ApiError::body_parse_error()).await;
        assert_eq!(body, r#"{"detail":"There was an error parsing the body"}"#);
    }

    #[tokio::test]
    async fn valideringsfeil_er_422_med_liste() {
        let err =
            json!({"type": "missing", "loc": ["body"], "msg": "Field required", "input": null});
        let (status, ctype, body, _) = render(ApiError::Validation(vec![err])).await;
        assert_eq!(status, 422);
        assert_eq!(ctype.as_deref(), Some("application/json"));
        assert_eq!(
            body,
            r#"{"detail":[{"type":"missing","loc":["body"],"msg":"Field required","input":null}]}"#
        );
    }

    #[tokio::test]
    async fn intern_feil_er_ren_tekst() {
        let (status, ctype, body, _) = render(ApiError::internal("boom")).await;
        assert_eq!(status, 500);
        assert_eq!(ctype.as_deref(), Some("text/plain; charset=utf-8"));
        assert_eq!(body, "Internal Server Error");
    }
}
