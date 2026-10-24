//! Tester for AI-merkingen.
//!
//! `tests/fixtures/tagging_golden.json` er laget av Python-funksjonene selv
//! (`tools/golden/tagging_golden.py`): prompt per profilsti, parsing av modellsvar,
//! tag-normalisering, kategorier, kostnad, JSON-feiltekster og flyttall. Resten kjører
//! hele steget mot en falsk leverandør og en lokal, falsk HTTP-tjener. Ingen nett.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Mutex;

use serde_json::{Value, json};

use super::*;
use crate::db;

const GOLDEN: &str = include_str!("../../tests/fixtures/tagging_golden.json");

fn golden() -> Value {
    serde_json::from_str(GOLDEN).expect("gyldig fasit-JSON")
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().map(|s| s.as_str().unwrap().to_string()).collect())
        .unwrap_or_default()
}

fn categories_map(v: &Value) -> HashMap<String, String> {
    v.as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
        .collect()
}

/// Sammenlign et `VisionResult` (eller en feil) med Pythons `_to_result`-utdata.
fn assert_case(case: &Value, got: Result<VisionResult, ImageError>, label: &str) {
    if let Some(err) = case.get("error") {
        let got = got.expect_err(label);
        assert_eq!(got.to_string(), err.as_str().unwrap(), "{label}");
        return;
    }
    let want = &case["result"];
    let got = got.unwrap_or_else(|e| panic!("{label}: uventet feil {e}"));
    let s = |k: &str| want[k].as_str().map(str::to_string);
    assert_eq!(got.scene, s("scene"), "{label} scene");
    assert_eq!(got.event_type, s("event_type"), "{label} event_type");
    assert_eq!(got.season, s("season"), "{label} season");
    assert_eq!(
        got.indoor_outdoor,
        s("indoor_outdoor"),
        "{label} indoor_outdoor"
    );
    assert_eq!(
        got.person_count,
        want["person_count"].as_i64(),
        "{label} person_count"
    );
    assert_eq!(got.description, s("description"), "{label} description");
    assert_eq!(got.tags, strings(&want["tags"]), "{label} tags");
    assert_eq!(
        got.tag_categories,
        categories_map(&want["tag_categories"]),
        "{label} tag_categories"
    );
}

#[test]
fn prompt_er_lik_python_for_alle_profilstier() {
    let g = golden();
    let cases = g["prompts"].as_array().unwrap();
    for case in cases {
        let focus = case["focus"].as_str();
        let cats = strings(&case["categories"]);
        assert_eq!(
            build_prompt(focus, &cats),
            case["prompt"].as_str().unwrap(),
            "focus={focus:?} cats={cats:?}"
        );
    }
    // Alle tre stiene er dekket: grunnprompt, flat fokus og kategorier.
    let prompts: HashSet<&str> = cases
        .iter()
        .map(|c| c["prompt"].as_str().unwrap())
        .collect();
    assert!(prompts.contains(PROMPT));
    assert!(prompts.iter().any(|p| p.contains("3–12 frie stikkord")));
    assert!(prompts.iter().any(|p| p.contains("<grunnform>")));
}

#[test]
fn modellsvar_tolkes_som_python() {
    let g = golden();
    for case in g["contents"].as_array().unwrap() {
        let content = case["content"].as_str().unwrap();
        let raw = json!({"id": "gen-1", "choices": [{"message": {"content": content}}]});
        assert_case(case, parse_response(raw), &format!("{content:?}"));
    }
}

#[test]
fn hele_svar_og_feiltyper_som_python() {
    let g = golden();
    for case in g["responses"].as_array().unwrap() {
        let raw = case["raw"].clone();
        let label = raw.to_string();
        assert_case(case, parse_response(raw), &label);
    }
}

#[test]
fn json_feiltekster_som_cpython() {
    let g = golden();
    for case in g["json_errors"].as_array().unwrap() {
        let text = case["text"].as_str().unwrap();
        assert!(serde_json::from_str::<Value>(text).is_err(), "{text:?}");
        assert_eq!(
            python_json_error(text).as_deref(),
            case["error"].as_str(),
            "{text:?}"
        );
    }
    // Det Python godtar, gir ingen feiltekst (serde_json avviser NaN og 1e400).
    for ok in [
        "{}",
        " [1, 2.5e3, -0, \"\\u00e6\"] ",
        "NaN",
        "[-Infinity]",
        "1e400",
    ] {
        assert_eq!(python_json_error(ok), None, "{ok:?}");
    }
}

#[test]
fn tag_normalisering_som_python() {
    let g = golden();
    for case in g["tags"].as_array().unwrap() {
        let (tags, cats) = parse_tags(Some(&case["input"]));
        assert_eq!(tags, strings(&case["tags"]), "{}", case["input"]);
        assert_eq!(
            cats,
            categories_map(&case["tag_categories"]),
            "{}",
            case["input"]
        );
    }
    assert_eq!(parse_tags(None), (Vec::new(), HashMap::new()));
}

#[test]
fn profilkategorier_som_python() {
    let g = golden();
    for case in g["categories"].as_array().unwrap() {
        assert_eq!(
            parse_categories(case["input"].as_str()),
            strings(&case["output"]),
            "{}",
            case["input"]
        );
    }
}

#[test]
fn kostnad_som_python() {
    let g = golden();
    for case in g["costs"].as_array().unwrap() {
        assert_eq!(
            extract_cost(&case["input"]),
            case["output"].as_f64(),
            "{}",
            case["input"]
        );
    }
}

#[test]
fn flyttall_som_python() {
    let g = golden();
    for case in g["floats"].as_array().unwrap() {
        let v = case["value"].as_f64().unwrap();
        assert_eq!(pyjson::float_repr(v), case["repr"].as_str().unwrap());
        assert_eq!(
            pyjson::dumps(&json!({"v": [v]})),
            case["dumps"].as_str().unwrap()
        );
    }
}

#[test]
fn python_str_og_tall() {
    assert_eq!(py_repr_str("a'b"), "\"a'b\"");
    assert_eq!(py_repr_str("a'b\"c"), "'a\\'b\"c'");
    assert_eq!(
        py_repr_str("æ\u{0}\u{7f}\u{a0}\u{200b}😀"),
        "'æ\\x00\\x7f\\xa0\\u200b😀'"
    );
    assert_eq!(py_int("\u{1c} 12 "), Some(12));
    assert_eq!(py_int("1__0"), None);
    assert_eq!(py_int("_1"), None);
    assert_eq!(py_int("99999999999999999999"), None);
    assert_eq!(py_float("1_0.5"), Some(10.5));
    assert_eq!(py_float("-Infinity"), Some(f64::NEG_INFINITY));
    assert_eq!(py_float("1._5"), None);
}

#[test]
fn bekreftelse_godtar_ja_varianter() {
    for yes in ["j", "ja", "y", "yes", " JA \n", "Yes"] {
        assert!(is_yes(yes), "{yes:?}");
    }
    for no in ["", "n", "nei", "jo", "ja!", "\n"] {
        assert!(!is_yes(no), "{no:?}");
    }
}

#[test]
fn leverandor_krever_nokkel_og_bruker_standardmodell() {
    let conn = Connection::open_in_memory().unwrap();
    db::init_db(&conn).unwrap();
    let missing = make_provider(&conn).err().unwrap();
    assert!(matches!(missing, Error::Tagging(_)));
    assert_eq!(missing.to_string(), MISSING_KEY);
    settings::set_setting(&conn, "openrouter_api_key", "").unwrap();
    assert!(make_provider(&conn).is_err());
    settings::set_setting(&conn, "openrouter_api_key", "sk-test").unwrap();
    settings::set_setting(&conn, "ai_model", "").unwrap();
    let p = make_provider(&conn).unwrap();
    assert_eq!(p.model(), "google/gemini-2.5-flash");
    assert_eq!(p.name(), "openrouter:google/gemini-2.5-flash");
    settings::set_setting(&conn, "ai_model", "x/y").unwrap();
    assert_eq!(make_provider(&conn).unwrap().name(), "openrouter:x/y");
}

// --- HTTP mot en lokal, falsk tjener -----------------------------------------

struct Captured {
    head: String,
    body: Vec<u8>,
}

/// Tjen `responses` (status, kropp) i rekkefølge, én tilkobling per forespørsel.
fn fake_server(responses: Vec<(u16, String)>) -> (String, std::thread::JoinHandle<Vec<Captured>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://{}/api/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let handle = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for (status, body) in responses {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut head = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                head.push_str(&line);
            }
            let len: usize = head
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse().unwrap())
                })
                .unwrap_or(0);
            let mut req_body = vec![0; len];
            reader.read_exact(&mut req_body).unwrap();
            let mut stream = reader.into_inner();
            write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json; charset=utf-8\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            seen.push(Captured {
                head,
                body: req_body,
            });
        }
        seen
    });
    (url, handle)
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

#[test]
fn http_kropp_headere_og_backoff() {
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("bilde_1024.jpg");
    std::fs::write(&img, b"\xff\xd8jpeg-bytes\xff\xd9").unwrap();
    let ok = json!({
        "id": "gen-1",
        "choices": [{"message": {"content": "```json\n{\"scene\": \"Strand\", \"tags\": [\"Hav\"]}\n```"}}],
        "usage": {"prompt_tokens": 10, "cost": 9.5e-05},
    });
    let (url, server) = fake_server(vec![
        (429, "{\"error\": \"rate limit\"}".into()),
        (503, "opptatt".into()),
        (200, ok.to_string()),
    ]);
    let p = OpenRouterProvider::with_url("sk-hemmelig", "x/y", &url)
        .with_backoff(Duration::from_millis(1));
    let cats = vec!["materiale".to_string()];
    let res = p.tag(&img, Some("Design"), &cats).unwrap();
    assert_eq!(res.scene.as_deref(), Some("Strand"));
    assert_eq!(res.tags, vec!["hav"]);
    assert_eq!(extract_cost(&res.raw), Some(9.5e-05));
    assert_eq!(
        pyjson::dumps(&res.raw),
        r#"{"id": "gen-1", "choices": [{"message": {"content": "```json\n{\"scene\": \"Strand\", \"tags\": [\"Hav\"]}\n```"}}], "usage": {"prompt_tokens": 10, "cost": 9.5e-05}}"#
    );

    let seen = server.join().unwrap();
    assert_eq!(seen.len(), 3, "429 og 503 prøves på nytt");
    let data_url = format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(std::fs::read(&img).unwrap())
    );
    let want = pyjson::dumps_ascii(&build_payload(
        "x/y",
        &build_prompt(Some("Design"), &cats),
        &data_url,
    ));
    for c in &seen {
        assert!(c.head.starts_with("POST /api/v1/chat/completions HTTP/1.1"));
        assert_eq!(header(&c.head, "authorization"), Some("Bearer sk-hemmelig"));
        assert_eq!(header(&c.head, "content-type"), Some("application/json"));
        assert_eq!(header(&c.head, "x-title"), Some("FotoMaxxer"));
        assert_eq!(String::from_utf8(c.body.clone()).unwrap(), want);
    }
    let body: Value = serde_json::from_slice(&seen[0].body).unwrap();
    assert_eq!(body["temperature"], json!(0.2));
    assert_eq!(body["response_format"], json!({"type": "json_object"}));
    assert_eq!(body["usage"], json!({"include": true}));
    assert!(want.is_ascii(), "ensure_ascii som requests");
}

#[test]
fn http_feil_etter_fem_forsok() {
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("b.jpg");
    std::fs::write(&img, b"x").unwrap();
    let long = "æ".repeat(250);
    let (url, server) = fake_server(vec![(500, long.clone()); 5]);
    let p = OpenRouterProvider::with_url("k", "m", &url).with_backoff(Duration::from_millis(1));
    let err = p.tag(&img, None, &[]).unwrap_err();
    assert_eq!(err.kind, "TaggingError");
    assert_eq!(err.message, format!("HTTP 500: {}", "æ".repeat(200)));
    assert_eq!(server.join().unwrap().len(), 5);

    let (url, server) = fake_server(vec![(400, "{}".into()); 5]);
    let p = OpenRouterProvider::with_url("k", "m", &url).with_backoff(Duration::from_millis(1));
    let err = p.tag(&img, None, &[]).unwrap_err();
    assert_eq!(
        err.to_string(),
        format!("HTTPError: 400 Client Error: Bad Request for url: {url}")
    );
    server.join().unwrap();

    let (url, server) = fake_server(vec![(200, "ikke json".into()); 5]);
    let p = OpenRouterProvider::with_url("k", "m", &url).with_backoff(Duration::from_millis(1));
    let err = p.tag(&img, None, &[]).unwrap_err();
    assert_eq!(
        err.to_string(),
        "JSONDecodeError: Expecting value: line 1 column 1 (char 0)"
    );
    assert_eq!(
        server.join().unwrap().len(),
        5,
        "ugyldig kropp prøves også på nytt"
    );
}

// --- Hele steget mot en falsk leverandør --------------------------------------

/// (filnavn, fokus, kategorier) for ett kall.
type Call = (String, Option<String>, Vec<String>);

#[derive(Default)]
struct FakeProvider {
    calls: Mutex<Vec<Call>>,
}

impl VisionProvider for FakeProvider {
    fn name(&self) -> &str {
        "falsk:modell"
    }

    fn tag(
        &self,
        image_path: &Path,
        focus: Option<&str>,
        categories: &[String],
    ) -> Result<VisionResult, ImageError> {
        let name = image_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        self.calls.lock().unwrap().push((
            name.clone(),
            focus.map(str::to_string),
            categories.to_vec(),
        ));
        let content = match &name[..2] {
            "h1" => {
                r#"{"scene": "verksted", "person_count": "2", "tags": {"materiale": ["Eik", "bjørk"], "teknikk": ["dreid", "eik"]}}"#
            }
            "h2" => r#"{"scene": "skog", "season": "null", "tags": ["eik", "Fugl", "fugl"]}"#,
            "h3" => r#"Svar: {"scene": "kjøkken", "tags": ["kopp"]} slutt"#,
            "h4" => return Err(ImageError::new("KeyError", "'choices'")),
            _ => r#"{"tags": ["annet"]}"#,
        };
        let raw = json!({"choices": [{"message": {"content": content}}], "usage": {"cost": 1e-05}});
        parse_response(raw)
    }
}

struct Lib {
    _dir: tempfile::TempDir,
    storage: Storage,
    conn: Connection,
}

/// Bilder `h1`…`hN` med ferdig derive og ai pending. Kilder: 1 (uten profil),
/// 2 (kategoriprofil), 3 (flat fokusprofil).
fn library(n: usize) -> Lib {
    let dir = tempfile::tempdir().unwrap();
    let storage = Storage::new(dir.path());
    storage.ensure_dirs().unwrap();
    let conn = Connection::open_in_memory().unwrap();
    db::init_db(&conn).unwrap();
    conn.execute_batch(
        "INSERT INTO tagging_profiles (id, name, focus, categories) VALUES
            (1, 'Håndverk', 'Design og håndverk', '[\"materiale\", \" teknikk \", \"\"]'),
            (2, 'Fugler', '  Fugler ', '[]');
         INSERT INTO sources (id, root_path, profile_id) VALUES
            (1, '/a', NULL), (2, '/b', 1), (3, '/c', 2);",
    )
    .unwrap();
    for i in 1..=n {
        let hash = format!("h{i}");
        conn.execute(
            "INSERT INTO images (id, content_hash) VALUES (?, ?)",
            params![i as i64, hash],
        )
        .unwrap();
        for (stage, status) in [("derive", "done"), ("ai", "pending"), ("faces", "pending")] {
            conn.execute(
                "INSERT INTO processing_state (image_id, stage, status) VALUES (?, ?, ?)",
                params![i as i64, stage, status],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO derived (image_id, size, path) VALUES (?, '1024', ?)",
            params![i as i64, format!("{hash}_1024.jpg")],
        )
        .unwrap();
        std::fs::write(storage.derived_path(&hash, "1024"), b"jpeg").unwrap();
    }
    Lib {
        _dir: dir,
        storage,
        conn,
    }
}

fn add_file(conn: &Connection, image_id: i64, source_id: i64) {
    conn.execute(
        "INSERT INTO files (image_id, source_id, path) VALUES (?, ?, ?)",
        params![
            image_id,
            source_id,
            format!("/f/{image_id}-{source_id}.jpg")
        ],
    )
    .unwrap();
}

fn state(conn: &Connection, image_id: i64) -> (String, Option<String>) {
    conn.query_row(
        "SELECT status, error FROM processing_state WHERE image_id = ? AND stage = 'ai'",
        [image_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap()
}

fn never(_: &str) -> bool {
    panic!("kostnadsgrensen skal ikke spørre her")
}

#[test]
fn hele_steget_profiler_lagring_og_myke_feil() {
    let lib = library(5);
    let conn = &lib.conn;
    add_file(conn, 1, 2); // kategoriprofil
    add_file(conn, 2, 1); // kilde 1 uten profil …
    add_file(conn, 2, 3); // … men også kilde 3 med flat profil → flat
    add_file(conn, 3, 1); // ingen profil, ingen standard → grunnprompt
    add_file(conn, 4, 1);
    add_file(conn, 5, 1);
    std::fs::remove_file(lib.storage.derived_path("h5", "1024")).unwrap();

    let fake = FakeProvider::default();
    let opts = TagOptions {
        workers: Some(3),
        ..TagOptions::default()
    };
    let stats = tag_with_provider(conn, &lib.storage, &fake, &opts, &mut never).unwrap();
    assert_eq!(stats.summary(), "pending=5 done=3 feil=2");

    let mut calls = fake.calls.lock().unwrap().clone();
    calls.sort();
    let cats = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        calls,
        vec![
            (
                "h1_1024.jpg".into(),
                Some("Design og håndverk".into()),
                cats(&["materiale", "teknikk"])
            ),
            ("h2_1024.jpg".into(), Some("  Fugler ".into()), vec![]),
            ("h3_1024.jpg".into(), None, vec![]),
            ("h4_1024.jpg".into(), None, vec![]),
        ]
    );

    assert_eq!(state(conn, 1), ("done".into(), None));
    assert_eq!(
        state(conn, 4),
        ("error".into(), Some("KeyError: 'choices'".into()))
    );
    assert_eq!(
        state(conn, 5),
        (
            "error".into(),
            Some("TaggingError: mangler 1024-derivat: h5_1024.jpg".into())
        )
    );

    let (provider, scene, season, pc, raw, cost): (String, String, Option<String>, Option<i64>, String, f64) = conn
        .query_row(
            "SELECT provider, scene, season_ai, person_count, raw_json, cost FROM ai_tags WHERE image_id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .unwrap();
    assert_eq!(provider, "falsk:modell");
    assert_eq!(scene, "verksted");
    assert_eq!(season, None);
    assert_eq!(pc, Some(2));
    assert!(raw.ends_with(r#""usage": {"cost": 1e-05}}"#), "{raw}");
    assert_eq!(cost, 1e-05);

    // «eik» kom kategorisert (materiale) fra h1 og flat fra h2: COALESCE beholder
    // kategorien uansett rekkefølge. Dedup innen svaret: «fugl» én gang.
    let mut tags: Vec<(String, Option<String>)> = conn
        .prepare("SELECT name, category FROM tags ORDER BY name")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    tags.sort();
    let t = |n: &str, c: Option<&str>| (n.to_string(), c.map(str::to_string));
    assert_eq!(
        tags,
        vec![
            t("bjørk", Some("materiale")),
            t("dreid", Some("teknikk")),
            t("eik", Some("materiale")),
            t("fugl", None),
            t("kopp", None),
        ]
    );
    let n_links: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM image_tags WHERE image_id = 2",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n_links, 2);

    // Idempotent: ingenting igjen å gjøre (feilede bilder prøves ikke automatisk).
    let again = tag_with_provider(conn, &lib.storage, &fake, &opts, &mut never).unwrap();
    assert_eq!(again.summary(), "pending=0 done=0 feil=0");

    // Standardprofil gjelder bilder uten kildeprofil; retag per kilde rører bare den kilden.
    settings::set_setting(conn, "default_profile_id", "2").unwrap();
    fake.calls.lock().unwrap().clear();
    let opts = TagOptions {
        retag: true,
        source_id: Some(3),
        ..TagOptions::default()
    };
    let stats = tag_with_provider(conn, &lib.storage, &fake, &opts, &mut never).unwrap();
    assert_eq!(stats.summary(), "pending=1 done=1 feil=0");
    assert_eq!(fake.calls.lock().unwrap()[0].0, "h2_1024.jpg");
    assert_eq!(state(conn, 4).0, "error");

    let opts = TagOptions {
        retag: true,
        limit: Some(2),
        ..TagOptions::default()
    };
    fake.calls.lock().unwrap().clear();
    let stats = tag_with_provider(conn, &lib.storage, &fake, &opts, &mut never).unwrap();
    assert_eq!(stats.summary(), "pending=2 done=2 feil=0");
    let mut calls = fake.calls.lock().unwrap().clone();
    calls.sort();
    assert_eq!(calls[0].0, "h1_1024.jpg");
    assert_eq!(
        calls[1],
        ("h2_1024.jpg".into(), Some("  Fugler ".into()), vec![])
    );
    // Bilde 3 (bare kilde 1) får nå standardprofilen.
    let stats = tag_with_provider(
        conn,
        &lib.storage,
        &fake,
        &TagOptions::default(),
        &mut never,
    )
    .unwrap();
    assert_eq!(stats.summary(), "pending=3 done=1 feil=2");
    let h3 = fake
        .calls
        .lock()
        .unwrap()
        .iter()
        .find(|c| c.0 == "h3_1024.jpg")
        .cloned()
        .unwrap();
    assert_eq!(h3.1.as_deref(), Some("  Fugler "));
    // Gammel ai_tags-rad erstattes, ikke dupliseres.
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM ai_tags WHERE image_id = 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(n, 1);
}

#[test]
fn kostnadsgrense_over_200_bilder() {
    let lib = library(CONFIRM_THRESHOLD + 1);
    let fake = FakeProvider::default();
    let mut asked = Vec::new();
    let stats = tag_with_provider(
        &lib.conn,
        &lib.storage,
        &fake,
        &TagOptions::default(),
        &mut |msg: &str| {
            asked.push(msg.to_string());
            false
        },
    )
    .unwrap();
    assert_eq!(stats.summary(), "avbrutt (pending=201, ingen kall gjort)");
    assert_eq!(
        asked,
        vec![
            "Skal merke 201 bilder via falsk:modell — dette treffer et betalt API og koster \
             ekte penger (per-bilde-pris varierer med modell)."
        ]
    );
    assert!(fake.calls.lock().unwrap().is_empty());

    // Akkurat på grensen spørres det ikke; --yes hopper over.
    let opts = TagOptions {
        limit: Some(200),
        ..TagOptions::default()
    };
    let stats = tag_with_provider(&lib.conn, &lib.storage, &fake, &opts, &mut never).unwrap();
    assert_eq!(stats.pending, 200);
    let opts = TagOptions {
        assume_yes: true,
        workers: Some(0),
        ..TagOptions::default()
    };
    let stats = tag_with_provider(&lib.conn, &lib.storage, &fake, &opts, &mut never).unwrap();
    assert_eq!(stats.pending, 1);

    let opts = TagOptions {
        retag: true,
        workers: Some(-1),
        assume_yes: true,
        ..TagOptions::default()
    };
    assert!(matches!(
        tag_with_provider(&lib.conn, &lib.storage, &fake, &opts, &mut never),
        Err(Error::Tagging(_))
    ));
}
