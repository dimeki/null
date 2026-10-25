//! Sign-in via YouTube cookies. Cookies are only ever read from a Netscape
//! `cookies.txt` file (exported by the user, or dumped by yt-dlp from their
//! browser), filtered down to youtube.com and google.com, and stored under
//! %LOCALAPPDATA%\ytmusic-lite. Values are never printed or logged, and they
//! are only sent to music.youtube.com (Innertube) and youtube.com (yt-dlp).

use crate::ytdlp::YtDlp;
use std::path::Path;

/// Browsers yt-dlp can read cookies from
pub const BROWSERS: [&str; 8] = ["firefox", "chrome", "edge", "brave", "chromium", "opera", "vivaldi", "whale"];

#[derive(Clone)]
pub struct Auth {
    pub cookie_header: String,
    pub sapisid: String,
}

struct Cookie<'a> {
    domain: &'a str,
    name: &'a str,
    value: &'a str,
}

fn parse_line(line: &str) -> Option<Cookie<'_>> {
    let line = line.trim_end_matches(['\r', '\n']);
    let line = line.strip_prefix("#HttpOnly_").unwrap_or(line);
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let f: Vec<&str> = line.split('\t').collect();
    if f.len() < 7 {
        return None;
    }
    Some(Cookie { domain: f[0].trim_start_matches('.'), name: f[5], value: f[6] })
}

fn wanted_domain(domain: &str) -> bool {
    domain.ends_with("youtube.com") || domain.ends_with("google.com")
}

/// Builds the request credentials from a stored cookie file
pub fn load_auth(path: &Path) -> Option<Auth> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut pairs: Vec<(String, String)> = Vec::new();
    let (mut sapisid, mut sapisid_3p) = (None, None);
    for c in text.lines().filter_map(parse_line) {
        if !c.domain.ends_with("youtube.com") {
            continue;
        }
        match c.name {
            "SAPISID" => sapisid = Some(c.value.to_string()),
            "__Secure-3PAPISID" => sapisid_3p = Some(c.value.to_string()),
            _ => {}
        }
        pairs.retain(|(n, _)| n != c.name);
        pairs.push((c.name.to_string(), c.value.to_string()));
    }
    let sapisid = sapisid_3p.or(sapisid)?;
    let cookie_header = pairs.iter().map(|(n, v)| format!("{n}={v}")).collect::<Vec<_>>().join("; ");
    Some(Auth { cookie_header, sapisid })
}

/// Copies the YouTube/Google cookies from `src` into `dest`, dropping everything else
pub fn import_file(src: &Path, dest: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(src).map_err(|e| format!("Couldn't read that file: {e}"))?;
    let mut kept = String::from("# Netscape HTTP Cookie File\n");
    let mut count = 0;
    for line in text.lines() {
        if parse_line(line).is_some_and(|c| wanted_domain(c.domain)) {
            kept.push_str(line.trim_end_matches(['\r', '\n']));
            kept.push('\n');
            count += 1;
        }
    }
    if count == 0 {
        return Err("No YouTube cookies found in that file.".into());
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = dest.with_extension("tmp");
    std::fs::write(&tmp, kept).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, dest).map_err(|e| e.to_string())?;
    if load_auth(dest).is_none() {
        let _ = std::fs::remove_file(dest);
        return Err("Those cookies aren't signed in (no SAPISID cookie). Export them while signed in to music.youtube.com.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // fake values only
    const SAMPLE: &str = "# Netscape HTTP Cookie File\n\
.youtube.com\tTRUE\t/\tTRUE\t1893456000\tSAPISID\tfake-sapisid\n\
#HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t1893456000\t__Secure-3PAPISID\tfake-3p\n\
.youtube.com\tTRUE\t/\tFALSE\t0\tPREF\tf6=400\n\
.google.com\tTRUE\t/\tTRUE\t1893456000\tSID\tfake-sid\n\
.example.com\tTRUE\t/\tTRUE\t1893456000\ttracker\tdo-not-keep\n";

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ytl-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn import_keeps_only_youtube_and_google_cookies() {
        let dir = temp("import");
        let (src, dest) = (dir.join("in.txt"), dir.join("out.txt"));
        std::fs::write(&src, SAMPLE).unwrap();
        import_file(&src, &dest).unwrap();
        let kept = std::fs::read_to_string(&dest).unwrap();
        assert!(kept.contains("SAPISID") && kept.contains("SID\tfake-sid"));
        assert!(!kept.contains("do-not-keep"), "unrelated cookies must not be copied");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn auth_prefers_the_3p_sapisid_and_joins_youtube_cookies() {
        let dir = temp("auth");
        let src = dir.join("c.txt");
        std::fs::write(&src, SAMPLE).unwrap();
        let auth = load_auth(&src).unwrap();
        assert_eq!(auth.sapisid, "fake-3p");
        assert!(auth.cookie_header.contains("SAPISID=fake-sapisid"));
        assert!(auth.cookie_header.contains("PREF=f6=400"));
        assert!(!auth.cookie_header.contains("fake-sid"), "google.com cookies are not sent to YouTube Music");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn import_rejects_cookies_that_are_not_signed_in() {
        let dir = temp("anon");
        let (src, dest) = (dir.join("in.txt"), dir.join("out.txt"));
        std::fs::write(&src, ".youtube.com\tTRUE\t/\tFALSE\t0\tPREF\tf6=400\n").unwrap();
        let err = import_file(&src, &dest).unwrap_err();
        assert!(err.contains("aren't signed in"));
        assert!(!dest.exists(), "a rejected import must not leave a cookie file behind");
        std::fs::remove_dir_all(dir).ok();
    }
}

/// Asks yt-dlp to read the browser's cookies, then keeps only the YouTube ones.
/// Chromium browsers on Windows often block this; Firefox usually works.
pub fn import_browser(ytdlp: &YtDlp, browser: &str, dest: &Path) -> Result<(), String> {
    let tmp = dest.with_extension("browser.tmp");
    let _ = std::fs::remove_file(&tmp);
    let result = ytdlp.dump_browser_cookies(browser, &tmp);
    let imported = result.and_then(|_| import_file(&tmp, dest));
    let _ = std::fs::remove_file(&tmp);
    imported.map_err(|e| {
        let first = e.lines().find(|l| l.contains("ERROR")).unwrap_or(&e);
        // yt-dlp's message names the problem (locked database, can't decrypt, ...) without cookie values
        format!("Couldn't read {browser}'s cookies: {}", first.trim_start_matches("ERROR: "))
    })
}
