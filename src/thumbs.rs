use crate::engine::Cmd;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};

/// Covers are stored square at this many pixels (shown at half size for sharpness on 2x displays)
pub const SIZE: u32 = 72;
const BYTES: usize = (SIZE * SIZE * 4) as usize;

/// Where cover art may come from. URLs arrive inside API responses, so
/// anything else is ignored in favour of the plain video thumbnail.
fn allowed_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let host = rest.split('/').next().unwrap_or("");
    ["googleusercontent.com", "ggpht.com", "ytimg.com"].iter().any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

/// Starts the cover worker. Send it `(video id, cover url)`; the url may be empty.
/// Each comes back as `Cmd::Thumb` with raw RGBA pixels (or `None` if it couldn't be loaded).
pub fn spawn(results: Sender<Cmd>, dir: PathBuf) -> Sender<(String, String)> {
    let (tx, rx) = channel::<(String, String)>();
    std::thread::spawn(move || {
        let _ = std::fs::create_dir_all(&dir);
        let agent = ureq::Agent::new_with_defaults();
        for (id, url) in rx {
            let rgba = load(&agent, &dir, &id, &url);
            if results.send(Cmd::Thumb { id, rgba }).is_err() {
                break;
            }
        }
    });
    tx
}

fn load(agent: &ureq::Agent, dir: &Path, id: &str, cover_url: &str) -> Option<Vec<u8>> {
    // ids end up in a file name and a URL, so only accept what YouTube issues
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return None;
    }
    let path = dir.join(format!("{id}.rgba"));
    if let Ok(bytes) = std::fs::read(&path) {
        if bytes.len() == BYTES {
            return Some(bytes);
        }
    }
    let url = if allowed_url(cover_url) {
        cover_url.to_string()
    } else {
        format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg")
    };
    let mut resp = agent.get(&url).call().ok()?;
    let bytes = resp.body_mut().with_config().limit(1_000_000).read_to_vec().ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    let side = img.width().min(img.height());
    let rgba = img
        .crop_imm((img.width() - side) / 2, (img.height() - side) / 2, side, side)
        .resize_exact(SIZE, SIZE, image::imageops::FilterType::Triangle)
        .to_rgba8()
        .into_raw();
    let _ = std::fs::write(&path, &rgba);
    Some(rgba)
}

/// The big cover shown in the art-first layout is this many pixels square
pub const BIG: u32 = 300;

/// The address of a larger version of a cover. Google's image hosts take the size in the URL;
/// for anything else the video's own larger thumbnail is used.
pub fn big_url(cover_url: &str, id: &str) -> String {
    if allowed_url(cover_url) && cover_url.contains("googleusercontent.com") {
        if let Some((base, _)) = cover_url.split_once('=') {
            return format!("{base}=w{BIG}-h{BIG}-l90-rj");
        }
    }
    format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg")
}

/// Starts a worker for big covers. Send it `(video id, url)`; the answer is `Cmd::BigThumb`.
/// These are not cached on disk: only the song playing right now has one.
pub fn spawn_big(results: Sender<Cmd>) -> Sender<(String, String)> {
    let (tx, rx) = channel::<(String, String)>();
    std::thread::spawn(move || {
        let agent = ureq::Agent::new_with_defaults();
        for (id, url) in rx {
            let rgba = load_big(&agent, &url);
            if results.send(Cmd::BigThumb { id, rgba }).is_err() {
                break;
            }
        }
    });
    tx
}

fn load_big(agent: &ureq::Agent, url: &str) -> Option<Vec<u8>> {
    if !allowed_url(url) {
        return None;
    }
    let mut resp = agent.get(url).call().ok()?;
    let bytes = resp.body_mut().with_config().limit(3_000_000).read_to_vec().ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    let side = img.width().min(img.height());
    Some(
        img.crop_imm((img.width() - side) / 2, (img.height() - side) / 2, side, side)
            .resize_exact(BIG, BIG, image::imageops::FilterType::Triangle)
            .to_rgba8()
            .into_raw(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn big_covers_ask_google_for_a_bigger_size() {
        assert_eq!(big_url("https://lh3.googleusercontent.com/abc=w120-h120-l90-rj", "x"), "https://lh3.googleusercontent.com/abc=w300-h300-l90-rj");
        // a video thumbnail, an unknown host or no cover at all fall back to the video's own picture
        assert_eq!(big_url("https://i.ytimg.com/vi/abc/mqdefault.jpg", "abc"), "https://i.ytimg.com/vi/abc/hqdefault.jpg");
        assert_eq!(big_url("https://evil.example/x=1", "abc"), "https://i.ytimg.com/vi/abc/hqdefault.jpg");
        assert_eq!(big_url("", "abc"), "https://i.ytimg.com/vi/abc/hqdefault.jpg");
    }
}
