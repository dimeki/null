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
