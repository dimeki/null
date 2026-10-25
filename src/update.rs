//! "Is there a newer version?" The app downloads a tiny JSON file and compares
//! version numbers. It never downloads or runs a new program itself: the notice
//! just opens the release page in the browser.
//!
//! The address can be a GitHub "latest release" API URL, or a file that looks like:
//!   { "version": "1.2.0", "url": "https://example.com/releases/1.2.0", "notes": "What changed" }

use serde_json::Value;
use std::time::Duration;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    /// The page or file to open (always https)
    pub url: String,
    pub notes: String,
}

impl Release {
    /// The site the link goes to, shown next to the button so people see where it leads
    pub fn host(&self) -> String {
        self.url.strip_prefix("https://").and_then(|r| r.split(['/', '?', '#']).next()).unwrap_or("").to_string()
    }
}

/// "1.2.3" or "v1.2" -> (1, 2, 3) / (1, 2, 0). Anything after a `-` (a pre-release tag) is ignored.
pub fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let core = text.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map_or(Some(0), |p| p.parse().ok())?;
    let patch = parts.next().map_or(Some(0), |p| p.parse().ok())?;
    parts.next().is_none().then_some((major, minor, patch))
}

pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

/// Reads the manifest. `Ok(None)` means "you're up to date".
pub fn interpret(body: &str, current: &str) -> Result<Option<Release>, String> {
    let v: Value = serde_json::from_str(body).map_err(|_| "The update information wasn't readable".to_string())?;
    // a GitHub release (`tag_name`, `html_url`, `body`) or the plain manifest (`version`, `url`, `notes`)
    let github = v.get("tag_name").is_some();
    if github && (v["draft"].as_bool() == Some(true) || v["prerelease"].as_bool() == Some(true)) {
        return Ok(None);
    }
    let version = v[if github { "tag_name" } else { "version" }]
        .as_str()
        .filter(|s| parse_version(s).is_some())
        .ok_or("The update information has no valid version")?;
    if !is_newer(version, current) {
        return Ok(None);
    }
    let version = version.trim().trim_start_matches('v');
    let url = v[if github { "html_url" } else { "url" }].as_str().unwrap_or("");
    // the link is opened in a browser, so accept nothing but plain https addresses
    if !url.starts_with("https://") || url.len() > 500 || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("The update information has no safe download link".into());
    }
    let raw = v[if github { "body" } else { "notes" }].as_str().unwrap_or("");
    // release notes are markdown: the banner shows just the first line of text
    let first = if github { raw.lines().map(|l| l.trim().trim_start_matches(['#', '-', '*', ' '])).find(|l| !l.is_empty()).unwrap_or("") } else { raw };
    let notes: String = first.chars().take(120).collect();
    Ok(Some(Release { version: version.to_string(), url: url.to_string(), notes }))
}

pub fn fetch(manifest_url: &str, current: &str) -> Result<Option<Release>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut resp = agent
        .get(manifest_url)
        .header("User-Agent", format!("ytmusic-lite/{current}"))
        .header("Accept", "application/vnd.github+json, application/json")
        .call()
        .map_err(|_| "Couldn't reach the update server".to_string())?;
    // GitHub answers 404 for a repository that has no published release yet
    if resp.status().as_u16() == 404 && manifest_url.contains("/releases/latest") {
        return Ok(None);
    }
    if !resp.status().is_success() {
        return Err(format!("The update server answered {}", resp.status().as_u16()));
    }
    let body = resp
        .body_mut()
        .with_config()
        .limit(64 * 1024)
        .read_to_string()
        .map_err(|_| "The update information was too large or unreadable".to_string())?;
    interpret(&body, current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn versions_compare_by_number_not_by_text() {
        assert!(is_newer("1.10.0", "1.9.9"), "10 is more than 9");
        assert!(is_newer("2.0", "1.99.99"));
        assert!(is_newer("v1.0.1", "1.0.0"));
        assert!(!is_newer("1.0.0", "1.0.0"));
        assert!(!is_newer("0.9.0", "1.0.0"));
        assert!(!is_newer("1.0.0-beta", "1.0.0"), "a pre-release tag is not newer");
        assert!(!is_newer("banana", "1.0.0"));
        assert!(!is_newer("1.0.0", "banana"));
    }

    #[test]
    fn this_build_has_a_valid_version() {
        assert!(parse_version(CURRENT_VERSION).is_some(), "{CURRENT_VERSION}");
    }

    #[test]
    fn a_newer_release_is_reported_with_its_details() {
        let body = r#"{"version":"1.2.0","url":"https://github.com/you/ytmusic-lite/releases/tag/v1.2.0","notes":"Lyrics fixes"}"#;
        let r = interpret(body, "1.1.0").unwrap().unwrap();
        assert_eq!(r.version, "1.2.0");
        assert_eq!(r.host(), "github.com");
        assert_eq!(r.notes, "Lyrics fixes");
        assert_eq!(interpret(body, "1.2.0").unwrap(), None);
        assert_eq!(interpret(body, "1.3.0").unwrap(), None);
    }

    #[test]
    fn a_github_release_is_understood() {
        let body = r###"{"tag_name":"v1.2.0","html_url":"https://github.com/dimeki/null/releases/tag/v1.2.0","body":"## What's new\r\n- Lyrics fixes\r\n","draft":false,"prerelease":false}"###;
        let r = interpret(body, "1.1.0").unwrap().unwrap();
        assert_eq!(r.version, "1.2.0", "the leading v is dropped");
        assert_eq!(r.host(), "github.com");
        assert_eq!(r.notes, "What's new");
        assert_eq!(interpret(body, "1.2.0").unwrap(), None);
        let pre = body.replace(r#""prerelease":false"#, r#""prerelease":true"#);
        assert_eq!(interpret(&pre, "1.1.0").unwrap(), None, "pre-releases are skipped");
        let draft = body.replace(r#""draft":false"#, r#""draft":true"#);
        assert_eq!(interpret(&draft, "1.1.0").unwrap(), None);
        let bad = body.replace("https://github.com", "http://github.com");
        assert!(interpret(&bad, "1.1.0").is_err());
    }

    #[test]
    fn unsafe_or_broken_manifests_are_refused() {
        let bad_links = ["http://example.com/x", "javascript:alert(1)", "file:///c:/windows/system32/calc.exe", "https://a.com/x y", "", "ftp://x.y/z"];
        for link in bad_links {
            let body = format!(r#"{{"version":"9.0.0","url":"{link}","notes":""}}"#);
            assert!(interpret(&body, "1.0.0").is_err(), "{link:?} must be refused");
        }
        assert!(interpret("not json", "1.0.0").is_err());
        assert!(interpret(r#"{"url":"https://a.b/c"}"#, "1.0.0").is_err(), "no version");
        assert!(interpret(r#"{"version":"soon","url":"https://a.b/c"}"#, "1.0.0").is_err());
    }

    /// A one-shot HTTP server on localhost that answers with `status` and `body`
    fn serve_once(status: &str, body: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let response = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(response.as_bytes());
            }
        });
        format!("http://127.0.0.1:{port}/latest.json")
    }

    #[test]
    fn fetching_works_end_to_end_against_a_local_server() {
        let url = serve_once("200 OK", r#"{"version":"3.0.0","url":"https://example.com/get","notes":"n"}"#);
        let r = fetch(&url, "1.0.0").unwrap().unwrap();
        assert_eq!(r.version, "3.0.0");
        let url = serve_once("200 OK", r#"{"version":"1.0.0","url":"https://example.com/get"}"#);
        assert_eq!(fetch(&url, "1.0.0").unwrap(), None);
        let url = serve_once("404 Not Found", "{}");
        assert!(fetch(&url, "1.0.0").unwrap_err().contains("404"));
        let url = serve_once("404 Not Found", "{}").replace("latest.json", "releases/latest");
        assert_eq!(fetch(&url, "1.0.0").unwrap(), None, "no release published yet");
        assert!(fetch("http://127.0.0.1:1/latest.json", "1.0.0").is_err());
    }
}
