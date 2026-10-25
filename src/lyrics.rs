//! Lyrics for the playing song: LRCLIB's timestamped lyrics first (so the current
//! line can be highlighted), then YouTube Music's plain text, then LRCLIB's plain text.

use crate::innertube::Client;
use crate::ytdlp::Track;
use serde_json::Value;
use std::time::Duration;

const LRCLIB: &str = "https://lrclib.net/api";
const USER_AGENT: &str = "ytmusic-lite/0.1 (personal music player)";

#[derive(Clone)]
pub struct Lyrics {
    /// `(seconds, text)`; seconds are all 0 when the lyrics are not synced
    pub lines: Vec<(f64, String)>,
    pub synced: bool,
    /// Shown above the lyrics, e.g. "YouTube Music"
    pub source: String,
}

/// Looks the song up in LRCLIB and YouTube Music. `Ok(None)` means neither has it.
pub fn find(client: &Client, track: &Track) -> Result<Option<Lyrics>, String> {
    pick(
        || lrclib(track),
        || client.lyrics(&track.id).ok().flatten().map(|text| plain_lines(&text)).filter(|lines| !lines.is_empty()),
    )
}

/// Chooses between the sources. The YouTube lookup is only made when LRCLIB has
/// no timestamped version, since that is the only case where it can matter.
fn pick(
    lrclib: impl FnOnce() -> Result<Option<Lyrics>, String>,
    youtube: impl FnOnce() -> Option<Vec<(f64, String)>>,
) -> Result<Option<Lyrics>, String> {
    let lrclib = lrclib();
    if let Ok(Some(l)) = &lrclib {
        if l.synced {
            return lrclib;
        }
    }
    if let Some(lines) = youtube() {
        return Ok(Some(Lyrics { lines, synced: false, source: "YouTube Music".into() }));
    }
    // LRCLIB's plain text beats nothing; a network error only matters if there is nothing else
    lrclib
}

fn lrclib(track: &Track) -> Result<Option<Lyrics>, String> {
    let agent: ureq::Agent =
        ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(12))).http_status_as_error(false).build().into();
    let artist = primary_artist(&track.channel);
    let get = |url: String| -> Result<Option<Value>, String> {
        let mut resp = agent.get(&url).header("User-Agent", USER_AGENT).call().map_err(|e| format!("Couldn't reach LRCLIB: {e}"))?;
        if !resp.status().is_success() {
            return Ok(None);
        }
        let text = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
        Ok(serde_json::from_str(&text).ok())
    };

    let exact = format!(
        "{LRCLIB}/get?track_name={}&artist_name={}&duration={}",
        encode(&track.title),
        encode(&artist),
        track.duration
    );
    let mut hit = get(exact)?;
    if hit.is_none() {
        // the exact lookup needs a matching duration; search is looser
        let search = format!("{LRCLIB}/search?track_name={}&artist_name={}", encode(&track.title), encode(&artist));
        if let Some(Value::Array(results)) = get(search)? {
            hit = results
                .into_iter()
                .find(|r| r["duration"].as_f64().is_some_and(|d| (d - track.duration as f64).abs() <= 3.0));
        }
    }
    let Some(hit) = hit else { return Ok(None) };

    if let Some(lrc) = hit["syncedLyrics"].as_str() {
        let lines = parse_lrc(lrc);
        if !lines.is_empty() {
            return Ok(Some(Lyrics { lines, synced: true, source: "LRCLIB".into() }));
        }
    }
    if let Some(plain) = hit["plainLyrics"].as_str() {
        let lines = plain_lines(plain);
        if !lines.is_empty() {
            return Ok(Some(Lyrics { lines, synced: false, source: "LRCLIB".into() }));
        }
    }
    Ok(None)
}

/// "Daft Punk · Discovery" -> "Daft Punk"; "A & B · Album" -> "A"
fn primary_artist(channel: &str) -> String {
    let first = channel.split(" · ").next().unwrap_or(channel);
    first.split([',', '&']).next().unwrap_or(first).trim().to_string()
}

fn plain_lines(text: &str) -> Vec<(f64, String)> {
    let mut lines: Vec<(f64, String)> = text.lines().map(|l| (0.0, l.trim().to_string())).collect();
    // drop blank lines at the start and end, keep the ones between verses
    while lines.first().is_some_and(|l| l.1.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.1.is_empty()) {
        lines.pop();
    }
    lines
}

/// Parses LRC text: `[mm:ss.xx] words`. A line may carry several timestamps.
fn parse_lrc(lrc: &str) -> Vec<(f64, String)> {
    let mut out: Vec<(f64, String)> = Vec::new();
    for raw in lrc.lines() {
        let mut rest = raw.trim();
        let mut stamps = Vec::new();
        // consume leading [mm:ss.xx] stamps; anything else in brackets ([ar:Artist] tags,
        // or a lyric that starts with "[Chorus]") ends the stamp list
        while let Some(after) = rest.strip_prefix('[') {
            let Some(end) = after.find(']') else { break };
            let Some(t) = parse_stamp(&after[..end]) else { break };
            stamps.push(t);
            rest = after[end + 1..].trim_start();
        }
        for t in stamps {
            out.push((t, rest.trim().to_string()));
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

fn parse_stamp(s: &str) -> Option<f64> {
    let (m, sec) = s.split_once(':')?;
    Some(m.parse::<f64>().ok()? * 60.0 + sec.parse::<f64>().ok()?)
}

/// Percent-encodes a query value
fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_timestamped_lines_and_skips_metadata() {
        let lrc = "[ar:Someone]\n[ti:A title]\n[00:12.50] second\n[00:01.00]first\n[01:02.250][00:30.00] repeated\n";
        let lines = parse_lrc(lrc);
        let texts: Vec<&str> = lines.iter().map(|l| l.1.as_str()).collect();
        assert_eq!(texts, ["first", "second", "repeated", "repeated"]);
        assert_eq!(lines[0].0, 1.0);
        assert_eq!(lines[1].0, 12.5);
        assert_eq!(lines[2].0, 30.0);
        assert_eq!(lines[3].0, 62.25);
    }

    fn lyr(synced: bool, source: &str) -> Lyrics {
        Lyrics { lines: vec![(0.0, "x".into())], synced, source: source.into() }
    }

    #[test]
    fn synced_lrclib_wins_and_youtube_is_not_even_asked() {
        let got = pick(|| Ok(Some(lyr(true, "LRCLIB"))), || panic!("YouTube must not be queried")).unwrap().unwrap();
        assert!(got.synced);
        assert_eq!(got.source, "LRCLIB");
    }

    #[test]
    fn youtube_plain_text_beats_unsynced_lrclib() {
        let got = pick(|| Ok(Some(lyr(false, "LRCLIB"))), || Some(vec![(0.0, "y".into())])).unwrap().unwrap();
        assert_eq!(got.source, "YouTube Music");
        assert!(!got.synced);
    }

    #[test]
    fn unsynced_lrclib_is_the_last_resort_before_nothing() {
        assert_eq!(pick(|| Ok(Some(lyr(false, "LRCLIB"))), || None).unwrap().unwrap().source, "LRCLIB");
        assert!(pick(|| Ok(None), || None).unwrap().is_none());
    }

    #[test]
    fn a_failed_lrclib_lookup_falls_back_to_youtube_or_reports_the_error() {
        let got = pick(|| Err("offline".into()), || Some(vec![(0.0, "y".into())])).unwrap().unwrap();
        assert_eq!(got.source, "YouTube Music");
        assert_eq!(pick(|| Err("offline".into()), || None).err().as_deref(), Some("offline"));
    }

    #[test]
    fn keeps_bracketed_words_that_are_part_of_the_lyric() {
        let lines = parse_lrc("[00:05.00] [Chorus]\n[00:09.00][Verse] words");
        assert_eq!(lines[0].1, "[Chorus]");
        assert_eq!(lines[1].1, "[Verse] words");
    }

    #[test]
    fn plain_text_keeps_verse_breaks_but_trims_the_ends() {
        let lines = plain_lines("\n\none\ntwo\n\nthree\n\n");
        let texts: Vec<&str> = lines.iter().map(|l| l.1.as_str()).collect();
        assert_eq!(texts, ["one", "two", "", "three"]);
    }

    #[test]
    fn picks_the_first_artist() {
        assert_eq!(primary_artist("Daft Punk · Discovery"), "Daft Punk");
        assert_eq!(primary_artist("Robin Schulz & Felix Jaehn · IIII"), "Robin Schulz");
        assert_eq!(primary_artist("Solo"), "Solo");
    }

    #[test]
    fn encodes_query_values() {
        assert_eq!(encode("Beyoncé & Jay-Z"), "Beyonc%C3%A9%20%26%20Jay-Z");
    }
}
