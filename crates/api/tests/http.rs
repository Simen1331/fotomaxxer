//! HTTP-tester mot hele appen (ruting, feilformat, SPA, desktop-port og jobber) uten
//! nettverk: forespørslene sendes rett inn i axum-ruteren.
//!
//! `make_storage()` leser miljøet, så alle testene deler én midlertidig pakke
//! (`FOTOMAXXER_HOME`), satt én gang før første forespørsel.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use fotomaxxer_api::spa::MemorySpa;
use fotomaxxer_api::{AppState, DesktopConfig, create_app};
use http_body_util::BodyExt;
use tower::ServiceExt;

fn package() -> &'static PathBuf {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap().keep();
        let root = dir.join("pakke");
        // SAFETY: settes én gang, før noen forespørsel leser miljøet.
        unsafe {
            std::env::set_var("FOTOMAXXER_HOME", &root);
            std::env::set_var("XDG_CONFIG_HOME", dir.join("xdg"));
        }
        root
    })
}

fn spa() -> MemorySpa {
    MemorySpa(vec![
        (
            "index.html".into(),
            b"<!doctype html><title>FotoMaxxer</title>".to_vec(),
        ),
        ("assets/app.js".into(), b"console.log(1)".to_vec()),
        ("assets/app.css".into(), b"body{}".to_vec()),
    ])
}

fn state(with_spa: bool, desktop: Option<DesktopConfig>) -> AppState {
    let root = package();
    std::fs::create_dir_all(root.join("derived")).unwrap();
    let spa = with_spa.then(|| Arc::new(spa()) as Arc<dyn fotomaxxer_api::spa::SpaSource>);
    AppState::with_parts(root.join("derived"), spa, desktop)
}

struct Resp {
    status: StatusCode,
    ctype: Option<String>,
    allow: Option<String>,
    etag: Option<String>,
    body: String,
}

async fn send(app: &axum::Router, req: Request<Body>) -> Resp {
    let resp = app.clone().oneshot(req).await.unwrap();
    let h = |name| {
        resp.headers()
            .get(name)
            .map(|v: &header::HeaderValue| v.to_str().unwrap().to_string())
    };
    let (status, ctype, allow, etag) = (
        resp.status(),
        h(header::CONTENT_TYPE),
        h(header::ALLOW),
        h(header::ETAG),
    );
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    Resp {
        status,
        ctype,
        allow,
        etag,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

fn get(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).unwrap()
}

fn json(method: &str, uri: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn health_og_ruterfeil_med_spa() {
    let app = create_app(state(true, None));
    let r = send(&app, get("/api/health")).await;
    assert_eq!(
        (r.status, r.body.as_str()),
        (StatusCode::OK, r#"{"status":"ok"}"#)
    );
    assert_eq!(r.ctype.as_deref(), Some("application/json"));

    // Kjent sti, feil metode: 405 med metoden til første rute på stien.
    let r = send(
        &app,
        Request::delete("/api/sources").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(r.allow.as_deref(), Some("GET"));
    assert_eq!(r.body, r#"{"detail":"Method Not Allowed"}"#);
    // HEAD på en GET-rute er 405 i FastAPI.
    let r = send(
        &app,
        Request::head("/api/health").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED);

    // Ukjent API-sti: SPA-fallbacken svarer 404 «ukjent endepunkt» (GET) / 405 (andre).
    let r = send(&app, get("/api/nope")).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(
        r.body,
        r#"{"detail":{"error":"not_found","message":"ukjent endepunkt"}}"#
    );
    let r = send(&app, json("POST", "/api/nope", "{}")).await;
    assert_eq!(
        (r.status, r.allow.as_deref()),
        (StatusCode::METHOD_NOT_ALLOWED, Some("GET"))
    );
}

#[tokio::test]
async fn uten_spa_er_ukjent_sti_not_found() {
    let app = create_app(state(false, None));
    for req in [get("/api/nope"), get("/personer"), json("POST", "/x", "{}")] {
        let r = send(&app, req).await;
        assert_eq!(
            (r.status, r.body.as_str()),
            (StatusCode::NOT_FOUND, r#"{"detail":"Not Found"}"#)
        );
    }
    // /assets finnes ikke uten SPA.
    assert_eq!(
        send(&app, get("/assets/app.js")).await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn spa_indeks_assets_og_304() {
    let app = create_app(state(true, None));
    for path in ["/", "/personer/3", "/favicon.ico", "/assets"] {
        let r = send(&app, get(path)).await;
        assert_eq!(r.status, StatusCode::OK, "{path}");
        assert_eq!(r.ctype.as_deref(), Some("text/html; charset=utf-8"));
        assert!(r.body.contains("FotoMaxxer"));
    }
    let r = send(&app, get("/assets/app.js")).await;
    assert_eq!(r.ctype.as_deref(), Some("text/javascript; charset=utf-8"));
    assert_eq!(r.body, "console.log(1)");
    let etag = r.etag.unwrap();
    let r = send(
        &app,
        Request::get("/assets/app.js")
            .header(header::IF_NONE_MATCH, format!("W/\"x\", {etag}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_MODIFIED);
    assert_eq!(r.etag.as_deref(), Some(etag.as_str()));
    assert!(r.body.is_empty());
    // StaticFiles: mappe og manglende fil gir 404 Not Found, HEAD virker.
    assert_eq!(
        send(&app, get("/assets/")).await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&app, get("/assets/mangler.js")).await.body,
        r#"{"detail":"Not Found"}"#
    );
    let r = send(
        &app,
        Request::head("/assets/app.css")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!((r.status, r.body.as_str()), (StatusCode::OK, ""));
    // SPA-fallbacken godtar bare GET.
    let r = send(&app, Request::head("/").body(Body::empty()).unwrap()).await;
    assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn derived_fra_datamappa() {
    let st = state(true, None);
    std::fs::write(st.derived_dir().join("abc_300.jpg"), b"jpeg").unwrap();
    let app = create_app(st);
    let r = send(&app, get("/derived/abc_300.jpg")).await;
    assert_eq!(
        (r.status, r.ctype.as_deref()),
        (StatusCode::OK, Some("image/jpeg"))
    );
    assert_eq!(r.body, "jpeg");
    assert!(r.etag.is_some());
    let r = send(&app, get("/derived/../fotomaxxer.db")).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = send(&app, json("POST", "/derived/abc_300.jpg", "{}")).await;
    assert_eq!((r.status, r.allow), (StatusCode::METHOD_NOT_ALLOWED, None));
    // `/derived` uten skråstrek er en SPA-sti, og den gir «ukjent endepunkt».
    assert!(
        send(&app, get("/derived"))
            .await
            .body
            .contains("ukjent endepunkt")
    );
}

#[tokio::test]
async fn valideringsfeil_samles_som_i_fastapi() {
    let app = create_app(state(true, None));
    let r = send(
        &app,
        json("PATCH", "/api/sources/1.5", r#"{"profile_id": "x"}"#),
    )
    .await;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        r.body,
        r#"{"detail":[{"type":"int_parsing","loc":["path","source_id"],"msg":"Input should be a valid integer, unable to parse string as an integer","input":"1.5"},{"type":"int_parsing","loc":["body","profile_id"],"msg":"Input should be a valid integer, unable to parse string as an integer","input":"x"}]}"#
    );
    let r = send(&app, json("PATCH", "/api/sources/1", "{bad")).await;
    assert_eq!(
        r.body,
        r#"{"detail":[{"type":"json_invalid","loc":["body",1],"msg":"JSON decode error","input":{},"ctx":{"error":"Expecting property name enclosed in double quotes"}}]}"#
    );
    let r = send(&app, get("/api/progress?source_id=99999999999999999999")).await;
    assert_eq!(r.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(r.ctype.as_deref(), Some("text/plain; charset=utf-8"));
}

#[tokio::test]
async fn innstillinger_og_profiler() {
    let app = create_app(state(true, None));
    let r = send(
        &app,
        json(
            "PUT",
            "/api/settings/openrouter_api_key",
            r#"{"value":"sk-or-v1-abcdefghijkl"}"#,
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(
        r.body.contains(r#""openrouter_api_key":"sk-o…ijkl""#),
        "{}",
        r.body
    );
    let r = send(
        &app,
        json("PUT", "/api/settings/hemmelig", r#"{"value":"x"}"#),
    )
    .await;
    assert_eq!(
        (r.status, r.body.as_str()),
        (
            StatusCode::BAD_REQUEST,
            r#"{"detail":{"error":"bad_key","message":"ukjent setting: hemmelig"}}"#
        )
    );
    let r = send(
        &app,
        json(
            "POST",
            "/api/profiles",
            r#"{"name":" Test ","categories":["a"," a ","b"]}"#,
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert!(
        r.body
            .ends_with(r#""name":"Test","focus":null,"categories":["a","b"]}"#),
        "{}",
        r.body
    );
}

#[tokio::test]
async fn jobber_via_api_og_opptatt() {
    let st = state(true, None);
    let app = create_app(st.clone());
    // Jobbene kjører ikke init_db (som i Python); en vanlig forespørsel oppretter skjemaet.
    assert_eq!(
        send(&app, get("/api/settings")).await.status,
        StatusCode::OK
    );
    // En blokkerende jobb holder kjøreren opptatt.
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let (steg_satt_tx, steg_satt_rx) = std::sync::mpsc::channel::<()>();
    st.runner()
        .start("eksport", move |run| {
            run.set_step("kopierer");
            steg_satt_tx.send(()).ok();
            rx.recv().ok();
            Ok(())
        })
        .unwrap();
    // Jobben kjører i en egen tråd: vent til steget er satt, ellers kan `/jobs/current`
    // under leses før det (kappløp som slo til på en treg CI-maskin).
    steg_satt_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let r = send(&app, json("POST", "/api/jobs/faces-cluster", "{}")).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(
        r.body,
        r#"{"detail":{"error":"busy","message":"en jobb kjører allerede — vent til den er ferdig"}}"#
    );
    let r = send(&app, get("/api/jobs/current")).await;
    assert!(
        r.body
            .starts_with(r#"{"running":true,"kind":"eksport","step":"kopierer","#)
    );
    tx.send(()).unwrap();
    assert!(st.runner().wait(Some(Duration::from_secs(5))));

    let r = send(
        &app,
        json("POST", "/api/jobs/faces-cluster", r#"{"full": true}"#),
    )
    .await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    assert!(r.body.ends_with(r#","kind":"faces-cluster"}"#));
    assert!(st.runner().wait(Some(Duration::from_secs(30))));
    let r = send(&app, get("/api/jobs/current")).await;
    assert!(
        r.body
            .contains(r#""running":false,"kind":"faces-cluster","step":"cluster","#),
        "{}",
        r.body
    );
    assert!(
        r.body
            .contains(r#""message":"Klynging ferdig.","progress":{},"source_id":null}"#),
        "{}",
        r.body
    );
}

#[tokio::test]
async fn desktop_porten() {
    let origin = "http://127.0.0.1:40000";
    let st = state(
        true,
        Some(DesktopConfig::new("hemmelig".into(), origin.into())),
    );
    let app = create_app(st.clone());
    let with = |req: axum::http::request::Builder| {
        req.header("x-fotomaxxer-session", "hemmelig")
            .header(header::HOST, "127.0.0.1:40000")
    };
    let r = send(&app, get("/api/health")).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(
        r.body,
        r#"{"detail":{"error":"desktop_access","message":"Tilgang til appen krever en gyldig desktop-sesjon."}}"#
    );
    let r = send(
        &app,
        with(Request::get("/api/health"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let r = send(
        &app,
        with(Request::get("/api/health"))
            .header(header::ORIGIN, "http://evil.example")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = send(
        &app,
        with(Request::post("/api/maintenance/data-dir"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"path":"/tmp/x"}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    st.runner().begin_shutdown();
    let r = send(
        &app,
        with(Request::post("/api/jobs/scan"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    let r = send(
        &app,
        with(Request::get("/api/jobs/current"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test]
async fn desktop_engangslenke_gir_cookie() {
    let origin = "http://127.0.0.1:40000";
    let cfg = DesktopConfig::new("hemmelig".into(), origin.into());
    let url = cfg.add_launch_code("engang".into());
    assert_eq!(
        url,
        "http://127.0.0.1:40000/__fotomaxxer/session?code=engang"
    );
    let app = create_app(state(true, Some(cfg)));
    let open = |path: &str| {
        Request::get(path)
            .header(header::HOST, "127.0.0.1:40000")
            .body(Body::empty())
            .unwrap()
    };
    // Feil kode, og riktig kode fra fremmed Host, gir 403 (og bruker ikke opp koden).
    let r = send(&app, open("/__fotomaxxer/session?code=feil")).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = app
        .clone()
        .oneshot(
            Request::get("/__fotomaxxer/session?code=engang")
                .header(header::HOST, "evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let r = app
        .clone()
        .oneshot(open("/__fotomaxxer/session?code=engang"))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::SEE_OTHER);
    assert_eq!(r.headers()["location"], "/");
    assert_eq!(
        r.headers()["set-cookie"],
        "fotomaxxer_session_40000=hemmelig; HttpOnly; SameSite=Strict; Path=/"
    );
    // Koden er brukt opp.
    let r = send(&app, open("/__fotomaxxer/session?code=engang")).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // Cookien slipper gjennom porten.
    let r = send(
        &app,
        Request::get("/api/health")
            .header(header::HOST, "127.0.0.1:40000")
            .header(header::COOKIE, "fotomaxxer_session_40000=hemmelig")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test]
async fn get_paa_sti_uten_get_rute() {
    // Med SPA vinner FastAPIs `GET /{full_path:path}` over 405: 404 «ukjent endepunkt»,
    // uten `allow`. HEAD og andre metoder gir fortsatt 405 med første rutes metode.
    let app = create_app(state(true, None));
    for path in [
        "/api/jobs/update",
        "/api/sources/1",
        "/api/settings/ai_model",
        "/api/export",
        "/api/albums/1/order",
    ] {
        let r = send(&app, get(path)).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(r.allow, None, "{path}");
        assert_eq!(
            r.body,
            r#"{"detail":{"error":"not_found","message":"ukjent endepunkt"}}"#
        );
    }
    let r = send(
        &app,
        Request::head("/api/sources/1").body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(
        (r.status, r.allow.as_deref()),
        (StatusCode::METHOD_NOT_ALLOWED, Some("PATCH"))
    );
    // Uten SPA finnes ingen GET-rute å falle tilbake på: 405 som i Starlette.
    let app = create_app(state(false, None));
    let r = send(&app, get("/api/jobs/update")).await;
    assert_eq!(
        (r.status, r.allow.as_deref()),
        (StatusCode::METHOD_NOT_ALLOWED, Some("POST"))
    );
}

#[tokio::test]
async fn album_og_personer_via_api() {
    let app = create_app(state(false, None));
    let r = send(&app, json("POST", "/api/albums", r#"{"name":"  Påske  "}"#)).await;
    assert_eq!(r.status, StatusCode::OK);
    let created: serde_json::Value = serde_json::from_str(&r.body).unwrap();
    assert_eq!(created["name"], "Påske");
    assert_eq!(created["item_count"], 0);
    let id = created["id"].as_i64().unwrap();
    let r = send(&app, get(&format!("/api/albums/{id}"))).await;
    assert_eq!(
        r.body,
        format!(r#"{{"id":{id},"name":"Påske","items":[]}}"#)
    );
    let r = send(&app, json("POST", "/api/albums", r#"{"name":" "}"#)).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = send(
        &app,
        Request::delete(format!("/api/albums/{id}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(r.body, format!(r#"{{"deleted":{id}}}"#));
    let r = send(&app, get(&format!("/api/albums/{id}"))).await;
    assert_eq!(
        (r.status, r.body.as_str()),
        (
            StatusCode::NOT_FOUND,
            r#"{"detail":{"error":"not_found","message":"album finnes ikke"}}"#
        )
    );

    let r = send(&app, get("/api/persons/424242/faces")).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = send(&app, json("PUT", "/api/faces/424242", "{}")).await;
    assert_eq!(
        (r.status, r.body.as_str()),
        (
            StatusCode::BAD_REQUEST,
            r#"{"detail":{"error":"bad_request","message":"ingen felt å oppdatere"}}"#
        )
    );
    let r = send(&app, json("PUT", "/api/faces/424242", r#"{"locked":true}"#)).await;
    assert_eq!(
        r.body,
        r#"{"detail":{"error":"not_found","message":"ansikt 424242 finnes ikke"}}"#
    );
    let r = send(
        &app,
        json("POST", "/api/persons/7/merge", r#"{"into_cluster_id":7}"#),
    )
    .await;
    assert_eq!(
        (r.status, r.body.as_str()),
        (
            StatusCode::BAD_REQUEST,
            r#"{"detail":{"error":"bad_request","message":"kan ikke slå sammen en person med seg selv"}}"#
        )
    );
}
