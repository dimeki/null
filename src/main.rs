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

fn main() -> Result<(), slint::PlatformError> {
    let ui = MainWindow::new()?;
    let (tx, rx) = mpsc::channel::<Cmd>();

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
    ui.on_party_start(move || {
        let _ = t.send(Cmd::PartyHost);
    });
    let t = tx.clone();
    ui.on_party_join(move |code| {
        let _ = t.send(Cmd::PartyJoin(code.to_string()));
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
    ui.window().on_close_requested(move || {
        let _ = t.send(Cmd::Visible(false));
        slint::CloseRequestResponse::HideWindow
    });

    media::init(tx.clone());
    let _tray = tray::create(ui.as_weak(), tx.clone());

    let weak = ui.as_weak();
    std::thread::spawn(move || engine::run(rx, tx, weak, vendor, cache));

    ui.show()?;
    slint::run_event_loop_until_quit()
}
