//! Developer tool, built only with `--features gallery`:
//!
//!   cargo run --release --features gallery -- --gallery <folder>
//!
//! Fills the window with made-up data and writes a PNG of every screen, in every theme,
//! plus an index.html to look at them all together. It draws into memory, so no window
//! appears and nothing is clicked. Use it after changing the interface to see every screen at once.

use crate::{LyricLine, MainWindow, PartyMember, Theme, TrackItem};
use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, Image, ModelRc, PhysicalSize, Rgba8Pixel, SharedPixelBuffer, VecModel};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

const WIDTH: u32 = 980;
const HEIGHT: u32 = 620;

/// The folder after `--gallery`, when the program was started to make screenshots
pub fn requested() -> Option<PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--gallery" {
            return Some(PathBuf::from(args.next().unwrap_or_else(|| "gallery".into())));
        }
    }
    None
}

struct GalleryPlatform {
    window: Rc<MinimalSoftwareWindow>,
    /// Animations follow this clock, which the tool moves forward by hand
    clock: Rc<Cell<Duration>>,
}

impl Platform for GalleryPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.clock.get()
    }
}

fn cover(top: [u8; 3], bottom: [u8; 3]) -> Image {
    const N: u32 = 64;
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(N, N);
    for (i, px) in buf.make_mut_slice().iter_mut().enumerate() {
        let t = (i as u32 / N) as f32 / (N - 1) as f32;
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t) as u8;
        *px = Rgba8Pixel { r: mix(top[0], bottom[0]), g: mix(top[1], bottom[1]), b: mix(top[2], bottom[2]), a: 255 };
    }
    Image::from_rgba8(buf)
}

const ARTIST: &str = "UCRr1xG_2WIDs18a6cIiCxeA";

fn covers() -> Vec<Image> {
    vec![
        cover([70, 110, 190], [25, 40, 90]),
        cover([190, 70, 100], [90, 20, 45]),
        cover([70, 180, 120], [20, 85, 60]),
        cover([190, 170, 70], [90, 80, 25]),
        cover([130, 70, 190], [50, 25, 90]),
    ]
}

fn item(i: usize, title: &str, artist: &str, album: &str, dur: &str) -> TrackItem {
    let channel = if album.is_empty() { artist.to_string() } else { format!("{artist} · {album}") };
    TrackItem {
        id: format!("song{i}").into(),
        title: title.into(),
        channel: channel.into(),
        artist: artist.into(),
        rest: album.into(),
        artist_id: ARTIST.into(),
        duration: dur.into(),
        thumb: covers()[i % 5].clone(),
    }
}

fn songs() -> Vec<TrackItem> {
    vec![
        item(0, "Song title one", "Artist", "Album name", "3:42"),
        item(1, "Song title two", "Artist & Guest", "Album name", "4:05"),
        item(2, "Song title three", "Another artist", "Second album", "2:58"),
        item(3, "Song title four", "Artist", "Third album", "5:21"),
        item(4, "A song with a much longer title that has to be shortened to fit the row", "Artist", "Album", "3:10"),
        item(0, "Song title six", "Artist", "Album name", "4:44"),
        item(1, "Song title seven", "Another artist", "Single", "3:03"),
    ]
}

fn model(v: Vec<TrackItem>) -> ModelRc<TrackItem> {
    ModelRc::new(VecModel::from(v))
}

fn albums() -> Vec<TrackItem> {
    let a = |i: usize, t: &str, s: &str| TrackItem { id: format!("album{i}").into(), title: t.into(), channel: s.into(), thumb: covers()[i % 5].clone(), ..Default::default() };
    vec![a(0, "Album name", "Album • 2023"), a(1, "Second album", "Album • 2019"), a(2, "A single", "Single • 2023"), a(3, "Older album", "Album • 2013")]
}

fn playlists() -> Vec<TrackItem> {
    let p = |i: usize, t: &str, s: &str| TrackItem { id: format!("PL{i}").into(), title: t.into(), channel: s.into(), thumb: covers()[i % 5].clone(), ..Default::default() };
    vec![
        TrackItem { id: "LM".into(), title: "Liked songs".into(), channel: "Auto playlist".into(), ..Default::default() },
        p(1, "Gym playlist", "You • 38 tracks"),
        p(2, "Road trip", "You • 21 tracks"),
        p(3, "Focus", "You • 54 tracks"),
        p(4, "Evening", "You • 17 tracks"),
    ]
}

fn lyric_lines() -> Vec<LyricLine> {
    [
        "A line of the lyrics appears here",
        "and another one follows it",
        "The current line is brighter and larger",
        "The next line waits underneath",
        "Then another one after it",
        "And a few more further down",
        "Plain text when nothing is timed",
        "The last line of the song",
    ]
    .iter()
    .enumerate()
    .map(|(i, t)| LyricLine { text: (*t).into(), time: i as f32 * 12.0 })
    .collect()
}

/// Puts every property back to an ordinary "signed in, a song playing" state
fn base(ui: &MainWindow) {
    ui.set_tab(0);
    ui.set_settings_open(false);
    ui.set_artist_open(false);
    ui.set_artist_album(false);
    ui.set_artist_loading(false);
    ui.set_artist_note("".into());
    ui.set_in_playlist(false);
    ui.set_query("".into());
    ui.set_results(model(vec![]));
    ui.set_search_note("".into());
    ui.set_searching(false);
    ui.set_queue(model(songs().into_iter().take(4).collect()));
    ui.set_current_index(0);
    ui.set_now_title("Song title one".into());
    ui.set_now_detail("Artist · Album name".into());
    ui.set_now_artist("Artist".into());
    ui.set_now_rest("Album name".into());
    ui.set_now_artist_id(ARTIST.into());
    ui.set_now_cover(covers()[0].clone());
    ui.set_active(true);
    ui.set_paused(false);
    ui.set_loading(false);
    ui.set_shuffle(false);
    ui.set_repeat_mode(0);
    ui.set_pos_text("1:12".into());
    ui.set_dur_text("3:42".into());
    ui.set_duration_secs(222.0);
    ui.set_progress(0.32);
    ui.set_volume(0.8);
    ui.set_signed_in(true);
    ui.set_playlists(model(playlists()));
    ui.set_playlist_tracks(model(vec![]));
    ui.set_playlist_title("".into());
    ui.set_playlist_loading(false);
    ui.set_lyrics_lines(ModelRc::new(VecModel::from(lyric_lines())));
    ui.set_lyrics_synced(true);
    ui.set_lyrics_current(2);
    ui.set_lyrics_source("Lyrics from LRCLIB".into());
    ui.set_lyrics_loading(false);
    ui.set_lyrics_note("".into());
    ui.set_perf_mode(false);
    ui.set_cache_text("13 MB of downloaded songs".into());
    ui.set_ytdlp_version("2026.10.21".into());
    ui.set_party_mode(0);
    ui.set_party_connecting(false);
    ui.set_party_room("".into());
    ui.set_party_note("".into());
    ui.set_party_control(false);
    ui.set_controls_locked(false);
    ui.set_app_version("1.0.3".into());
    ui.set_update_available(false);
    ui.set_party_server_ready(true);
    ui.set_party_server_built_in(true);
    ui.set_display_name("Giovanni".into());
}

fn members(guests: &[(&str, &str)]) -> ModelRc<PartyMember> {
    let mut v = vec![PartyMember { name: "Giovanni".into(), initial: "G".into(), tag: "Host".into() }];
    for (name, tag) in guests {
        v.push(PartyMember { name: (*name).into(), initial: name.chars().next().unwrap_or('?').to_string().into(), tag: (*tag).into() });
    }
    ModelRc::new(VecModel::from(v))
}

type Scene = (&'static str, fn(&MainWindow));

fn scenes() -> Vec<Scene> {
    vec![
        ("search", |ui| {
            ui.set_query("artist name".into());
            ui.set_results(model(songs()));
        }),
        ("search-empty", |_| {}),
        ("search-update-banner", |ui| {
            ui.set_query("artist name".into());
            ui.set_results(model(songs()));
            ui.set_update_available(true);
            ui.set_update_version("1.0.4".into());
            ui.set_update_notes("A short line about what changed".into());
            ui.set_update_host("github.com".into());
        }),
        ("library", |ui| ui.set_tab(1)),
        ("library-signed-out", |ui| {
            ui.set_tab(1);
            ui.set_signed_in(false);
        }),
        ("library-playlist", |ui| {
            ui.set_tab(1);
            ui.set_in_playlist(true);
            ui.set_playlist_title("Gym playlist".into());
            ui.set_playlist_tracks(model(songs()));
        }),
        ("artist", |ui| {
            ui.set_artist_open(true);
            ui.set_artist_name("Artist name".into());
            ui.set_artist_sub("67.8M monthly audience".into());
            ui.set_artist_image(covers()[0].clone());
            ui.set_playlist_tracks(model(songs().into_iter().take(5).collect()));
            ui.set_artist_albums(model(albums()));
        }),
        ("album", |ui| {
            ui.set_artist_open(true);
            ui.set_artist_album(true);
            ui.set_artist_name("Album name".into());
            ui.set_artist_sub("Artist · Album • 2023".into());
            ui.set_artist_image(covers()[1].clone());
            ui.set_playlist_tracks(model(songs()));
        }),
        ("lyrics", |ui| ui.set_tab(2)),
        ("lyrics-none", |ui| {
            ui.set_tab(2);
            ui.set_lyrics_lines(ModelRc::new(VecModel::from(Vec::<LyricLine>::new())));
            ui.set_lyrics_note("No lyrics found for this song".into());
        }),
        ("party", |ui| ui.set_tab(3)),
        ("party-host", |ui| {
            ui.set_tab(3);
            ui.set_party_mode(1);
            ui.set_party_room("KZDWT6".into());
            ui.set_party_control(true);
            ui.set_party_members(members(&[("Guest one", "In sync"), ("Guest two", "Catching up…")]));
        }),
        ("party-guest", |ui| {
            ui.set_tab(3);
            ui.set_party_mode(2);
            ui.set_party_room("KZDWT6".into());
            ui.set_party_host_name("Giovanni".into());
            ui.set_party_sync("In sync".into());
            ui.set_controls_locked(true);
            ui.set_party_members(members(&[("Guest one", "In sync"), ("You", "In sync")]));
        }),
        ("settings", |ui| {
            ui.set_settings_open(true);
            ui.set_update_available(true);
            ui.set_update_version("1.0.4".into());
            ui.set_update_host("github.com".into());
        }),
        ("nothing-playing", |ui| {
            ui.set_queue(model(vec![]));
            ui.set_current_index(-1);
            ui.set_now_title("Nothing playing".into());
            ui.set_now_detail("".into());
            ui.set_now_artist("".into());
            ui.set_now_rest("".into());
            ui.set_now_artist_id("".into());
            ui.set_now_cover(Image::default());
            ui.set_active(false);
            ui.set_pos_text("0:00".into());
            ui.set_dur_text("0:00".into());
            ui.set_progress(0.0);
        }),
    ]
}

pub fn run(out: PathBuf) -> Result<(), slint::PlatformError> {
    let fail = |e: String| slint::PlatformError::Other(e);
    std::fs::create_dir_all(&out).map_err(|e| fail(format!("couldn't create {}: {e}", out.display())))?;

    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let clock = Rc::new(Cell::new(Duration::ZERO));
    slint::platform::set_platform(Box::new(GalleryPlatform { window: window.clone(), clock: clock.clone() }))
        .map_err(|e| fail(format!("couldn't set up the renderer: {e}")))?;

    let ui = MainWindow::new()?;
    window.set_size(PhysicalSize::new(WIDTH, HEIGHT));
    ui.show()?;

    let mut shots: Vec<(String, String)> = Vec::new();
    for (palette, scheme_name) in [(1, "dark"), (2, "light"), (3, "slate"), (4, "ember"), (5, "paper"), (6, "black")] {
        ui.global::<Theme>().set_palette(palette);
        for (n, (name, setup)) in scenes().iter().enumerate() {
            base(&ui);
            setup(&ui);
            let file = format!("{scheme_name}-{:02}-{name}.png", n + 1);
            snapshot(&window, &clock, &out.join(&file)).map_err(fail)?;
            shots.push((scheme_name.to_string(), file));
        }
    }
    write_index(&out, &shots).map_err(|e| fail(format!("couldn't write index.html: {e}")))?;
    println!("Wrote {} screenshots to {}", shots.len(), out.display());
    Ok(())
}

/// Lets animations finish, draws the window into memory and saves it
fn snapshot(window: &Rc<MinimalSoftwareWindow>, clock: &Cell<Duration>, path: &Path) -> Result<(), String> {
    let mut pixels = vec![PremultipliedRgbaColor::default(); (WIDTH * HEIGHT) as usize];
    for _ in 0..3 {
        clock.set(clock.get() + Duration::from_secs(2));
        slint::platform::update_timers_and_animations();
        window.draw_if_needed(|renderer| {
            renderer.render(&mut pixels, WIDTH as usize);
        });
    }
    let mut bytes = Vec::with_capacity(pixels.len() * 4);
    for p in &pixels {
        bytes.extend_from_slice(&[p.red, p.green, p.blue, 255]);
    }
    let img = image::RgbaImage::from_raw(WIDTH, HEIGHT, bytes).ok_or("the screenshot had the wrong size")?;
    img.save(path).map_err(|e| format!("couldn't save {}: {e}", path.display()))
}

fn write_index(out: &Path, shots: &[(String, String)]) -> std::io::Result<()> {
    let mut html = String::from(
        "<!doctype html><meta charset=utf-8><title>ytmusic-lite screens</title><style>body{font:14px Segoe UI,sans-serif;background:#111;color:#ddd;margin:20px}\
         h2{margin:28px 0 10px}.g{display:grid;grid-template-columns:repeat(auto-fill,minmax(480px,1fr));gap:16px}figure{margin:0}img{width:100%;border-radius:8px;border:1px solid #333}\
         figcaption{margin:4px 2px;color:#999}</style>",
    );
    let mut names: Vec<&str> = shots.iter().map(|(s, _)| s.as_str()).collect();
    names.dedup();
    for scheme in names {
        html.push_str(&format!("<h2>{scheme}</h2><div class=g>"));
        for (s, file) in shots.iter().filter(|(s, _)| s == scheme) {
            let label = file.trim_end_matches(".png").trim_start_matches(&format!("{s}-"));
            html.push_str(&format!("<figure><a href=\"{file}\"><img src=\"{file}\" loading=lazy></a><figcaption>{label}</figcaption></figure>"));
        }
        html.push_str("</div>");
    }
    std::fs::write(out.join("index.html"), html)
}

