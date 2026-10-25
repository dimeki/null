//! "Sign in with Google": a small window with an embedded browser (WebView2) in
//! a private session. Once YouTube's session cookies appear they are written to
//! a temporary cookies.txt and go through the same import and validation as the
//! other sign-in routes. Then the browser is closed, which also lets its helper
//! processes exit, so it only costs memory while the window is open.

use crate::cookies;
use crate::engine::Cmd;
use crate::LoginWindow;
use slint::{ComponentHandle, Timer, TimerMode};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::time::Duration;
use wry::cookie::Cookie;
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::{Rect, WebView, WebViewBuilder};

const LOGIN_URL: &str =
    "https://accounts.google.com/ServiceLogin?service=youtube&continue=https%3A%2F%2Fmusic.youtube.com%2F";
const TICK: Duration = Duration::from_millis(250);
/// Cookies are checked every this many ticks (about 1.5 s)
const TICKS_PER_CHECK: u32 = 6;
/// Slint creates the native window a moment after `show`, so the browser is
/// attached on a later tick; give up after this many (about 6 s)
const MAX_ATTACH_TRIES: u32 = 24;
const WIDTH: f64 = 480.0;
const HEIGHT: f64 = 720.0;

// field order matters: the timer (which holds the webview slot) and the webview
// must go before the window they are attached to
struct Session {
    _timer: Timer,
    _webview: Rc<RefCell<Option<WebView>>>,
    window: LoginWindow,
}

thread_local! {
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
}

/// Opens the sign-in window. Must be called on the UI thread. The outcome is
/// reported as `Cmd::SignInDone` (an empty error means the user just closed the window).
pub fn open(tx: Sender<Cmd>, dest: PathBuf) {
    if SESSION.with(|s| s.borrow().is_some()) {
        return;
    }
    match start(&tx, dest) {
        Ok(session) => SESSION.with(|s| *s.borrow_mut() = Some(session)),
        Err(e) => {
            let _ = tx.send(Cmd::SignInDone(Err(e)));
        }
    }
}

fn start(tx: &Sender<Cmd>, dest: PathBuf) -> Result<Session, String> {
    let window = LoginWindow::new().map_err(|e| e.to_string())?;
    window.show().map_err(|e| e.to_string())?;

    let tx_close = tx.clone();
    window.window().on_close_requested(move || {
        let _ = tx_close.send(Cmd::SignInDone(Err(String::new())));
        end_session();
        slint::CloseRequestResponse::HideWindow
    });

    let slot: Rc<RefCell<Option<WebView>>> = Rc::new(RefCell::new(None));
    let (slot_tick, window_weak, tx_tick) = (slot.clone(), window.as_weak(), tx.clone());
    let log_path = dest.with_file_name("login-debug.txt");
    let _ = std::fs::write(&log_path, "");
    let (mut ticks, mut hits) = (0u32, 0u8);
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, TICK, move || {
        ticks += 1;
        let Some(window) = window_weak.upgrade() else { return };
        let mut slot = slot_tick.borrow_mut();

        // phase 1: attach the browser as soon as the native window exists
        let Some(webview) = slot.as_ref() else {
            match attach(&window) {
                Ok(wv) => {
                    note(&log_path, "browser attached");
                    *slot = Some(wv);
                }
                Err(e) if ticks >= MAX_ATTACH_TRIES => {
                    note(&log_path, &format!("giving up attaching: {e}"));
                    let _ = tx_tick.send(Cmd::SignInDone(Err(e)));
                    end_session();
                }
                Err(_) => {}
            }
            return;
        };

        // phase 2: watch for the signed-in session
        if ticks % TICKS_PER_CHECK != 0 {
            return;
        }
        let url = webview.url().unwrap_or_default();
        let on_youtube = is_youtube(&url);
        let cookies = session_cookies(webview);
        let signed_in = cookies.iter().any(|c| matches!(c.name(), "SAPISID" | "__Secure-3PAPISID"));
        // two checks in a row, so the rest of the cookies have been set too
        hits = if on_youtube && signed_in { hits.saturating_add(1) } else { 0 };
        note(&log_path, &describe_check(&url, &cookies, on_youtube, signed_in, hits));
        if hits == 2 {
            let result = save(&cookies, &dest);
            note(&log_path, &format!("saved: {}", result.as_ref().map_or_else(|e| e.as_str(), |_| "ok")));
            let _ = tx_tick.send(Cmd::SignInDone(result));
            end_session();
        }
    });

    Ok(Session { _timer: timer, _webview: slot, window })
}

/// True when the page itself is on youtube.com. Looking at the host matters:
/// Google's sign-in addresses carry a `continue=...youtube.com...` parameter.
fn is_youtube(url: &str) -> bool {
    let host = url.strip_prefix("https://").and_then(|rest| rest.split(['/', '?', '#']).next());
    host.is_some_and(|h| h == "youtube.com" || h.ends_with(".youtube.com"))
}

/// Appends a line to the login diagnostics file. It never holds secrets: only
/// page addresses without query strings, and cookie names without values.
fn note(path: &Path, line: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(path) {
        let _ = writeln!(f, "{line}");
    }
}

fn describe_check(url: &str, cookies: &[Cookie<'static>], on_youtube: bool, signed_in: bool, hits: u8) -> String {
    let page = url.split(['?', '#']).next().unwrap_or("");
    let has = |name: &str| cookies.iter().any(|c| c.name() == name);
    format!(
        "page={page} cookies={} SAPISID={} 3PAPISID={} SID={} LOGIN_INFO={} on_youtube={on_youtube} signed_in={signed_in} hits={hits}",
        cookies.len(),
        has("SAPISID"),
        has("__Secure-3PAPISID"),
        has("SID"),
        has("LOGIN_INFO"),
    )
}

fn attach(window: &LoginWindow) -> Result<WebView, String> {
    WebViewBuilder::new()
        .with_url(LOGIN_URL)
        .with_incognito(true)
        .with_bounds(Rect {
            position: LogicalPosition::new(0.0, 0.0).into(),
            size: LogicalSize::new(WIDTH, HEIGHT).into(),
        })
        .build_as_child(&window.window().window_handle())
        .map_err(|e| format!("Couldn't start the embedded browser: {e}"))
}

/// Closes the window and browser. Deferred, because this can be reached from
/// inside the window's own close handler or the timer's callback.
fn end_session() {
    let _ = slint::invoke_from_event_loop(|| {
        let session = SESSION.with(|s| s.borrow_mut().take());
        // dropping the window alone leaves it on screen; closing it by hand hides it, so do the same
        if let Some(s) = &session {
            let _ = s.window.hide();
        }
        drop(session);
    });
}

/// The cookies the browser would send to YouTube Music and YouTube
fn session_cookies(wv: &WebView) -> Vec<Cookie<'static>> {
    let mut all: Vec<Cookie<'static>> = Vec::new();
    for url in ["https://music.youtube.com", "https://www.youtube.com"] {
        for c in wv.cookies_for_url(url).unwrap_or_default() {
            let same = |o: &Cookie| o.name() == c.name() && o.domain() == c.domain() && o.path() == c.path();
            if !all.iter().any(same) {
                all.push(c);
            }
        }
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    // fake values only
    fn cookie(name: &'static str, value: &'static str, domain: &'static str, http_only: bool) -> Cookie<'static> {
        Cookie::build((name, value)).domain(domain).path("/").secure(true).http_only(http_only).build()
    }

    #[test]
    fn captured_browser_cookies_become_a_working_sign_in() {
        let dir = std::env::temp_dir().join(format!("ytl-login-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("cookies.txt");
        let captured = vec![
            cookie("SAPISID", "fake-sapisid", ".youtube.com", false),
            cookie("__Secure-3PAPISID", "fake-3p", ".youtube.com", true),
            cookie("LOGIN_INFO", "fake-login", ".youtube.com", true),
        ];
        save(&captured, &dest).unwrap();
        let auth = cookies::load_auth(&dest).expect("saved cookies should give a sign-in");
        assert_eq!(auth.sapisid, "fake-3p");
        assert!(auth.cookie_header.contains("LOGIN_INFO=fake-login"));
        // HttpOnly cookies must survive the round trip
        assert!(std::fs::read_to_string(&dest).unwrap().contains("#HttpOnly_.youtube.com"));
        assert!(!dest.with_extension("login.tmp").exists(), "the temporary file must be removed");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn only_real_youtube_pages_count() {
        assert!(is_youtube("https://music.youtube.com/"));
        assert!(is_youtube("https://www.youtube.com/watch?v=x"));
        assert!(!is_youtube("https://accounts.google.com/v3/signin?continue=https://music.youtube.com/"));
        assert!(!is_youtube("https://notyoutube.com/"));
        assert!(!is_youtube("http://music.youtube.com/"));
    }

    #[test]
    fn captured_cookies_without_a_session_are_rejected() {
        let dir = std::env::temp_dir().join(format!("ytl-login-test2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("cookies.txt");
        let err = save(&[cookie("PREF", "f6=400", ".youtube.com", false)], &dest).unwrap_err();
        assert!(err.contains("aren't signed in"));
        assert!(!dest.exists());
        std::fs::remove_dir_all(dir).ok();
    }
}

/// Writes the cookies as a Netscape cookies.txt and imports them like any other file
fn save(cookies: &[Cookie<'static>], dest: &Path) -> Result<(), String> {
    let mut text = String::from("# Netscape HTTP Cookie File\n");
    for c in cookies {
        let Some(domain) = c.domain() else { continue };
        // the cookie crate drops the leading dot, which would turn these into
        // host-only cookies that yt-dlp wouldn't send to www.youtube.com
        let domain = format!(".{}", domain.trim_start_matches('.'));
        let prefix = if c.http_only().unwrap_or(false) { "#HttpOnly_" } else { "" };
        let expiry = c.expires_datetime().map_or(0, |d| d.unix_timestamp());
        text.push_str(&format!(
            "{prefix}{domain}\tTRUE\t{}\t{}\t{expiry}\t{}\t{}\n",
            c.path().unwrap_or("/"),
            if c.secure().unwrap_or(false) { "TRUE" } else { "FALSE" },
            c.name(),
            c.value(),
        ));
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = dest.with_extension("login.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    let result = cookies::import_file(&tmp, dest);
    let _ = std::fs::remove_file(&tmp);
    result
}
