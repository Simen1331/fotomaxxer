//! Mini-pydantic: validering av sti-, spørre- og kroppsparametre med samme feilformat
//! som FastAPI 0.137 / pydantic 2.13 i lax-modus.
//!
//! En forespørselsmodell deklareres én gang med [`crate::model!`], som lager både
//! serde-structen og feltlisten ([`Model::fields`]) validatoren bruker:
//!
//! ```ignore
//! model! {
//!     pub struct TagRequest {
//!         pub limit: Option<i64> = null,
//!         pub retag: bool = false,
//!     }
//! }
//! ```
//!
//! Felt uten `= standardverdi` er påkrevd. `=> ge(1) le(1000)` gir pydantics
//! `Query(…, ge=1, le=1000)`. Støttede felttyper: `i64`, `String`, `bool`, `Vec<i64>`,
//! `Vec<String>` og `Option<…>` av disse.
//!
//! Håndtererne tar parametrene inn via [`Req`] (uten database) eller [`DbReq`] (med en
//! tilkobling som Pythons `Depends(get_conn)`). Rekkefølgen følger FastAPI: kroppen leses
//! og JSON-dekodes først (`json_invalid` og 400 avbryter før noe annet), så åpnes
//! databasen (`init_db`, også når valideringen siden feiler), og til slutt valideres
//! sti, spørring og kropp — feilene samles i én 422-liste i den rekkefølgen.
//!
//! Lax-reglene er målt mot pydantic (se enhetstestene): heltall godtar `"1"`, `" 1 "`,
//! `"1_000"`, `"1.0"`, `1.0` og `true`; boolske verdier godtar `"yes"`/`"on"`/`"t"`/…,
//! `0`/`1` og `0.0`/`1.0`; strenger godtar bare strenger.
//!
//! Kjente avvik fra pydantic/FastAPI (ingen av dem kan utløses av frontend):
//! - Heltall utenfor `i64` (som streng eller JSON-heltall) gir 500 med en gang. Pydantic
//!   godtar dem, og Python feiler så med 500 først når tallet når SQLite
//!   (`OverflowError`). Store JSON-heltall over `u64` leses som flyttall av serde_json og
//!   gir `int_parsing_size`, som `1e20` gjør i pydantic.
//! - JSON-kropper må være UTF-8 (med eller uten BOM); UTF-16/32 gir 400, som ugyldig UTF-8.
//! - `NaN`/`Infinity` og ensomme surrogater (`"\ud800"`) er gyldig JSON for Python, men
//!   ikke for serde_json; de gir 500 (Python gir 500 for `NaN` i et tallfelt).

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{FromRequest, FromRequestParts, RawPathParams, Request};
use axum::http::{HeaderMap, header};
use rusqlite::Connection;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use crate::error::ApiError;
use crate::state::open_request_db;

// --- Felttyper ---------------------------------------------------------------

/// Pydantic-typen et felt valideres som.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Int,
    Str,
    Bool,
    ListInt,
    ListStr,
}

/// Kobler en Rust-type til [`Kind`] (og om `None`/`null` er lov).
pub trait FieldType {
    const KIND: Kind;
    const NULLABLE: bool = false;
}

impl FieldType for i64 {
    const KIND: Kind = Kind::Int;
}
impl FieldType for String {
    const KIND: Kind = Kind::Str;
}
impl FieldType for bool {
    const KIND: Kind = Kind::Bool;
}
impl FieldType for Vec<i64> {
    const KIND: Kind = Kind::ListInt;
}
impl FieldType for Vec<String> {
    const KIND: Kind = Kind::ListStr;
}
impl<T: FieldType> FieldType for Option<T> {
    const KIND: Kind = T::KIND;
    const NULLABLE: bool = true;
}

/// Ett felt i en modell, i deklarasjonsrekkefølge.
#[derive(Debug, Clone)]
pub struct Field {
    pub name: &'static str,
    pub kind: Kind,
    pub nullable: bool,
    /// `None` = påkrevd.
    pub default: Option<Value>,
    pub ge: Option<i64>,
    pub le: Option<i64>,
}

impl Field {
    pub fn new(name: &'static str, kind: Kind, nullable: bool) -> Self {
        Self {
            name,
            kind,
            nullable,
            default: None,
            ge: None,
            le: None,
        }
    }

    pub fn default(mut self, value: Value) -> Self {
        self.default = Some(value);
        self
    }

    pub fn ge(mut self, bound: i64) -> Self {
        self.ge = Some(bound);
        self
    }

    pub fn le(mut self, bound: i64) -> Self {
        self.le = Some(bound);
        self
    }
}

/// En pydantic-modell: serde-struct + feltliste for validering.
pub trait Model: DeserializeOwned + Sized {
    fn fields() -> Vec<Field>;
}

/// Lag en forespørselsmodell (serde-struct + [`Model`]). Se moduldokumentasjonen.
#[macro_export]
macro_rules! model {
    (
        $(#[$meta:meta])*
        pub struct $name:ident {
            $(
                $(#[$fmeta:meta])*
                pub $field:ident : $ty:ty $( = $default:tt )? $( => $( $cname:ident ( $cval:expr ) )+ )?
            ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        pub struct $name {
            $( $(#[$fmeta])* pub $field: $ty, )*
        }

        impl $crate::validate::Model for $name {
            fn fields() -> Vec<$crate::validate::Field> {
                vec![
                    $(
                        $crate::validate::Field::new(
                            stringify!($field),
                            <$ty as $crate::validate::FieldType>::KIND,
                            <$ty as $crate::validate::FieldType>::NULLABLE,
                        )
                        $( .default(serde_json::json!($default)) )?
                        $( $( .$cname($cval) )+ )?
                    ),*
                ]
            }
        }
    };
}

// --- Feil ----------------------------------------------------------------------

fn error(ty: &str, loc: &[Value], msg: &str, input: &Value) -> Value {
    json!({"type": ty, "loc": loc, "msg": msg, "input": input})
}

fn error_ctx(ty: &str, loc: &[Value], msg: &str, input: &Value, ctx: Value) -> Value {
    json!({"type": ty, "loc": loc, "msg": msg, "input": input, "ctx": ctx})
}

const INT_TYPE: &str = "Input should be a valid integer";
const INT_PARSING: &str = "Input should be a valid integer, unable to parse string as an integer";
const INT_FROM_FLOAT: &str = "Input should be a valid integer, got a number with a fractional part";
const INT_PARSING_SIZE: &str = "Unable to parse input string as an integer, exceeded maximum size";
const STRING_TYPE: &str = "Input should be a valid string";
const BOOL_TYPE: &str = "Input should be a valid boolean";
const BOOL_PARSING: &str = "Input should be a valid boolean, unable to interpret input";
const LIST_TYPE: &str = "Input should be a valid list";
const MODEL_ATTRIBUTES: &str =
    "Input should be a valid dictionary or object to extract fields from";

// --- Koersjon (lax-modus) ------------------------------------------------------

/// Pydantics `str → int`: whitespace trimmes, valgfritt fortegn, sifre med enkle `_`
/// mellom, og et valgfritt `.0…`-hale (`"1.0"` godtas, `"1."` og `"1.5"` ikke).
pub fn parse_int_str(s: &str) -> Result<i64, &'static str> {
    let t = s.trim();
    let (neg, rest) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let digits = match rest.split_once('.') {
        Some((int, frac)) if !frac.is_empty() && frac.bytes().all(|b| b == b'0') => int,
        Some(_) => return Err("int_parsing"),
        None => rest,
    };
    let bytes = digits.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_digit() || !bytes[bytes.len() - 1].is_ascii_digit() {
        return Err("int_parsing");
    }
    let mut clean = String::with_capacity(bytes.len() + 1);
    if neg {
        clean.push('-');
    }
    let mut prev_underscore = false;
    for &b in bytes {
        match b {
            b'0'..=b'9' => {
                clean.push(b as char);
                prev_underscore = false;
            }
            b'_' if !prev_underscore => prev_underscore = true,
            _ => return Err("int_parsing"),
        }
    }
    clean.parse::<i64>().map_err(|_| OVERFLOW)
}

/// Intern feiltype for heltall utenfor `i64`; blir 500 (se moduldokumentasjonen).
const OVERFLOW: &str = "__overflow__";

fn coerce_int(v: &Value) -> Result<i64, (&'static str, &'static str)> {
    match v {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i)
            } else if n.is_u64() {
                Err((OVERFLOW, OVERFLOW))
            } else {
                let f = n.as_f64().unwrap_or(f64::NAN);
                if f.fract() != 0.0 {
                    Err(("int_from_float", INT_FROM_FLOAT))
                } else if f.abs() < 9.223_372_036_854_776e18 {
                    Ok(f as i64)
                } else {
                    Err(("int_parsing_size", INT_PARSING_SIZE))
                }
            }
        }
        Value::Bool(b) => Ok(i64::from(*b)),
        Value::String(s) => parse_int_str(s).map_err(|kind| match kind {
            OVERFLOW => (OVERFLOW, OVERFLOW),
            _ => ("int_parsing", INT_PARSING),
        }),
        _ => Err(("int_type", INT_TYPE)),
    }
}

fn coerce_bool(v: &Value) -> Result<bool, (&'static str, &'static str)> {
    match v {
        Value::Bool(b) => Ok(*b),
        Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(0), _) => Ok(false),
            (Some(1), _) => Ok(true),
            (Some(_), _) => Err(("bool_parsing", BOOL_PARSING)),
            (None, Some(0.0)) => Ok(false),
            (None, Some(1.0)) => Ok(true),
            _ => Err(("bool_type", BOOL_TYPE)),
        },
        Value::String(s) => match s.to_lowercase().as_str() {
            "0" | "off" | "f" | "false" | "n" | "no" => Ok(false),
            "1" | "on" | "t" | "true" | "y" | "yes" => Ok(true),
            _ => Err(("bool_parsing", BOOL_PARSING)),
        },
        _ => Err(("bool_type", BOOL_TYPE)),
    }
}

/// Valider én verdi; feil legges i `errors`. Returnerer den koersede verdien.
fn validate_value(
    field: &Field,
    kind: Kind,
    nullable: bool,
    v: &Value,
    loc: &[Value],
    errors: &mut Vec<Value>,
) -> Option<Value> {
    if v.is_null() && nullable {
        return Some(Value::Null);
    }
    match kind {
        Kind::Int => match coerce_int(v) {
            Ok(i) => {
                if let Some(ge) = field.ge.filter(|ge| i < *ge) {
                    errors.push(error_ctx(
                        "greater_than_equal",
                        loc,
                        &format!("Input should be greater than or equal to {ge}"),
                        v,
                        json!({"ge": ge}),
                    ));
                    return None;
                }
                if let Some(le) = field.le.filter(|le| i > *le) {
                    errors.push(error_ctx(
                        "less_than_equal",
                        loc,
                        &format!("Input should be less than or equal to {le}"),
                        v,
                        json!({"le": le}),
                    ));
                    return None;
                }
                Some(Value::from(i))
            }
            Err((ty, msg)) => {
                errors.push(error(ty, loc, msg, v));
                None
            }
        },
        Kind::Str => match v {
            Value::String(_) => Some(v.clone()),
            _ => {
                errors.push(error("string_type", loc, STRING_TYPE, v));
                None
            }
        },
        Kind::Bool => match coerce_bool(v) {
            Ok(b) => Some(Value::Bool(b)),
            Err((ty, msg)) => {
                errors.push(error(ty, loc, msg, v));
                None
            }
        },
        Kind::ListInt | Kind::ListStr => {
            let Value::Array(items) = v else {
                errors.push(error("list_type", loc, LIST_TYPE, v));
                return None;
            };
            let inner = if kind == Kind::ListInt {
                Kind::Int
            } else {
                Kind::Str
            };
            let before = errors.len();
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                let mut item_loc = loc.to_vec();
                item_loc.push(Value::from(i));
                if let Some(val) = validate_value(field, inner, false, item, &item_loc, errors) {
                    out.push(val);
                }
            }
            (errors.len() == before).then_some(Value::Array(out))
        }
    }
}

/// Resultatet av en validert modell: verdien og feltene klienten faktisk sendte
/// (pydantics `model_fields_set`, brukt av f.eks. `PUT /api/faces/{id}`).
#[derive(Debug, Clone)]
pub struct Validated<T> {
    pub value: T,
    pub fields_set: Vec<&'static str>,
}

/// Valider et JSON-objekt mot modellen `T`. `prefix` er `"body"`, `"query"` eller
/// `"path"`; `missing_input` er det pydantic viser som `input` for manglende felt
/// (hele kroppen for `body`, `null` for sti og spørring).
pub fn validate_object<T: Model>(
    object: &Map<String, Value>,
    prefix: &str,
    missing_input: &Value,
    errors: &mut Vec<Value>,
) -> Option<Validated<T>> {
    let before = errors.len();
    let mut out = Map::new();
    let mut fields_set = Vec::new();
    for field in T::fields() {
        let loc = [Value::from(prefix), Value::from(field.name)];
        match object.get(field.name) {
            Some(v) => {
                fields_set.push(field.name);
                if let Some(val) =
                    validate_value(&field, field.kind, field.nullable, v, &loc, errors)
                {
                    out.insert(field.name.into(), val);
                }
            }
            None => match &field.default {
                Some(default) => {
                    out.insert(field.name.into(), default.clone());
                }
                None => errors.push(error("missing", &loc, "Field required", missing_input)),
            },
        }
    }
    if errors.len() != before {
        return None;
    }
    match serde_json::from_value::<T>(Value::Object(out)) {
        Ok(value) => Some(Validated { value, fields_set }),
        Err(e) => {
            // Feltlisten og structen er ute av takt — en programmeringsfeil, ikke brukerfeil.
            errors.push(json!({"type": "model_mismatch", "loc": [prefix], "msg": e.to_string(), "input": null}));
            None
        }
    }
}

// --- Sti og spørring -------------------------------------------------------------

/// Sti- eller spørreparametre (`()` = ingen).
pub trait Params: Sized {
    fn from_strings(
        map: &Map<String, Value>,
        prefix: &str,
        errors: &mut Vec<Value>,
    ) -> Option<Self>;
}

impl Params for () {
    fn from_strings(_: &Map<String, Value>, _: &str, _: &mut Vec<Value>) -> Option<Self> {
        Some(())
    }
}

impl<T: Model> Params for T {
    fn from_strings(
        map: &Map<String, Value>,
        prefix: &str,
        errors: &mut Vec<Value>,
    ) -> Option<Self> {
        validate_object::<T>(map, prefix, &Value::Null, errors).map(|v| v.value)
    }
}

/// Spørrestrengen som Starlette leser den (`parse_qsl(keep_blank_values=True)`):
/// `+` er mellomrom, prosentkoding dekodes som UTF-8, og siste verdi vinner.
pub fn query_map(query: Option<&str>) -> Map<String, Value> {
    let mut map = Map::new();
    for (k, v) in form_urlencoded::parse(query.unwrap_or("").as_bytes()) {
        map.insert(k.into_owned(), Value::String(v.into_owned()));
    }
    map
}

// --- Kropp -------------------------------------------------------------------

/// Kroppen slik FastAPI har lest den før valideringen.
#[derive(Debug, Clone)]
pub enum Body {
    /// Tom kropp → `None`.
    Missing,
    /// Ikke JSON-innholdstype (eller ingen): rå tekst, som pydantic avviser for modeller.
    Raw(String),
    Json(Value),
}

/// FastAPI 0.137 med `strict_content_type`: bare `application/json` og
/// `application/*+json` JSON-dekodes; uten `Content-Type` behandles kroppen som bytes.
pub fn parse_body(headers: &HeaderMap, bytes: &[u8]) -> Result<Body, ApiError> {
    if bytes.is_empty() {
        return Ok(Body::Missing);
    }
    let is_json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(is_json_content_type);
    if !is_json {
        return Ok(Body::Raw(String::from_utf8_lossy(bytes).into_owned()));
    }
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let text = std::str::from_utf8(bytes).map_err(|_| ApiError::body_parse_error())?;
    match python_json_error(text) {
        Some(PyJsonError::Decode(msg, pos)) => {
            return Err(ApiError::Validation(vec![json!({
                "type": "json_invalid",
                "loc": ["body", pos],
                "msg": "JSON decode error",
                "input": {},
                "ctx": {"error": msg},
            })]));
        }
        Some(PyJsonError::TooDeep) => return Err(ApiError::body_parse_error()),
        None => {}
    }
    serde_json::from_str(text).map(Body::Json).map_err(|e| {
        ApiError::internal(format!("JSON som Python godtar, men ikke serde_json: {e}"))
    })
}

/// `email.message`-tolkningen FastAPI bruker: hovedtype `application` og undertype
/// `json` eller `…+json` (parametre som `charset` ignoreres).
fn is_json_content_type(value: &str) -> bool {
    let mime = value
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match mime.split_once('/') {
        Some((main, sub)) => main == "application" && (sub == "json" || sub.ends_with("+json")),
        None => false,
    }
}

/// Kroppsmodell (`()` = endepunktet tar ingen kropp, og den leses ikke).
pub trait BodyModel: Sized {
    const NEEDS_BODY: bool;
    fn from_body(body: &Body, errors: &mut Vec<Value>) -> Option<(Self, Vec<&'static str>)>;
}

impl BodyModel for () {
    const NEEDS_BODY: bool = false;
    fn from_body(_: &Body, _: &mut Vec<Value>) -> Option<(Self, Vec<&'static str>)> {
        Some(((), Vec::new()))
    }
}

impl<T: Model> BodyModel for T {
    const NEEDS_BODY: bool = true;
    fn from_body(body: &Body, errors: &mut Vec<Value>) -> Option<(Self, Vec<&'static str>)> {
        match body {
            Body::Missing => {
                errors.push(error(
                    "missing",
                    &[json!("body")],
                    "Field required",
                    &Value::Null,
                ));
                None
            }
            Body::Raw(text) => {
                errors.push(error(
                    "model_attributes_type",
                    &[json!("body")],
                    MODEL_ATTRIBUTES,
                    &Value::String(text.clone()),
                ));
                None
            }
            Body::Json(Value::Object(object)) => {
                let whole = Value::Object(object.clone());
                validate_object::<T>(object, "body", &whole, errors)
                    .map(|v| (v.value, v.fields_set))
            }
            Body::Json(other) => {
                errors.push(error(
                    "model_attributes_type",
                    &[json!("body")],
                    MODEL_ATTRIBUTES,
                    other,
                ));
                None
            }
        }
    }
}

// --- Extractorer ----------------------------------------------------------------

/// Validerte sti-, spørre- og kroppsparametre for et endepunkt uten database.
#[derive(Debug)]
pub struct Req<P = (), Q = (), B = ()> {
    pub path: P,
    pub query: Q,
    pub body: B,
    /// Kroppsfeltene klienten sendte (`model_fields_set`).
    pub body_fields: Vec<&'static str>,
}

/// Som [`Req`], men med en databasetilkobling åpnet som Pythons `Depends(get_conn)`.
pub struct DbReq<P = (), Q = (), B = ()> {
    pub conn: Connection,
    pub path: P,
    pub query: Q,
    pub body: B,
    pub body_fields: Vec<&'static str>,
}

struct Raw {
    path: Map<String, Value>,
    query: Map<String, Value>,
    body: Body,
}

/// Les sti, spørring og (ved behov) kropp. JSON-dekodefeil avbryter her, før databasen
/// åpnes — som i FastAPI, der kroppen leses før avhengighetene løses.
async fn read_raw<S: Send + Sync>(
    req: Request,
    state: &S,
    needs_body: bool,
) -> Result<Raw, ApiError> {
    let (mut parts, body) = req.into_parts();
    let mut path = Map::new();
    if let Ok(params) = RawPathParams::from_request_parts(&mut parts, state).await {
        for (k, v) in params.iter() {
            path.insert(k.to_string(), Value::String(v.to_string()));
        }
    }
    let query = query_map(parts.uri.query());
    let body = if needs_body {
        let bytes: Bytes = axum::body::to_bytes(body, usize::MAX)
            .await
            .map_err(|_| ApiError::body_parse_error())?;
        parse_body(&parts.headers, &bytes)?
    } else {
        Body::Missing
    };
    Ok(Raw { path, query, body })
}

type Parsed<P, Q, B> = (P, Q, B, Vec<&'static str>);

fn validate_raw<P: Params, Q: Params, B: BodyModel>(
    raw: &Raw,
) -> Result<Parsed<P, Q, B>, ApiError> {
    let mut errors = Vec::new();
    let path = P::from_strings(&raw.path, "path", &mut errors);
    let query = Q::from_strings(&raw.query, "query", &mut errors);
    let body = B::from_body(&raw.body, &mut errors);
    if errors.iter().any(|e| e["type"] == OVERFLOW) {
        return Err(ApiError::internal(
            "heltall utenfor i64 (Python: OverflowError i SQLite)",
        ));
    }
    match (path, query, body) {
        (Some(p), Some(q), Some((b, fields))) if errors.is_empty() => Ok((p, q, b, fields)),
        _ => Err(ApiError::Validation(errors)),
    }
}

impl<S, P, Q, B> FromRequest<S> for Req<P, Q, B>
where
    S: Send + Sync,
    P: Params + Send,
    Q: Params + Send,
    B: BodyModel + Send,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let raw = read_raw(req, state, B::NEEDS_BODY).await?;
        let (path, query, body, body_fields) = validate_raw::<P, Q, B>(&raw)?;
        Ok(Self {
            path,
            query,
            body,
            body_fields,
        })
    }
}

impl<S, P, Q, B> FromRequest<S> for DbReq<P, Q, B>
where
    S: Send + Sync,
    P: Params + Send,
    Q: Params + Send,
    B: BodyModel + Send,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let raw = read_raw(req, state, B::NEEDS_BODY).await?;
        let conn = open_request_db().await?;
        let (path, query, body, body_fields) = validate_raw::<P, Q, B>(&raw)?;
        Ok(Self {
            conn,
            path,
            query,
            body,
            body_fields,
        })
    }
}

/// Spørre-/stiverdier som en enkel `HashMap` (for tester og feilsøking).
pub fn to_string_map(map: &Map<String, Value>) -> HashMap<String, String> {
    map.iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .collect()
}

// --- Pythons JSON-dekoder (feilmeldinger og posisjoner) ---------------------------

/// Første feil Pythons `json.loads` ville gitt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PyJsonError {
    /// `JSONDecodeError(msg, pos)`; `pos` er tegnindeks (kodepunkter), som i Python.
    Decode(&'static str, usize),
    /// `RecursionError` (FastAPI svarer 400).
    TooDeep,
}

/// Kjør Pythons C-skanner (`_json.c`) sin grammatikk på `text` og returnér første feil,
/// eller `None` om Python ville godtatt teksten.
pub fn python_json_error(text: &str) -> Option<PyJsonError> {
    let s: Vec<char> = text.chars().collect();
    let scanner = PyScanner { s: &s };
    let start = scanner.skip_ws(0);
    match scanner.scan_once(start, 0) {
        Err(Scan::Stop(i)) => Some(PyJsonError::Decode("Expecting value", i)),
        Err(Scan::Error(e)) => Some(e),
        Ok(end) => {
            let end = scanner.skip_ws(end);
            (end != s.len()).then_some(PyJsonError::Decode("Extra data", end))
        }
    }
}

enum Scan {
    /// `StopIteration(idx)`: ingen verdi her (blir «Expecting value»).
    Stop(usize),
    Error(PyJsonError),
}

fn fail(msg: &'static str, pos: usize) -> Scan {
    Scan::Error(PyJsonError::Decode(msg, pos))
}

/// Pythons rekursjonsgrense (1000) minus stakken FastAPI allerede bruker, omtrent.
const MAX_DEPTH: usize = 900;

struct PyScanner<'a> {
    s: &'a [char],
}

impl PyScanner<'_> {
    fn at(&self, i: usize) -> Option<char> {
        self.s.get(i).copied()
    }

    fn skip_ws(&self, mut i: usize) -> usize {
        while matches!(self.at(i), Some(' ' | '\t' | '\n' | '\r')) {
            i += 1;
        }
        i
    }

    fn literal(&self, i: usize, word: &str) -> bool {
        let n = word.chars().count();
        i + n <= self.s.len() && self.s[i..i + n].iter().copied().eq(word.chars())
    }

    fn scan_once(&self, idx: usize, depth: usize) -> Result<usize, Scan> {
        if depth > MAX_DEPTH {
            return Err(Scan::Error(PyJsonError::TooDeep));
        }
        let Some(c) = self.at(idx) else {
            return Err(Scan::Stop(idx));
        };
        match c {
            '"' => self.scan_string(idx + 1),
            '{' => self.parse_object(idx + 1, depth + 1),
            '[' => self.parse_array(idx + 1, depth + 1),
            'n' if self.literal(idx, "null") => Ok(idx + 4),
            't' if self.literal(idx, "true") => Ok(idx + 4),
            'f' if self.literal(idx, "false") => Ok(idx + 5),
            'N' if self.literal(idx, "NaN") => Ok(idx + 3),
            'I' if self.literal(idx, "Infinity") => Ok(idx + 8),
            '-' if self.literal(idx, "-Infinity") => Ok(idx + 9),
            _ => self.match_number(idx),
        }
    }

    fn is_digit(&self, i: usize) -> bool {
        self.at(i).is_some_and(|c| c.is_ascii_digit())
    }

    fn match_number(&self, start: usize) -> Result<usize, Scan> {
        let len = self.s.len();
        let mut idx = start;
        if self.at(idx) == Some('-') {
            idx += 1;
            if idx >= len {
                return Err(Scan::Stop(start));
            }
        }
        match self.at(idx) {
            Some('1'..='9') => {
                idx += 1;
                while self.is_digit(idx) {
                    idx += 1;
                }
            }
            Some('0') => idx += 1,
            _ => return Err(Scan::Stop(start)),
        }
        if idx + 1 < len && self.at(idx) == Some('.') && self.is_digit(idx + 1) {
            idx += 2;
            while self.is_digit(idx) {
                idx += 1;
            }
        }
        if idx + 1 < len && matches!(self.at(idx), Some('e' | 'E')) {
            let e_start = idx;
            idx += 1;
            if idx + 1 < len && matches!(self.at(idx), Some('-' | '+')) {
                idx += 1;
            }
            while self.is_digit(idx) {
                idx += 1;
            }
            if !self.is_digit(idx - 1) {
                idx = e_start;
            }
        }
        Ok(idx)
    }

    /// `end` peker rett etter åpningsfnutten.
    fn scan_string(&self, mut end: usize) -> Result<usize, Scan> {
        let len = self.s.len();
        let begin = end - 1;
        loop {
            let mut next = end;
            let mut c = '\0';
            while next < len {
                c = self.s[next];
                if c == '"' || c == '\\' {
                    break;
                }
                if (c as u32) <= 0x1f {
                    return Err(fail("Invalid control character at", next));
                }
                next += 1;
            }
            if next >= len || (c != '"' && c != '\\') {
                return Err(fail("Unterminated string starting at", begin));
            }
            next += 1;
            if c == '"' {
                return Ok(next);
            }
            if next == len {
                return Err(fail("Unterminated string starting at", begin));
            }
            let esc = self.s[next];
            if esc != 'u' {
                end = next + 1;
                if !matches!(esc, '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't') {
                    return Err(fail("Invalid \\escape", end - 2));
                }
            } else {
                next += 1;
                end = next + 4;
                if end >= len {
                    return Err(fail("Invalid \\uXXXX escape", next - 1));
                }
                let mut cp: u32 = 0;
                for &d in &self.s[next..end] {
                    let Some(v) = d.to_digit(16) else {
                        return Err(fail("Invalid \\uXXXX escape", end - 5));
                    };
                    cp = (cp << 4) | v;
                }
                next = end;
                // Surrogatpar: en høy surrogat fulgt av `\uXXXX`.
                if (0xd800..=0xdbff).contains(&cp)
                    && end + 6 < len
                    && self.s[next] == '\\'
                    && self.s[next + 1] == 'u'
                {
                    let start2 = next + 2;
                    let end2 = end + 6;
                    let mut cp2: u32 = 0;
                    for &d in &self.s[start2..end2] {
                        let Some(v) = d.to_digit(16) else {
                            return Err(fail("Invalid \\uXXXX escape", end2 - 5));
                        };
                        cp2 = (cp2 << 4) | v;
                    }
                    if (0xdc00..=0xdfff).contains(&cp2) {
                        end = end2;
                    }
                }
            }
        }
    }

    fn parse_object(&self, idx: usize, depth: usize) -> Result<usize, Scan> {
        let len = self.s.len();
        let mut idx = self.skip_ws(idx);
        if idx >= len || self.s[idx] != '}' {
            loop {
                if idx >= len || self.s[idx] != '"' {
                    return Err(fail(
                        "Expecting property name enclosed in double quotes",
                        idx,
                    ));
                }
                idx = self.scan_string(idx + 1)?;
                idx = self.skip_ws(idx);
                if idx >= len || self.s[idx] != ':' {
                    return Err(fail("Expecting ':' delimiter", idx));
                }
                idx = self.skip_ws(idx + 1);
                idx = self.scan_once(idx, depth)?;
                idx = self.skip_ws(idx);
                if idx < len && self.s[idx] == '}' {
                    break;
                }
                if idx >= len || self.s[idx] != ',' {
                    return Err(fail("Expecting ',' delimiter", idx));
                }
                let comma = idx;
                idx = self.skip_ws(idx + 1);
                if idx < len && self.s[idx] == '}' {
                    return Err(fail("Illegal trailing comma before end of object", comma));
                }
            }
        }
        Ok(idx + 1)
    }

    fn parse_array(&self, idx: usize, depth: usize) -> Result<usize, Scan> {
        let len = self.s.len();
        let mut idx = self.skip_ws(idx);
        if idx >= len || self.s[idx] != ']' {
            loop {
                idx = self.scan_once(idx, depth)?;
                idx = self.skip_ws(idx);
                if idx < len && self.s[idx] == ']' {
                    break;
                }
                if idx >= len || self.s[idx] != ',' {
                    return Err(fail("Expecting ',' delimiter", idx));
                }
                let comma = idx;
                idx = self.skip_ws(idx + 1);
                if idx < len && self.s[idx] == ']' {
                    return Err(fail("Illegal trailing comma before end of array", comma));
                }
            }
        }
        Ok(idx + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    crate::model! {
        /// Testmodell med alle felttypene.
        pub struct Alle {
            pub navn: String,
            pub fokus: Option<String> = null,
            pub tall: Option<i64> = null,
            pub flagg: bool = false,
            pub ider: Vec<i64> = [],
            pub kategorier: Vec<String> = [],
            pub grense: i64 = 200 => ge(1) le(1000),
        }
    }

    fn check_body(body: Value) -> Result<Validated<Alle>, Vec<Value>> {
        let mut errors = Vec::new();
        let Value::Object(map) = &body else { panic!() };
        validate_object::<Alle>(map, "body", &body, &mut errors).ok_or(errors)
    }

    #[test]
    fn heltall_fra_streng_som_pydantic() {
        for (s, want) in [
            ("1", Ok(1)),
            (" 1 ", Ok(1)),
            ("\t5\n", Ok(5)),
            ("+1", Ok(1)),
            ("-0", Ok(0)),
            ("1.0", Ok(1)),
            ("1.00", Ok(1)),
            ("1_000", Ok(1000)),
            ("007", Ok(7)),
            ("1.", Err("int_parsing")),
            ("1.5", Err("int_parsing")),
            ("1e3", Err("int_parsing")),
            ("0x10", Err("int_parsing")),
            ("_1", Err("int_parsing")),
            ("1__0", Err("int_parsing")),
            ("", Err("int_parsing")),
            (" ", Err("int_parsing")),
            ("1 2", Err("int_parsing")),
            ("１", Err("int_parsing")),
            ("9223372036854775808", Err(OVERFLOW)),
        ] {
            assert_eq!(parse_int_str(s), want, "{s:?}");
        }
    }

    #[test]
    fn koersjon_av_json_verdier() {
        assert_eq!(coerce_int(&json!(1.0)), Ok(1));
        assert_eq!(coerce_int(&json!(true)), Ok(1));
        assert_eq!(coerce_int(&json!(2.5)).unwrap_err().0, "int_from_float");
        assert_eq!(coerce_int(&json!(1e20)).unwrap_err().0, "int_parsing_size");
        assert_eq!(coerce_int(&json!(null)).unwrap_err().0, "int_type");
        assert_eq!(coerce_int(&json!([1])).unwrap_err().0, "int_type");
        for (v, want) in [
            (json!("TRUE"), Ok(true)),
            (json!("yes"), Ok(true)),
            (json!("Y"), Ok(true)),
            (json!("off"), Ok(false)),
            (json!(0), Ok(false)),
            (json!(1.0), Ok(true)),
            (json!(" true"), Err("bool_parsing")),
            (json!("ja"), Err("bool_parsing")),
            (json!(2), Err("bool_parsing")),
            (json!(0.5), Err("bool_type")),
            (json!(null), Err("bool_type")),
        ] {
            assert_eq!(coerce_bool(&v).map_err(|e| e.0), want, "{v}");
        }
    }

    #[test]
    fn modell_med_standardverdier_og_feltsett() {
        let v = check_body(json!({"navn": "A", "tall": "7", "ekstra": 1})).unwrap();
        assert_eq!(v.value.navn, "A");
        assert_eq!(v.value.tall, Some(7));
        assert_eq!(v.value.grense, 200);
        assert!(v.value.ider.is_empty());
        assert_eq!(v.fields_set, ["navn", "tall"]);
    }

    #[test]
    fn feil_i_feltrekkefolge_med_pydantic_tekst() {
        let body =
            json!({"fokus": 3, "kategorier": [1, "b", null], "flagg": "kanskje", "grense": 0});
        let errors = check_body(body.clone()).unwrap_err();
        assert_eq!(
            Value::Array(errors),
            json!([
                {"type": "missing", "loc": ["body", "navn"], "msg": "Field required", "input": body},
                {"type": "string_type", "loc": ["body", "fokus"], "msg": "Input should be a valid string", "input": 3},
                {"type": "bool_parsing", "loc": ["body", "flagg"], "msg": "Input should be a valid boolean, unable to interpret input", "input": "kanskje"},
                {"type": "string_type", "loc": ["body", "kategorier", 0], "msg": "Input should be a valid string", "input": 1},
                {"type": "string_type", "loc": ["body", "kategorier", 2], "msg": "Input should be a valid string", "input": null},
                {"type": "greater_than_equal", "loc": ["body", "grense"], "msg": "Input should be greater than or equal to 1", "input": 0, "ctx": {"ge": 1}},
            ])
        );
    }

    #[test]
    fn kropp_som_fastapi() {
        let mut errors = Vec::new();
        assert!(Alle::from_body(&Body::Missing, &mut errors).is_none());
        assert!(Alle::from_body(&Body::Raw("{bad".into()), &mut errors).is_none());
        assert!(Alle::from_body(&Body::Json(json!([1, 2])), &mut errors).is_none());
        assert_eq!(
            Value::Array(errors),
            json!([
                {"type": "missing", "loc": ["body"], "msg": "Field required", "input": null},
                {"type": "model_attributes_type", "loc": ["body"], "msg": MODEL_ATTRIBUTES, "input": "{bad"},
                {"type": "model_attributes_type", "loc": ["body"], "msg": MODEL_ATTRIBUTES, "input": [1, 2]},
            ])
        );
    }

    #[test]
    fn innholdstype_avgjor_json() {
        let mut h = HeaderMap::new();
        assert!(matches!(parse_body(&h, b"{}").unwrap(), Body::Raw(_)));
        h.insert(
            header::CONTENT_TYPE,
            "application/vnd.x+json; charset=utf-8".parse().unwrap(),
        );
        assert!(matches!(parse_body(&h, b"{}").unwrap(), Body::Json(_)));
        h.insert(header::CONTENT_TYPE, "text/plain".parse().unwrap());
        assert!(matches!(parse_body(&h, b"{}").unwrap(), Body::Raw(_)));
        h.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
        assert!(matches!(parse_body(&h, b"").unwrap(), Body::Missing));
        assert!(matches!(
            parse_body(&h, b"\xef\xbb\xbf{}").unwrap(),
            Body::Json(_)
        ));
        assert!(matches!(
            parse_body(&h, b"{\"a\": \"\xe6\"}"),
            Err(ApiError::Http { .. })
        ));
    }

    #[test]
    fn json_feil_som_pythons_dekoder() {
        // Fasit fra Pythons `json.loads` (msg, pos).
        for (text, want) in [
            ("", Some(("Expecting value", 0))),
            (" ", Some(("Expecting value", 1))),
            (
                "{",
                Some(("Expecting property name enclosed in double quotes", 1)),
            ),
            ("{\"a\"", Some(("Expecting ':' delimiter", 4))),
            ("{\"a\":", Some(("Expecting value", 5))),
            ("{\"a\":1", Some(("Expecting ',' delimiter", 6))),
            (
                "{\"a\":1,",
                Some(("Expecting property name enclosed in double quotes", 7)),
            ),
            (
                "{\"a\":1,}",
                Some(("Illegal trailing comma before end of object", 6)),
            ),
            ("{\"a\" 1}", Some(("Expecting ':' delimiter", 5))),
            (
                "{a}",
                Some(("Expecting property name enclosed in double quotes", 1)),
            ),
            ("[", Some(("Expecting value", 1))),
            ("[1", Some(("Expecting ',' delimiter", 2))),
            ("[1,", Some(("Expecting value", 3))),
            (
                "[1,]",
                Some(("Illegal trailing comma before end of array", 2)),
            ),
            ("[1 2]", Some(("Expecting ',' delimiter", 3))),
            ("\"abc", Some(("Unterminated string starting at", 0))),
            ("\"a\u{1}b\"", Some(("Invalid control character at", 2))),
            ("\"a\\qb\"", Some(("Invalid \\escape", 2))),
            ("\"\\u12\"", Some(("Invalid \\uXXXX escape", 2))),
            ("\"\\u12zz\"", Some(("Invalid \\uXXXX escape", 2))),
            ("nul", Some(("Expecting value", 0))),
            ("-", Some(("Expecting value", 0))),
            ("01", Some(("Extra data", 1))),
            ("1.", Some(("Extra data", 1))),
            ("1e", Some(("Extra data", 1))),
            ("{\"a\":1} x", Some(("Extra data", 8))),
            ("[]]", Some(("Extra data", 2))),
            ("1 2", Some(("Extra data", 2))),
            ("{\"a\":tru}", Some(("Expecting value", 5))),
            ("{\"a\":[1,{\"b\":}]}", Some(("Expecting value", 13))),
            ("{\"a\"\n:\n}", Some(("Expecting value", 7))),
            ("[,1]", Some(("Expecting value", 1))),
            (
                "{,}",
                Some(("Expecting property name enclosed in double quotes", 1)),
            ),
            ("{\"a\":1 \"b\":2}", Some(("Expecting ',' delimiter", 7))),
            ("[-]", Some(("Expecting value", 1))),
            ("1.5e", Some(("Extra data", 3))),
            ("0x10", Some(("Extra data", 1))),
            ("[1,,2]", Some(("Expecting value", 3))),
            ("\"\t\"", Some(("Invalid control character at", 1))),
            ("{\"æø\": \"å\\n\\u00e6\\ud83d\\ude00\"}", None),
            ("NaN", None),
            ("-Infinity", None),
            ("\"\\ud800\"", None),
            (" [1, 2.5e-3, {\"a\": null}] ", None),
        ] {
            let got = python_json_error(text).map(|e| match e {
                PyJsonError::Decode(m, p) => (m, p),
                PyJsonError::TooDeep => ("TooDeep", 0),
            });
            assert_eq!(got, want, "{text:?}");
        }
        assert_eq!(
            python_json_error(&"[".repeat(5000)),
            Some(PyJsonError::TooDeep)
        );
    }
}
