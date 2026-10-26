// no console window in release builds; this is a tray app
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod cookies;
mod engine;
mod innertube;
mod login;
mod lyrics;
mod media;
mod mpv;
mod party;
mod state;
mod thumbs;
mod tray;
mod update;
mod ytdlp;

use engine::Cmd;
use slint::ComponentHandle;
use std::path::PathBuf;
use std::sync::mpsc;

slint::include_modules!();

const CACHE_LIMIT_BYTES: u64 = 300 * 1024 * 1024;

fn vendor_dir() -> PathBuf {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf()));
    for base in [exe_dir, std::env::current_dir().ok()].into_iter().flatten() {
        for rel in [".", "vendor", "../../vendor"] {
            let d = base.join(rel);
            if d.join("libmpv-2.dll").exists() {
                return d;
            }
        }
    }
    PathBuf::from("vendor")
}

fn cache_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("ytmusic-lite")
        .join("cache")
}

/// Where the window size and the open tab are kept between runs
fn window_file() -> PathBuf {
    cache_dir().with_file_name("window.txt")
}

/// "996 659 1" -> width, height, tab. Sizes outside what a window can sensibly be are ignored,
/// so a damaged file never opens a window that is tiny or off the screen.
fn parse_window(text: &str) -> Option<(u32, u32, i32)> {
    let mut parts = text.split_whitespace();
    let w: u32 = parts.next()?.parse().ok()?;
    let h: u32 = parts.next()?.parse().ok()?;
    let tab: i32 = parts.next().and_then(|t| t.parse().ok()).unwrap_or(0);
    ((600..=7680).contains(&w) && (400..=4320).contains(&h)).then_some((w, h, tab.clamp(0, 3)))
}

fn save_window(ui: &MainWindow) {
    let size = ui.window().size();
    // a minimized window reports no size; keep what was saved
    if size.width == 0 || size.height == 0 {
        return;
    }
    let _ = std::fs::write(window_file(), format!("{} {} {}\n", size.width, size.height, ui.get_tab()));
}

fn main() -> Result<(), slint::PlatformError> {
    let ui = MainWindow::new()?;
    let (tx, rx) = mpsc::channel::<Cmd>();

    if let Some((w, h, tab)) = std::fs::read_to_string(window_file()).ok().and_then(|t| parse_window(&t)) {
        ui.window().set_size(slint::PhysicalSize::new(w, h));
        ui.set_tab(tab);
        match tab {
            1 => {
                let _ = tx.send(Cmd::OpenLibrary);
            }
            2 => {
                let _ = tx.send(Cmd::ViewChanged(2));
            }
            _ => {}
        }
    }

    let t = tx.clone();
    ui.on_search(move |q| {
        let _ = t.send(Cmd::Search(q.to_string()));
    });
    let t = tx.clone();
    ui.on_play_result(move |i| {
        let _ = t.send(Cmd::PlayResult(i as usize));
    });
    let t = tx.clone();
    ui.on_add_result(move |i| {
        let _ = t.send(Cmd::AddResult(i as usize));
    });
    let t = tx.clone();
    ui.on_play_queue(move |i| {
        let _ = t.send(Cmd::PlayQueue(i as usize));
    });
    let t = tx.clone();
    ui.on_remove_queue(move |i| {
        let _ = t.send(Cmd::RemoveQueue(i as usize));
    });
    let t = tx.clone();
    ui.on_clear_queue(move || {
        let _ = t.send(Cmd::ClearQueue);
    });
    let t = tx.clone();
    ui.on_toggle_pause(move || {
        let _ = t.send(Cmd::TogglePause);
    });
    let t = tx.clone();
    ui.on_next(move || {
        let _ = t.send(Cmd::Next);
    });
    let t = tx.clone();
    ui.on_prev(move || {
        let _ = t.send(Cmd::Prev);
    });
    let t = tx.clone();
    ui.on_seek(move |f| {
        let _ = t.send(Cmd::SeekFraction(f));
    });
    let t = tx.clone();
    ui.on_seek_by(move |secs| {
        let _ = t.send(Cmd::SeekBy(secs as f64));
    });
    let t = tx.clone();
    ui.on_set_volume(move |v| {
        let _ = t.send(Cmd::SetVolume(v));
    });
    let t = tx.clone();
    ui.on_toggle_shuffle(move || {
        let _ = t.send(Cmd::ToggleShuffle);
    });
    let t = tx.clone();
    ui.on_cycle_repeat(move || {
        let _ = t.send(Cmd::CycleRepeat);
    });

    let (vendor, cache) = (vendor_dir(), cache_dir());
    ytdlp::prune_cache(&cache, CACHE_LIMIT_BYTES);
    let t = tx.clone();
    ui.on_prefetch_result(move |i| {
        let _ = t.send(Cmd::PrefetchResult(i as usize));
    });
    let t = tx.clone();
    ui.on_prefetch_playlist_track(move |i| {
        let _ = t.send(Cmd::PrefetchPlaylistTrack(i as usize));
    });
    let t = tx.clone();
    ui.on_prefetch_queue(move |i| {
        let _ = t.send(Cmd::PrefetchQueue(i as usize));
    });
    ui.on_copy_text(move |text| {
        // clipboard failures are harmless: the code is still on screen to read out
        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            let _ = clipboard.set_text(text.to_string());
        }
    });
    let t = tx.clone();
    ui.on_check_update(move || {
        let _ = t.send(Cmd::CheckUpdate);
    });
    let t = tx.clone();
    ui.on_open_update(move || {
        let _ = t.send(Cmd::OpenUpdate);
    });
    let t = tx.clone();
    ui.on_set_auto_update(move |on| {
        let _ = t.send(Cmd::SetAutoUpdate(on));
    });
    let t = tx.clone();
    ui.on_move_queue(move |from, to| {
        let _ = t.send(Cmd::MoveQueue { from: from.max(0) as usize, to: to.max(0) as usize });
    });
    let t = tx.clone();
    ui.on_play_next_result(move |i| {
        let _ = t.send(Cmd::PlayNextResult(i.max(0) as usize));
    });
    let t = tx.clone();
    ui.on_play_next_playlist_track(move |i| {
        let _ = t.send(Cmd::PlayNextPlaylistTrack(i.max(0) as usize));
    });
    let t = tx.clone();
    ui.on_play_next_queue(move |i| {
        let _ = t.send(Cmd::PlayNextQueue(i.max(0) as usize));
    });
    let t = tx.clone();
    ui.on_open_track_web(move |id| {
        let _ = t.send(Cmd::OpenTrackWeb(id.to_string()));
    });
    let t = tx.clone();
    ui.on_open_artist(move |id| {
        let _ = t.send(Cmd::OpenArtist(id.to_string()));
    });
    let t = tx.clone();
    ui.on_open_album(move |i| {
        let _ = t.send(Cmd::OpenAlbum(i as usize));
    });
    let t = tx.clone();
    ui.on_artist_back(move || {
        let _ = t.send(Cmd::ArtistBack);
    });
    let t = tx.clone();
    ui.on_close_artist(move || {
        let _ = t.send(Cmd::CloseArtist);
    });
    let t = tx.clone();
    ui.on_party_start(move || {
        let _ = t.send(Cmd::PartyHost);
    });
    let t = tx.clone();
    ui.on_party_join(move |code| {
        let _ = t.send(Cmd::PartyJoin(code.to_string()));
    });
    let t = tx.clone();
    ui.on_party_control_changed(move |on| {
        let _ = t.send(Cmd::PartyControl(on));
    });
    let t = tx.clone();
    ui.on_party_leave(move || {
        let _ = t.send(Cmd::PartyLeave);
    });
    let t = tx.clone();
    ui.on_party_server_edited(move |address| {
        let _ = t.send(Cmd::SetPartyServer(address.to_string()));
    });
    let t = tx.clone();
    ui.on_display_name_edited(move |name| {
        let _ = t.send(Cmd::SetDisplayName(name.to_string()));
    });
    let t = tx.clone();
    ui.on_view_changed(move |view| {
        let _ = t.send(Cmd::ViewChanged(view));
    });
    let t = tx.clone();
    ui.on_set_perf(move |on| {
        let _ = t.send(Cmd::SetPerf(on));
    });
    let t = tx.clone();
    ui.on_clear_cache(move || {
        let _ = t.send(Cmd::ClearCache);
    });
    let t = tx.clone();
    ui.on_update_ytdlp(move || {
        let _ = t.send(Cmd::UpdateYtdlp);
    });
    let t = tx.clone();
    ui.on_refresh_library(move || {
        let _ = t.send(Cmd::RefreshLibrary);
    });
    let t = tx.clone();
    ui.on_seek_lyric(move |secs| {
        let _ = t.send(Cmd::SeekSecs(secs as f64));
    });
    let t = tx.clone();
    ui.on_open_library(move || {
        let _ = t.send(Cmd::OpenLibrary);
    });
    let t = tx.clone();
    ui.on_open_playlist(move |i| {
        let _ = t.send(Cmd::OpenPlaylist(i as usize));
    });
    let t = tx.clone();
    ui.on_play_playlist_track(move |i| {
        let _ = t.send(Cmd::PlayPlaylistTrack(i as usize));
    });
    let t = tx.clone();
    ui.on_add_playlist_track(move |i| {
        let _ = t.send(Cmd::AddPlaylistTrack(i as usize));
    });
    let t = tx.clone();
    ui.on_play_all(move || {
        let _ = t.send(Cmd::PlayAll);
    });
    let t = tx.clone();
    ui.on_add_all(move || {
        let _ = t.send(Cmd::AddAll);
    });
    let t = tx.clone();
    ui.on_sign_in(move |input| {
        let _ = t.send(Cmd::SignIn(input.to_string()));
    });
    let (t, weak, dest) = (tx.clone(), ui.as_weak(), cache_dir().with_file_name("cookies.txt"));
    ui.on_sign_in_google(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_signing_in(true);
            ui.set_auth_note("".into());
        }
        login::open(t.clone(), dest.clone());
    });
    let t = tx.clone();
    ui.on_sign_out(move || {
        let _ = t.send(Cmd::SignOut);
    });

    // closing the window hides it to the tray; "Quit" in the tray menu ends the app
    let t = tx.clone();
    let weak_ui = ui.as_weak();
    ui.window().on_close_requested(move || {
        if let Some(ui) = weak_ui.upgrade() {
            save_window(&ui);
        }
        let _ = t.send(Cmd::Visible(false));
        slint::CloseRequestResponse::HideWindow
    });

    media::init(tx.clone());
    let _tray = tray::create(ui.as_weak(), tx.clone());

    let weak = ui.as_weak();
    std::thread::spawn(move || engine::run(rx, tx, weak, vendor, cache));

    // Windows can discard what the window last drew (long idle, sleep, display off), and the
    // app only redraws what changed, so hovering would bring the window back piece by piece.
    // A full redraw every couple of seconds while it is on screen repairs that quickly.
    let repaint = slint::Timer::default();
    let weak_ui = ui.as_weak();
    let mut ticks = 0i32;
    repaint.start(slint::TimerMode::Repeated, std::time::Duration::from_secs(2), move || {
        let Some(ui) = weak_ui.upgrade() else { return };
        ticks = ticks.wrapping_add(1);
        // performance saving trades a slower repair for less work
        if ui.window().is_visible() && (!ui.get_perf_mode() || ticks % 3 == 0) {
            ui.set_repaint_nonce(ui.get_repaint_nonce().wrapping_add(1));
        }
    });

    ui.show()?;
    let result = slint::run_event_loop_until_quit();
    // Quit from the tray menu ends the loop without a close request
    save_window(&ui);
    result
}

#[cfg(test)]
mod tests {
    use super::parse_window;

    #[test]
    fn the_saved_window_is_read_back_and_nonsense_is_ignored() {
        assert_eq!(parse_window("996 659 1\n"), Some((996, 659, 1)));
        assert_eq!(parse_window("1200 800"), Some((1200, 800, 0)), "a file from before tabs were saved");
        assert_eq!(parse_window("1200 800 9"), Some((1200, 800, 3)), "an unknown tab is clamped");
        for bad in ["", "abc", "100 100 0", "99999 800 0", "800 -5 0", "800"] {
            assert_eq!(parse_window(bad), None, "{bad:?}");
        }
    }
}
