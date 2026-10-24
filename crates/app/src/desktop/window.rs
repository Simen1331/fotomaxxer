//! Vinduet (tao + wry) — motparten til `DesktopWindow`/`AppPage` i
//! `fotomaxxer/desktop_window.py`.
//!
//! - Webview-en (WebView2 på Windows, WebKitGTK på Linux) kjører i inkognito (ingen varige
//!   cookies eller cache) og åpner først tjenerens engangslenke, som setter sesjons-cookien.
//! - Navigasjon bort fra appens origin avvises, nye vinduer blokkeres og nedlastinger avbrytes.
//! - Lukking mens en jobb kjører gir en dialog (avbryt jobben og lukk / fortsett). Vinduet
//!   lukkes først når tjeneren og jobben er stoppet.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tao::dpi::LogicalSize;
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::platform::run_return::EventLoopExtRunReturn;
use tao::window::{Window, WindowBuilder};
use wry::{NewWindowResponse, WebContext, WebView, WebViewBuilder};

use super::dialog;
use super::log::{error, info, warning};
use super::server::Server;
use super::smoke::{Smoke, SmokeAction};

pub const TITLE: &str = "FotoMaxxer";
const TICK: Duration = Duration::from_millis(100);

/// Hendelser til vinduets hendelsesløkke fra andre tråder.
#[derive(Debug)]
pub enum UserEvent {
    /// En ny start med samme datamappe vil ha frem vinduet.
    Activate,
    /// Røyktesten lukker vinduet (som brukeren, med «Avbryt jobben og lukk»).
    SmokeClose,
    /// Fast takt (hvert 100. ms) fra en egen tråd. `ControlFlow::WaitUntil` alene kan bli
    /// forsinket i sekunder på Linux under xvfb; en brukerhendelse vekker løkken med en gang.
    Tick,
}

/// Navigasjoner og vinduer handlerne har avvist (røyktesten leser dem).
#[derive(Debug, Default)]
pub struct Blocked {
    pub navigations: Vec<String>,
    pub new_windows: Vec<String>,
}

/// Er `url` innenfor appens origin (`http://127.0.0.1:<port>` fulgt av `/`, `?` eller `#`)?
/// Kravet om skilletegn stopper `http://127.0.0.1:4000@evil.example/` og andre porter.
pub fn is_local(url: &str, origin: &str) -> bool {
    url == "about:blank"
        || url
            .strip_prefix(origin)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(['/', '?', '#']))
}

pub struct AppWindow {
    pub window: Window,
    pub webview: WebView,
    pub blocked: Arc<Mutex<Blocked>>,
    // Må leve like lenge som webview-en.
    _context: WebContext,
}

/// Lag vinduet og webview-en. `Err` betyr at reserven (standardnettleseren) skal brukes.
pub fn create(
    event_loop: &EventLoop<UserEvent>,
    server: &Server,
    webview_dir: Option<&Path>,
) -> Result<AppWindow, String> {
    let window = WindowBuilder::new()
        .with_title(TITLE)
        .with_inner_size(LogicalSize::new(1280.0, 850.0))
        .build(event_loop)
        .map_err(|e| format!("vinduet kunne ikke lages: {e}"))?;
    let blocked = Arc::new(Mutex::new(Blocked::default()));
    let mut context = WebContext::new(webview_dir.map(Path::to_path_buf));
    let origin = server.origin.clone();
    let nav_blocked = blocked.clone();
    let win_blocked = blocked.clone();
    let builder = WebViewBuilder::new_with_web_context(&mut context)
        .with_url(server.launch_url())
        .with_incognito(true)
        .with_navigation_handler(move |url| {
            let ok = is_local(&url, &origin);
            if !ok {
                warning!("Avviste navigasjon til {url}");
                lock(&nav_blocked).navigations.push(url);
            }
            ok
        })
        .with_new_window_req_handler(move |url, _features| {
            warning!("Blokkerte nytt vindu: {url}");
            lock(&win_blocked).new_windows.push(url);
            NewWindowResponse::Deny
        })
        .with_on_page_load_handler(|event, url| {
            let what = match event {
                wry::PageLoadEvent::Started => "starter",
                wry::PageLoadEvent::Finished => "ferdig",
            };
            info!("Side {what}: {}", url.split('?').next().unwrap_or(""));
        })
        .with_download_started_handler(|url, _path| {
            warning!("Avbrøt nedlasting: {url}");
            false
        });
    #[cfg(target_os = "linux")]
    let webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        let vbox = window
            .default_vbox()
            .ok_or_else(|| "vinduet mangler GTK-boks".to_string())?;
        builder.build_gtk(vbox)
    };
    #[cfg(not(target_os = "linux"))]
    let webview = builder.build(&window);
    let webview = webview.map_err(|e| format!("webview-en kunne ikke lages: {e}"))?;
    Ok(AppWindow {
        window,
        webview,
        blocked,
        _context: context,
    })
}

pub fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Overlegg mens appen avslutter (Python deaktiverte visningen og viste en statuslinje).
const CLOSING_JS: &str = r#"(() => {
  const d = document.createElement('div');
  d.textContent = 'Venter på pågående arbeid og avslutter …';
  d.style.cssText = 'position:fixed;inset:0;z-index:2147483647;display:flex;align-items:center;'
    + 'justify-content:center;background:rgba(0,0,0,.55);color:#fff;font:600 18px system-ui,sans-serif';
  document.body.appendChild(d);
})()"#;

/// Kjør hendelsesløkken til vinduet er lukket. Returnerer prosessens exit-kode.
pub fn run(
    mut event_loop: EventLoop<UserEvent>,
    app: AppWindow,
    server: &Server,
    activations: Arc<AtomicUsize>,
    mut smoke: Option<Smoke>,
) -> i32 {
    let mut closing = false;
    let mut exit_code = 0;
    let mut closed_at: Option<Instant> = None;
    let proxy = event_loop.create_proxy();
    let ticker = event_loop.create_proxy();
    let _ = std::thread::Builder::new()
        .name("desktop-tick".into())
        .spawn(move || {
            while ticker.send_event(UserEvent::Tick).is_ok() {
                std::thread::sleep(TICK);
            }
        });
    let begin_close = |closing: &mut bool, cancel: bool, app: &AppWindow| {
        if !*closing {
            *closing = true;
            info!("Lukker vinduet (avbryt jobb: {cancel})");
            server.request_stop(cancel);
            app.window.set_title(&format!("{TITLE} – avslutter …"));
            let _ = app.webview.evaluate_script(CLOSING_JS);
        }
    };
    event_loop.run_return(|event, _target, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + TICK);
        match event {
            Event::NewEvents(StartCause::ResumeTimeReached { .. } | StartCause::Init)
            | Event::UserEvent(UserEvent::Tick) => {
                if closing {
                    if server.stopped() {
                        info!(
                            "Tjeneren og jobben er stoppet ({} ms etter lukking)",
                            closed_at.map_or(0, |t| t.elapsed().as_millis())
                        );
                        *control_flow = ControlFlow::Exit;
                    }
                    return;
                }
                if server.stopped() {
                    let why = server.error().unwrap_or_else(|| "ukjent årsak".into());
                    error!("Den lokale tjeneren stoppet uventet: {why}");
                    dialog::error(&format!(
                        "Den lokale tjeneren stoppet uventet.\n\n{why}{}",
                        super::log_hint()
                    ));
                    exit_code = 1;
                    *control_flow = ControlFlow::Exit;
                    return;
                }
                if let Some(smoke) = smoke.as_mut() {
                    match smoke.tick(&app, server, &proxy) {
                        SmokeAction::None => {}
                        SmokeAction::Close => {
                            closed_at = Some(Instant::now());
                            begin_close(&mut closing, true, &app);
                        }
                    }
                }
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                if closing {
                    return;
                }
                let cancel = server.state().runner().is_running();
                if cancel && !dialog::confirm_close() {
                    info!("Brukeren valgte å fortsette mens jobben kjører");
                    return;
                }
                closed_at = Some(Instant::now());
                begin_close(&mut closing, cancel, &app);
            }
            Event::UserEvent(UserEvent::Activate) => {
                activations.fetch_add(1, Ordering::SeqCst);
                app.window.set_minimized(false);
                app.window.set_visible(true);
                app.window.set_focus();
            }
            Event::UserEvent(UserEvent::SmokeClose) => {
                if let Some(smoke) = smoke.as_mut() {
                    smoke.on_close(server);
                }
                closed_at = Some(Instant::now());
                begin_close(&mut closing, true, &app);
                if let Some(smoke) = smoke.as_mut() {
                    smoke.after_close_started(server);
                }
            }
            _ => {}
        }
    });
    server.wait();
    if let Some(smoke) = smoke {
        exit_code = smoke.finish(server, &app);
    }
    drop(app);
    exit_code
}

#[cfg(test)]
mod tests {
    use super::is_local;

    #[test]
    fn bare_appens_origin_er_lokal() {
        let o = "http://127.0.0.1:40000";
        for ok in [
            "http://127.0.0.1:40000",
            "http://127.0.0.1:40000/",
            "http://127.0.0.1:40000/people/3?x=1",
            "http://127.0.0.1:40000#a",
            "about:blank",
        ] {
            assert!(is_local(ok, o), "{ok}");
        }
        for bad in [
            "https://example.com/",
            "file:///etc/passwd",
            "http://127.0.0.1:400001/",
            "http://127.0.0.1:40000@evil.example/",
            "http://127.0.0.1:40000.evil.example/",
            "https://127.0.0.1:40000/",
            "http://localhost:40000/",
        ] {
            assert!(!is_local(bad, o), "{bad}");
        }
    }
}
