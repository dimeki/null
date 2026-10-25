//! Windows media overlay and media keys through the System Media Transport
//! Controls (SMTC). Everything here runs on the UI thread.

use crate::engine::Cmd;
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig};
use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::mpsc::Sender;
use std::time::Duration;

thread_local! {
    static CONTROLS: RefCell<Option<MediaControls>> = const { RefCell::new(None) };
    static SHOWN_TRACK: RefCell<Option<String>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy, PartialEq)]
pub enum State {
    Stopped,
    Playing,
    Paused,
}

pub struct NowPlaying {
    pub id: String,
    pub title: String,
    pub artist: String,
    /// Cover art URL; empty falls back to the video thumbnail
    pub cover: String,
    pub duration_secs: u32,
    pub position_secs: f64,
    pub state: State,
}

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        name: *const u16,
        style: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        parent: isize,
        menu: isize,
        instance: isize,
        param: *const c_void,
    ) -> isize;
}

/// SMTC attaches to a window handle. The real window can be hidden to the tray,
/// so this uses its own never-shown window that lives as long as the process.
#[cfg(windows)]
fn hidden_window() -> Option<isize> {
    let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
    let name: Vec<u16> = "ytmusic-lite media\0".encode_utf16().collect();
    let hwnd = unsafe { CreateWindowExW(0, class.as_ptr(), name.as_ptr(), 0, 0, 0, 0, 0, 0, 0, 0, std::ptr::null()) };
    (hwnd != 0).then_some(hwnd)
}

#[cfg(windows)]
pub fn init(tx: Sender<Cmd>) {
    let Some(hwnd) = hidden_window() else { return };
    let config = PlatformConfig { dbus_name: "ytmusic_lite", display_name: "ytmusic-lite", hwnd: Some(hwnd as *mut c_void) };
    let Ok(mut controls) = MediaControls::new(config) else { return };
    let attached = controls.attach(move |event| {
        let cmd = match event {
            MediaControlEvent::Play => Cmd::Resume,
            MediaControlEvent::Pause => Cmd::PausePlayback,
            MediaControlEvent::Toggle => Cmd::TogglePause,
            MediaControlEvent::Next => Cmd::Next,
            MediaControlEvent::Previous => Cmd::Prev,
            MediaControlEvent::Stop => Cmd::Stop,
            MediaControlEvent::SetPosition(MediaPosition(at)) => Cmd::SeekSecs(at.as_secs_f64()),
            _ => return,
        };
        let _ = tx.send(cmd);
    });
    if attached.is_ok() {
        CONTROLS.with(|c| *c.borrow_mut() = Some(controls));
    }
}

#[cfg(not(windows))]
pub fn init(_tx: Sender<Cmd>) {}

pub fn update(np: &NowPlaying) {
    CONTROLS.with(|c| {
        let mut c = c.borrow_mut();
        let Some(controls) = c.as_mut() else { return };
        let new_track = SHOWN_TRACK.with(|s| {
            let mut s = s.borrow_mut();
            let changed = np.state != State::Stopped && s.as_deref() != Some(np.id.as_str());
            if changed {
                *s = Some(np.id.clone());
            }
            changed
        });
        if new_track {
            let cover = if np.cover.starts_with("https://") {
                np.cover.clone()
            } else {
                format!("https://i.ytimg.com/vi/{}/mqdefault.jpg", np.id)
            };
            let _ = controls.set_metadata(MediaMetadata {
                title: Some(&np.title),
                artist: Some(&np.artist),
                album: None,
                cover_url: Some(&cover),
                duration: Some(Duration::from_secs(np.duration_secs as u64)),
            });
        }
        let progress = Some(MediaPosition(Duration::from_secs_f64(np.position_secs.max(0.0))));
        let _ = controls.set_playback(match np.state {
            State::Stopped => MediaPlayback::Stopped,
            State::Playing => MediaPlayback::Playing { progress },
            State::Paused => MediaPlayback::Paused { progress },
        });
    });
}
