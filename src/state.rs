//! Saves the queue and playback settings between runs in a small text file:
//! a few `key=value` lines, then one tab-separated `track` line per queued song.

use crate::ytdlp::Track;
use std::path::Path;

const HEADER: &str = "ytmusic-lite-state 1";

pub struct Saved {
    pub queue: Vec<Track>,
    pub cur: Option<usize>,
    pub position: f64,
    pub shuffle: bool,
    /// 0 = off, 1 = all, 2 = one
    pub repeat: u8,
    pub volume: f32,
    /// Performance saving mode
    pub perf: bool,
    /// Address of the listening-party relay, e.g. wss://example.deno.dev
    pub party_server: String,
    /// Name shown to others in a party
    pub display_name: String,
    /// Look for a newer version when the app starts and once a day
    pub auto_update_check: bool,
}

pub fn save(path: &Path, s: &Saved) -> std::io::Result<()> {
    let mut out = String::new();
    out.push_str(HEADER);
    out.push('\n');
    if let Some(c) = s.cur {
        out.push_str(&format!("cur={c}\n"));
    }
    out.push_str(&format!("position={:.1}\n", s.position));
    out.push_str(&format!("shuffle={}\n", s.shuffle as u8));
    out.push_str(&format!("repeat={}\n", s.repeat));
    out.push_str(&format!("volume={:.2}\n", s.volume));
    out.push_str(&format!("perf={}\n", s.perf as u8));
    out.push_str(&format!("party_server={}\n", clean(&s.party_server)));
    out.push_str(&format!("name={}\n", clean(&s.display_name)));
    out.push_str(&format!("update_check={}\n", s.auto_update_check as u8));
    for t in &s.queue {
        out.push_str(&format!(
            "track\t{}\t{}\t{}\t{}\t{}\n",
            t.id,
            clean(&t.title),
            clean(&t.channel),
            t.duration,
            clean(&t.thumb)
        ));
    }
    // write beside the file and rename so a crash can't leave half a state file
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, out)?;
    std::fs::rename(&tmp, path)
}

pub fn load(path: &Path) -> Option<Saved> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    if lines.next()? != HEADER {
        return None;
    }
    let mut s = Saved {
        queue: Vec::new(),
        cur: None,
        position: 0.0,
        shuffle: false,
        repeat: 0,
        volume: 0.8,
        perf: false,
        party_server: String::new(),
        display_name: String::new(),
        auto_update_check: true,
    };
    for line in lines {
        if let Some(rest) = line.strip_prefix("track\t") {
            let mut p = rest.split('\t');
            let (Some(id), Some(title), Some(channel), Some(dur)) = (p.next(), p.next(), p.next(), p.next()) else {
                continue;
            };
            if id.is_empty() {
                continue;
            }
            s.queue.push(Track {
                id: id.to_string(),
                title: title.to_string(),
                channel: channel.to_string(),
                duration: dur.parse().unwrap_or(0),
                // files from before covers were saved simply lack this field
                thumb: p.next().unwrap_or("").to_string(),
            });
        } else if let Some((k, v)) = line.split_once('=') {
            match k {
                "cur" => s.cur = v.parse().ok(),
                "position" => s.position = v.parse().unwrap_or(0.0),
                "shuffle" => s.shuffle = v == "1",
                "repeat" => s.repeat = v.parse().unwrap_or(0).min(2),
                "volume" => s.volume = v.parse().unwrap_or(0.8f32).clamp(0.0, 1.0),
                "perf" => s.perf = v == "1",
                "party_server" => s.party_server = v.trim().to_string(),
                "name" => s.display_name = v.trim().to_string(),
                "update_check" => s.auto_update_check = v != "0",
                _ => {}
            }
        }
    }
    s.cur = s.cur.filter(|&c| c < s.queue.len());
    Some(s)
}

fn clean(s: &str) -> String {
    s.chars().map(|c| if matches!(c, '\t' | '\n' | '\r') { ' ' } else { c }).collect()
}
