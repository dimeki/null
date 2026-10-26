use crate::cookies;
use crate::innertube::{AlbumInfo, ArtistPage, Client, PlaylistInfo};
use crate::lyrics::{self, Lyrics};
use crate::mpv::{Mpv, PlayerEvent};
use crate::party::{self, Act, ActKind, Correction, HostState, Out as PartyOut, TrackInfo};
use crate::update;
use crate::ytdlp::{Track, YtDlp};
use crate::{AppState, LyricLine, MainWindow, PartyMember, Theme, TrackItem};
use slint::{ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel, Weak};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

type Thumbs = HashMap<String, Arc<Vec<u8>>>;

#[cfg(windows)]
mod os {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32EmptyWorkingSet(process: isize) -> i32;
    }

    /// Hands resident pages back to Windows. Loading libmpv reads most of its
    /// 128 MB DLL into the working set once; steady playback only needs ~13 MB.
    pub fn trim_memory() {
        unsafe {
            K32EmptyWorkingSet(GetCurrentProcess());
        }
    }
}
#[cfg(not(windows))]
mod os {
    pub fn trim_memory() {}
}

pub enum Cmd {
    Search(String),
    SearchDone(Result<Vec<Track>, String>),
    PlayResult(usize),
    AddResult(usize),
    PlayQueue(usize),
    RemoveQueue(usize),
    /// Drag a queue row from one position to another
    MoveQueue { from: usize, to: usize },
    /// Right-click menu: queue this song right after the current one
    PlayNextResult(usize),
    PlayNextPlaylistTrack(usize),
    /// Right-click menu on a queue row: move it to right after the current one
    PlayNextQueue(usize),
    /// Right-click menu: open the song's page on YouTube Music in the browser
    OpenTrackWeb(String),
    ClearQueue,
    TogglePause,
    Next,
    Prev,
    SeekFraction(f32),
    SetVolume(f32),
    ToggleShuffle,
    CycleRepeat,
    Fetched { id: String, result: Result<PathBuf, String> },
    Thumb { id: String, rgba: Option<Vec<u8>> },
    /// The large cover for the song playing (art-first layout)
    BigThumb { id: String, rgba: Option<Vec<u8>> },
    /// The mouse is resting on a row: start getting its audio so a click plays sooner
    PrefetchResult(usize),
    PrefetchPlaylistTrack(usize),
    PrefetchQueue(usize),
    /// Media key or overlay "play"
    Resume,
    /// Media key or overlay "pause"
    PausePlayback,
    Stop,
    SeekSecs(f64),
    /// Keyboard seek: this many seconds forward (or back, when negative)
    SeekBy(f64),
    /// The window was shown (true) or hidden to the tray (false)
    Visible(bool),
    Quit,
    OpenLibrary,
    LibraryLoaded(Result<Vec<PlaylistInfo>, String>),
    OpenPlaylist(usize),
    PlaylistLoaded { id: String, result: Result<Vec<Track>, String> },
    /// Show an artist's page (the id is the "UC…" page id)
    OpenArtist(String),
    ArtistLoaded { id: String, result: Result<ArtistPage, String> },
    /// Open one of the albums or singles listed on the artist page
    OpenAlbum(usize),
    AlbumLoaded { id: String, result: Result<Vec<Track>, String> },
    /// Back from an album to the artist, or from the artist to where the user was
    ArtistBack,
    CloseArtist,
    PlayPlaylistTrack(usize),
    AddPlaylistTrack(usize),
    PlayAll,
    AddAll,
    /// A cookies.txt path or a browser name
    SignIn(String),
    SignInDone(Result<(), String>),
    SignOut,
    /// Which view is showing: 0 search, 1 library, 2 lyrics, -1 settings
    ViewChanged(i32),
    LyricsLoaded { id: String, result: Result<Option<Lyrics>, String> },
    SetPerf(bool),
    ClearCache,
    UpdateYtdlp,
    YtdlpVersion(String),
    YtdlpUpdated(Result<String, String>),
    RefreshLibrary,
    PartyHost,
    PartyJoin(String),
    PartyLeave,
    /// The host lets guests control playback, or stops letting them
    PartyControl(bool),
    Party(party::Event),
    SetPartyServer(String),
    SetDisplayName(String),
    /// The user asked to look for a newer version
    CheckUpdate,
    UpdateChecked { manual: bool, result: Result<Option<update::Release>, String> },
    OpenUpdate,
    SetAutoUpdate(bool),
    /// The user picked a colour theme (0 follows Windows)
    SetTheme(i32),
    /// The user picked a layout (0 tabs, 1 sidebar, 2 art first)
    SetLayout(i32),
    /// The user switched the floating player on or off (sidebar layout)
    SetFloatingPlayer(bool),
}

struct ArtistView {
    id: String,
    page: ArtistPage,
}

#[derive(Clone, Copy, PartialEq)]
enum PartyRole {
    Host,
    Guest,
}

struct PartyState {
    role: PartyRole,
    handle: party::Handle,
    /// The relay confirmed the room (created or joined)
    ready: bool,
    room: String,
    host_name: String,
    members: Vec<party::Member>,
    note: String,
    /// Latest state from the host (guests only)
    last_host: Option<HostState>,
    /// What the host last announced, so a new announcement is only made when it changes (host only)
    signature: String,
    last_sent: Instant,
    /// Send the next state right away (a seek, or a guest just joined)
    force: bool,
    /// The guest's own queue, put back when they leave
    saved_queue: Option<(Vec<Track>, Option<usize>)>,
    sync_text: String,
    last_check: Instant,
    /// Shared control: the host allows guests to play, pause, skip, seek and add songs
    control: bool,
}

/// Commands a guest cannot give: the host steers playback
fn is_playback_control(cmd: &Cmd) -> bool {
    matches!(
        cmd,
        Cmd::PlayResult(_)
            | Cmd::AddResult(_)
            | Cmd::PlayPlaylistTrack(_)
            | Cmd::AddPlaylistTrack(_)
            | Cmd::PlayAll
            | Cmd::AddAll
            | Cmd::PlayQueue(_)
            | Cmd::RemoveQueue(_)
            | Cmd::MoveQueue { .. }
            | Cmd::PlayNextResult(_)
            | Cmd::PlayNextPlaylistTrack(_)
            | Cmd::PlayNextQueue(_)
            | Cmd::ClearQueue
            | Cmd::TogglePause
            | Cmd::Resume
            | Cmd::PausePlayback
            | Cmd::Stop
            | Cmd::Next
            | Cmd::Prev
            | Cmd::SeekFraction(_)
            | Cmd::SeekSecs(_)
            | Cmd::SeekBy(_)
            | Cmd::ToggleShuffle
            | Cmd::CycleRepeat
    )
}

/// Moves the item at `from` so it ends up at `to`, keeping `cur` on the same song.
/// Returns false when nothing changed.
fn move_in_queue<T>(queue: &mut Vec<T>, cur: &mut Option<usize>, from: usize, to: usize) -> bool {
    if from >= queue.len() || to >= queue.len() || from == to {
        return false;
    }
    let item = queue.remove(from);
    queue.insert(to, item);
    *cur = cur.map(|c| {
        if c == from {
            to
        } else if from < c && to >= c {
            c - 1
        } else if from > c && to <= c {
            c + 1
        } else {
            c
        }
    });
    true
}

/// "myrelay.example.dev" and "https://..." both become a websocket address
fn normalize_server(input: &str) -> String {
    let s = input.trim();
    if let Some(rest) = s.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = s.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if s.is_empty() || s.contains("://") {
        s.to_string()
    } else {
        format!("wss://{s}")
    }
}

fn clean_room_code(input: &str) -> String {
    input.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_uppercase()
}

#[derive(Clone, Copy, PartialEq)]
enum Repeat {
    Off,
    All,
    One,
}

struct Engine {
    mpv: Mpv,
    ytdlp: YtDlp,
    cache_dir: PathBuf,
    tx: Sender<Cmd>,
    ui: Weak<MainWindow>,
    results: Vec<Track>,
    queue: Vec<Track>,
    cur: Option<usize>,
    /// Id of the track we are waiting on a download for
    loading: Option<String>,
    in_flight: HashSet<String>,
    /// A file is loaded in mpv
    playing: bool,
    paused: bool,
    shuffle: bool,
    repeat: Repeat,
    skip_chain: usize,
    status: String,
    rng: u64,
    /// Decoded covers, kept only for tracks currently in the results or queue
    thumbs: Thumbs,
    thumb_pending: HashSet<String>,
    thumb_tx: Sender<(String, String)>,
    big_tx: Sender<(String, String)>,
    /// Song whose big cover is shown or being fetched
    big_for: Option<String>,
    trim_at: Option<Instant>,
    client: Client,
    cookie_path: PathBuf,
    signed_in: bool,
    playlists: Vec<PlaylistInfo>,
    library_loading: bool,
    /// Id of the playlist whose tracks are shown (or loading)
    playlist_id: String,
    playlist_tracks: Vec<Track>,
    /// The artist page being shown. While it is open, `playlist_tracks` holds its top
    /// songs, or the songs of `album_open`, so Play all, Add all and the row buttons work as in a playlist.
    artist: Option<ArtistView>,
    /// Artist (and album) whose page is loading; an answer for any other is ignored
    artist_wanted: String,
    album_open: Option<AlbumInfo>,
    album_wanted: String,
    state_path: PathBuf,
    /// Set by `sync` whenever something worth saving changed
    dirty: Cell<bool>,
    last_save: Instant,
    /// Window is on screen (false while hidden in the tray)
    visible: bool,
    volume: f32,
    /// Position restored from the last run, shown until the track is played
    resume_pos: Option<f64>,
    /// Where the track being loaded should start
    pending_start: Option<f64>,
    quit: bool,
    /// Performance saving: no covers, slower progress updates
    perf: bool,
    settings_open: bool,
    ytdlp_version: String,
    ytdlp_updating: bool,
    /// The lyrics view is showing
    lyrics_wanted: bool,
    /// Video id the lyrics on screen (or being fetched) belong to
    lyrics_for: Option<String>,
    lyrics: Option<Lyrics>,
    lyrics_cur: i32,
    party: Option<PartyState>,
    party_server: String,
    display_name: String,
    /// Shown in the party panel when there is no party (why the last one ended, or what's missing)
    party_note: String,
    /// A newer version, once one has been found
    update: Option<update::Release>,
    update_checking: bool,
    update_note: String,
    auto_update: bool,
    /// Colour theme: 0 follows Windows, then the palettes in ui/theme.slint
    theme: i32,
    /// Layout: 0 tabs along the top, 1 a sidebar, 2 art first
    layout: i32,
    /// Sidebar layout only: the player floats as a pill
    floating_player: bool,
    /// When the next automatic check is due (none while automatic checks are off)
    next_update_check: Option<Instant>,
}

pub fn run(rx: Receiver<Cmd>, tx: Sender<Cmd>, ui: Weak<MainWindow>, vendor: PathBuf, cache_dir: PathBuf) {
    let mpv = match Mpv::new(&vendor.join("libmpv-2.dll")) {
        Ok(m) => m,
        Err(e) => {
            let _ = ui.upgrade_in_event_loop(move |ui| ui.global::<AppState>().set_search_note(e.into()));
            return;
        }
    };
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1)
        | 1;
    let state_path = cache_dir.with_file_name("state.txt");
    let cookie_path = cache_dir.with_file_name("cookies.txt");
    // temp cookie copies are normally deleted right after each yt-dlp run; clear any a crash left behind
    if let Ok(entries) = std::fs::read_dir(cookie_path.parent().unwrap_or(&cache_dir)) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if (name.starts_with("run-") && name.ends_with(".tmp"))
                || name.ends_with(".browser.tmp")
                || name.ends_with(".login.tmp")
            {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let thumb_dir = cache_dir.with_file_name("thumbs");
    crate::ytdlp::prune_cache(&thumb_dir, 50 * 1024 * 1024);
    let thumb_tx = crate::thumbs::spawn(tx.clone(), thumb_dir);
    let big_tx = crate::thumbs::spawn_big(tx.clone());
    let mut eng = Engine {
        mpv,
        ytdlp: YtDlp::new(&vendor.join("yt-dlp.exe")),
        cache_dir,
        tx,
        ui,
        results: Vec::new(),
        queue: Vec::new(),
        cur: None,
        loading: None,
        in_flight: HashSet::new(),
        playing: false,
        paused: false,
        shuffle: false,
        repeat: Repeat::Off,
        skip_chain: 0,
        status: String::new(),
        rng: seed,
        thumbs: HashMap::new(),
        thumb_pending: HashSet::new(),
        thumb_tx,
        big_tx,
        big_for: None,
        trim_at: Some(Instant::now() + Duration::from_secs(5)),
        client: Client::new(None),
        cookie_path,
        signed_in: false,
        playlists: Vec::new(),
        library_loading: false,
        playlist_id: String::new(),
        playlist_tracks: Vec::new(),
        artist: None,
        artist_wanted: String::new(),
        album_open: None,
        album_wanted: String::new(),
        state_path,
        dirty: Cell::new(false),
        last_save: Instant::now(),
        visible: true,
        volume: 0.8,
        resume_pos: None,
        pending_start: None,
        quit: false,
        perf: false,
        settings_open: false,
        ytdlp_version: String::new(),
        ytdlp_updating: false,
        lyrics_wanted: false,
        lyrics_for: None,
        lyrics: None,
        lyrics_cur: -1,
        party: None,
        party_server: String::new(),
        display_name: std::env::var("USERNAME").unwrap_or_default(),
        party_note: String::new(),
        update: None,
        update_checking: false,
        update_note: String::new(),
        auto_update: true,
        theme: 0,
        layout: 0,
        floating_player: false,
        next_update_check: None,
    };
    eng.load_stored_auth();
    eng.restore();
    // look for a newer version shortly after start-up, not while everything else is loading
    eng.next_update_check = eng.auto_update.then(|| Instant::now() + Duration::from_secs(15));
    eng.push_update();
    eng.fetch_ytdlp_version();
    eng.push_settings();
    eng.push_party_settings();
    eng.push_party();

    loop {
        loop {
            match eng.mpv.wait_event(0.0) {
                PlayerEvent::Idle => break,
                PlayerEvent::Other => {}
                PlayerEvent::Eof => eng.on_eof(),
                PlayerEvent::Error => eng.on_play_failed("playback error"),
                PlayerEvent::Shutdown => return,
            }
        }
        // the progress bar only needs frequent updates while it can be seen
        let ticking = eng.playing && !eng.paused && eng.visible;
        let wait = if ticking && !eng.perf { 250 } else { 1000 };
        match rx.recv_timeout(Duration::from_millis(wait)) {
            Ok(cmd) => {
                eng.handle(cmd);
                while let Ok(cmd) = rx.try_recv() {
                    eng.handle(cmd);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if eng.quit {
            return;
        }
        if ticking {
            eng.push_progress();
            eng.update_lyric_cursor();
        }
        eng.party_tick();
        if eng.auto_update && eng.next_update_check.is_some_and(|t| Instant::now() >= t) {
            eng.next_update_check = Some(Instant::now() + Duration::from_secs(24 * 60 * 60));
            eng.check_update(false);
        }
        if eng.trim_at.is_some_and(|t| Instant::now() >= t) {
            eng.trim_at = None;
            os::trim_memory();
            // mpv's position isn't valid right at load (e.g. when resuming), so correct the overlay now
            eng.push_media();
        }
        let since_save = eng.last_save.elapsed();
        let playing_now = eng.playing && !eng.paused;
        if (eng.dirty.get() && since_save >= Duration::from_secs(2))
            || (playing_now && since_save >= Duration::from_secs(30))
        {
            eng.save_state();
        }
    }
}

impl Engine {
    fn handle(&mut self, cmd: Cmd) {
        self.handle_cmd(cmd);
        self.refresh_thumbs();
        self.refresh_big_cover();
    }

    /// The art-first layout shows the current song's cover large. Fetch a bigger one for it,
    /// and let it go again when that layout is not in use (or covers are off).
    fn refresh_big_cover(&mut self) {
        let want = if self.layout == 2 && !self.perf {
            self.cur.and_then(|c| self.queue.get(c)).map(|t| (t.id.clone(), t.thumb.clone()))
        } else {
            None
        };
        match want {
            None => {
                if self.big_for.take().is_some() {
                    let _ = self.ui.upgrade_in_event_loop(|ui| ui.global::<AppState>().set_now_cover_big(Image::default()));
                }
            }
            Some((id, thumb)) => {
                if self.big_for.as_deref() != Some(id.as_str()) {
                    let url = crate::thumbs::big_url(&thumb, &id);
                    self.big_for = Some(id.clone());
                    let _ = self.ui.upgrade_in_event_loop(|ui| ui.global::<AppState>().set_now_cover_big(Image::default()));
                    let _ = self.big_tx.send((id, url));
                }
            }
        }
    }

    fn big_thumb_arrived(&mut self, id: String, rgba: Option<Vec<u8>>) {
        // the song may have changed while it loaded
        if self.big_for.as_deref() != Some(id.as_str()) {
            return;
        }
        let Some(rgba) = rgba else { return };
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(crate::thumbs::BIG, crate::thumbs::BIG);
            if rgba.len() == buf.as_bytes().len() {
                buf.make_mut_bytes().copy_from_slice(&rgba);
                ui.global::<AppState>().set_now_cover_big(Image::from_rgba8(buf));
            }
        });
    }

    /// Drops covers nobody is showing any more and asks for the ones that are missing
    fn refresh_thumbs(&mut self) {
        if self.perf {
            self.thumbs.clear();
            return;
        }
        // video id -> cover url, for every track currently listed anywhere
        let mut wanted: HashMap<&str, &str> = HashMap::new();
        for t in self.results.iter().chain(&self.queue).chain(&self.playlist_tracks) {
            wanted.entry(t.id.as_str()).or_insert(t.thumb.as_str());
        }
        // playlists share the cover cache; ones without art (Liked songs) are skipped
        // because there is no video thumbnail to fall back on
        for p in self.playlists.iter().filter(|p| !p.thumb.is_empty()) {
            wanted.entry(p.id.as_str()).or_insert(p.thumb.as_str());
        }
        // an artist page shows the artist's photo and the covers of their albums
        if let Some(view) = &self.artist {
            if !view.page.image.is_empty() {
                wanted.entry(view.id.as_str()).or_insert(view.page.image.as_str());
            }
            for a in view.page.albums.iter().filter(|a| !a.thumb.is_empty()) {
                wanted.entry(a.id.as_str()).or_insert(a.thumb.as_str());
            }
        }
        self.thumbs.retain(|id, _| wanted.contains_key(id.as_str()));
        let missing: Vec<(String, String)> = wanted
            .iter()
            .filter(|(id, _)| !self.thumbs.contains_key(**id) && !self.thumb_pending.contains(**id))
            .map(|(id, url)| (id.to_string(), url.to_string()))
            .collect();
        for (id, url) in missing {
            self.thumb_pending.insert(id.clone());
            let _ = self.thumb_tx.send((id, url));
        }
    }

    fn thumb_arrived(&mut self, id: String, rgba: Option<Vec<u8>>) {
        self.thumb_pending.remove(&id);
        let Some(rgba) = rgba else { return };
        if self.perf {
            return;
        }
        let on_artist_page = self.artist.as_ref().is_some_and(|v| v.id == id || v.page.albums.iter().any(|a| a.id == id));
        let still_shown = self.results.iter().chain(&self.queue).chain(&self.playlist_tracks).any(|t| t.id == id)
            || self.playlists.iter().any(|p| p.id == id)
            || on_artist_page;
        if !still_shown {
            return;
        }
        let rgba = Arc::new(rgba);
        self.thumbs.insert(id.clone(), rgba.clone());
        let current_id = self.cur.and_then(|c| self.queue.get(c)).map(|t| t.id.clone());
        // the picture at the top of the artist page: the artist's photo, or the open album's cover
        let header_id = match (&self.album_open, &self.artist) {
            (Some(album), _) => Some(album.id.clone()),
            (None, Some(view)) => Some(view.id.clone()),
            _ => None,
        };
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            let img = cover(&rgba);
            if current_id.as_deref() == Some(id.as_str()) {
                ui.global::<AppState>().set_now_cover(img.clone());
                ui.global::<AppState>().set_now_tint(average_color(&rgba));
            }
            if header_id.as_deref() == Some(id.as_str()) {
                ui.global::<AppState>().set_artist_image(img.clone());
            }
            // update rows in place so the lists don't rebuild or jump while scrolling
            for model in [ui.global::<AppState>().get_results(), ui.global::<AppState>().get_queue(), ui.global::<AppState>().get_playlist_tracks(), ui.global::<AppState>().get_playlists(), ui.global::<AppState>().get_artist_albums()] {
                for i in 0..model.row_count() {
                    if let Some(mut row) = model.row_data(i) {
                        if row.id == id.as_str() {
                            row.thumb = img.clone();
                            model.set_row_data(i, row);
                        }
                    }
                }
            }
        });
    }

    fn handle_cmd(&mut self, cmd: Cmd) {
        if self.is_guest() && is_playback_control(&cmd) {
            let allowed = self.party.as_ref().is_some_and(|p| p.control);
            self.status = match (allowed, self.guest_request(&cmd)) {
                (true, Some(act)) => {
                    if let Some(p) = self.party.as_ref() {
                        p.handle.send(PartyOut::Act(act));
                    }
                    // nothing changes here until the host does it and the new state comes back
                    "Sent to the host".into()
                }
                (true, None) => "Guests can play, pause, skip, seek and add songs".into(),
                (false, _) => "Only the host controls playback".into(),
            };
            self.sync();
            return;
        }
        match cmd {
            Cmd::PartyHost => self.party_host(),
            Cmd::PartyControl(on) => self.set_party_control(on),
            Cmd::PartyJoin(code) => self.party_join(code),
            Cmd::PartyLeave => self.end_party(String::new()),
            Cmd::Party(event) => self.handle_party_event(event),
            Cmd::SetPartyServer(address) => {
                self.party_server = address.trim().to_string();
                self.dirty.set(true);
                self.push_party_server_state();
            }
            Cmd::SetDisplayName(name) => {
                self.display_name = name.trim().to_string();
                self.dirty.set(true);
            }
            Cmd::CheckUpdate => self.check_update(true),
            Cmd::UpdateChecked { manual, result } => {
                self.update_checking = false;
                match result {
                    Ok(Some(release)) => {
                        self.update_note.clear();
                        self.update = Some(release);
                    }
                    Ok(None) => {
                        self.update = None;
                        self.update_note = if manual { "You're up to date".into() } else { String::new() };
                    }
                    // a failed background check stays quiet; only a manual one explains itself
                    Err(e) => self.update_note = if manual { e } else { String::new() },
                }
                self.push_update();
            }
            Cmd::OpenUpdate => {
                // the link was validated as plain https when the release was read
                if let Some(release) = &self.update {
                    let _ = webbrowser::open(&release.url);
                }
            }
            Cmd::SetFloatingPlayer(on) => {
                self.floating_player = on;
                self.dirty.set(true);
            }
            Cmd::SetLayout(layout) => {
                self.layout = layout.clamp(0, 3);
                self.dirty.set(true);
            }
            Cmd::SetTheme(theme) => {
                self.theme = theme.clamp(0, 6);
                self.dirty.set(true);
            }
            Cmd::SetAutoUpdate(on) => {
                self.auto_update = on;
                self.dirty.set(true);
                self.next_update_check = on.then(|| Instant::now() + Duration::from_secs(15));
            }
            Cmd::Search(q) => self.search(q),
            Cmd::SearchDone(res) => self.search_done(res),
            Cmd::PlayResult(i) => {
                if let Some(track) = self.results.get(i).cloned() {
                    self.play_now(track);
                }
            }
            Cmd::AddResult(i) => {
                if let Some(track) = self.results.get(i).cloned() {
                    self.enqueue(vec![track]);
                }
            }
            Cmd::PrefetchResult(i) => {
                let id = self.results.get(i).map(|t| t.id.clone());
                self.prefetch_hovered(id);
            }
            Cmd::PrefetchPlaylistTrack(i) => {
                let id = self.playlist_tracks.get(i).map(|t| t.id.clone());
                self.prefetch_hovered(id);
            }
            Cmd::PrefetchQueue(i) => {
                let id = self.queue.get(i).map(|t| t.id.clone());
                self.prefetch_hovered(id);
            }
            Cmd::PlayPlaylistTrack(i) => {
                if let Some(track) = self.playlist_tracks.get(i).cloned() {
                    self.play_now(track);
                }
            }
            Cmd::AddPlaylistTrack(i) => {
                if let Some(track) = self.playlist_tracks.get(i).cloned() {
                    self.enqueue(vec![track]);
                }
            }
            Cmd::PlayAll => {
                if !self.playlist_tracks.is_empty() {
                    self.queue = self.playlist_tracks.clone();
                    self.cur = None;
                    self.play_index(0);
                }
            }
            Cmd::AddAll => {
                let tracks = self.playlist_tracks.clone();
                self.enqueue(tracks);
            }
            Cmd::OpenLibrary => {
                if self.signed_in && self.playlists.is_empty() && !self.library_loading {
                    self.load_library();
                }
            }
            Cmd::LibraryLoaded(result) => self.library_loaded(result),
            Cmd::OpenPlaylist(i) => self.open_playlist(i),
            Cmd::PlaylistLoaded { id, result } => self.playlist_loaded(id, result),
            Cmd::OpenArtist(id) => self.open_artist(id),
            Cmd::ArtistLoaded { id, result } => self.artist_loaded(id, result),
            Cmd::OpenAlbum(i) => self.open_album(i),
            Cmd::AlbumLoaded { id, result } => self.album_loaded(id, result),
            Cmd::ArtistBack => self.artist_back(),
            Cmd::CloseArtist => self.close_artist(),
            Cmd::SignIn(input) => self.sign_in(input),
            Cmd::SignInDone(result) => self.sign_in_done(result),
            Cmd::SignOut => self.sign_out(),
            Cmd::RefreshLibrary => {
                if self.signed_in && !self.library_loading {
                    self.playlists.clear();
                    self.load_library();
                }
            }
            Cmd::ViewChanged(view) => {
                self.lyrics_wanted = view == 2;
                self.settings_open = view == -1;
                if self.lyrics_wanted {
                    self.ensure_lyrics();
                }
                if self.settings_open {
                    self.push_settings();
                }
            }
            Cmd::LyricsLoaded { id, result } => self.lyrics_loaded(id, result),
            Cmd::SetPerf(on) => self.set_perf(on),
            Cmd::ClearCache => self.clear_cache(),
            Cmd::UpdateYtdlp => self.update_ytdlp(),
            Cmd::YtdlpVersion(v) => {
                self.ytdlp_version = v;
                self.push_settings();
            }
            Cmd::YtdlpUpdated(result) => {
                self.ytdlp_updating = false;
                let note = match result {
                    Ok(line) => line,
                    Err(e) => format!("Update failed: {}", e.lines().last().unwrap_or("unknown error")),
                };
                let _ = self.ui.upgrade_in_event_loop(move |ui| {
                    ui.global::<AppState>().set_ytdlp_updating(false);
                    ui.global::<AppState>().set_ytdlp_note(note.into());
                });
                self.fetch_ytdlp_version();
            }
            Cmd::PlayQueue(i) => self.play_index(i),
            Cmd::RemoveQueue(i) => self.remove_queue(i),
            Cmd::MoveQueue { from, to } => {
                if move_in_queue(&mut self.queue, &mut self.cur, from, to) {
                    self.sync();
                    self.prefetch_next();
                }
            }
            Cmd::PlayNextResult(i) => {
                if let Some(track) = self.results.get(i).cloned() {
                    self.queue_next(track);
                }
            }
            Cmd::PlayNextPlaylistTrack(i) => {
                if let Some(track) = self.playlist_tracks.get(i).cloned() {
                    self.queue_next(track);
                }
            }
            Cmd::PlayNextQueue(i) => {
                if let Some(c) = self.cur.filter(|&c| c != i) {
                    // moving a row from above the current one lands one place earlier
                    let last = self.queue.len().saturating_sub(1);
                    let to = if i < c { c } else { c + 1 };
                    if move_in_queue(&mut self.queue, &mut self.cur, i, to.min(last)) {
                        self.sync();
                        self.prefetch_next();
                    }
                }
            }
            Cmd::OpenTrackWeb(id) => {
                // ids come from YouTube and go into a URL, so only plain video ids are opened
                if id.len() >= 6 && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
                    let _ = webbrowser::open(&format!("https://music.youtube.com/watch?v={id}"));
                }
            }
            Cmd::ClearQueue => {
                self.queue.clear();
                self.stop();
            }
            Cmd::TogglePause => self.toggle_pause(),
            Cmd::Resume => {
                if !self.playing || self.paused {
                    self.toggle_pause();
                }
            }
            Cmd::PausePlayback => {
                if self.playing && !self.paused {
                    self.toggle_pause();
                }
            }
            Cmd::Stop => self.stop(),
            Cmd::Next => {
                if let Some(n) = self.next_index() {
                    self.play_index(n);
                }
            }
            Cmd::Prev => self.prev(),
            Cmd::SeekFraction(f) => {
                if self.playing {
                    if let Some(dur) = self.mpv.property_f64("duration") {
                        self.seek_to((f.clamp(0.0, 1.0) as f64) * dur);
                    }
                } else if let Some(track) = self.cur.and_then(|c| self.queue.get(c)) {
                    // restored but not yet playing: move the saved position
                    self.resume_pos = Some((f.clamp(0.0, 1.0) as f64) * track.duration as f64);
                    self.push_progress();
                    self.dirty.set(true);
                }
            }
            Cmd::SeekSecs(secs) => {
                if self.playing {
                    self.seek_to(secs);
                }
            }
            Cmd::SeekBy(delta) => {
                if self.playing {
                    let pos = self.mpv.property_f64("time-pos").unwrap_or(0.0);
                    let end = self.mpv.property_f64("duration").unwrap_or(f64::MAX);
                    // stop just short of the end so a long press of → doesn't skip the song by accident
                    self.seek_to((pos + delta).clamp(0.0, (end - 1.0).max(0.0)));
                }
            }
            Cmd::SetVolume(v) => self.set_volume(v),
            Cmd::Visible(shown) => {
                self.visible = shown;
                if shown {
                    // the UI was idle while hidden, so bring it fully up to date
                    self.sync();
                    self.push_progress();
                    // and redraw all of it: what the window last drew may be gone
                    let _ = self.ui.upgrade_in_event_loop(|ui| ui.global::<AppState>().set_repaint_nonce(ui.global::<AppState>().get_repaint_nonce().wrapping_add(1)));
                } else {
                    self.trim_at = Some(Instant::now() + Duration::from_millis(500));
                }
            }
            Cmd::Quit => {
                self.end_party(String::new());
                self.save_state();
                let _ = self.mpv.command(&["stop"]);
                self.quit = true;
                let _ = slint::invoke_from_event_loop(|| {
                    let _ = slint::quit_event_loop();
                });
            }
            Cmd::ToggleShuffle => {
                self.shuffle = !self.shuffle;
                self.sync();
            }
            Cmd::CycleRepeat => {
                self.repeat = match self.repeat {
                    Repeat::Off => Repeat::All,
                    Repeat::All => Repeat::One,
                    Repeat::One => Repeat::Off,
                };
                self.sync();
            }
            Cmd::Fetched { id, result } => {
                self.in_flight.remove(&id);
                if self.loading.as_deref() == Some(id.as_str()) {
                    match result {
                        Ok(path) => self.start_file(&path),
                        Err(e) => self.on_play_failed(e.lines().last().unwrap_or("download failed")),
                    }
                }
            }
            Cmd::Thumb { id, rgba } => self.thumb_arrived(id, rgba),
            Cmd::BigThumb { id, rgba } => self.big_thumb_arrived(id, rgba),
        }
    }

    /// Queues `track` to play right after the current one, without interrupting it
    fn queue_next(&mut self, track: Track) {
        match self.cur {
            Some(c) => {
                self.queue.insert((c + 1).min(self.queue.len()), track);
                self.sync();
                self.prefetch_next();
            }
            // nothing is playing, so "next" is "now"
            None => self.enqueue(vec![track]),
        }
    }

    /// Plays `track` next, right after the current one
    fn play_now(&mut self, track: Track) {
        let at = self.cur.map_or(0, |c| c + 1).min(self.queue.len());
        self.queue.insert(at, track);
        self.play_index(at);
    }

    /// Appends tracks to the queue, starting playback if nothing is playing
    fn enqueue(&mut self, tracks: Vec<Track>) {
        if tracks.is_empty() {
            return;
        }
        let first_new = self.queue.len();
        self.queue.extend(tracks);
        if self.cur.is_none() {
            self.play_index(first_new);
        } else {
            self.sync();
            self.prefetch_next();
        }
    }

    /// Picks up cookies imported in an earlier run
    fn load_stored_auth(&mut self) {
        let auth = cookies::load_auth(&self.cookie_path);
        self.signed_in = auth.is_some();
        self.client = Client::new(auth);
        self.ytdlp.set_cookies(self.signed_in.then(|| self.cookie_path.clone()));
        let signed_in = self.signed_in;
        let _ = self.ui.upgrade_in_event_loop(move |ui| ui.global::<AppState>().set_signed_in(signed_in));
    }

    fn sign_in(&mut self, input: String) {
        let input = input.trim().trim_matches('"').to_string();
        if input.is_empty() {
            let _ = self.ui.upgrade_in_event_loop(|ui| {
                ui.global::<AppState>().set_auth_note("Enter the path to a cookies.txt file, or a browser name.".into());
            });
            return;
        }
        let _ = self.ui.upgrade_in_event_loop(|ui| {
            ui.global::<AppState>().set_signing_in(true);
            ui.global::<AppState>().set_auth_note("".into());
        });
        let (ytdlp, dest, tx) = (self.ytdlp.clone(), self.cookie_path.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let browser = input.to_lowercase();
            let result = if cookies::BROWSERS.contains(&browser.as_str()) {
                cookies::import_browser(&ytdlp, &browser, &dest)
            } else {
                cookies::import_file(std::path::Path::new(&input), &dest)
            };
            let _ = tx.send(Cmd::SignInDone(result));
        });
    }

    fn sign_in_done(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.load_stored_auth();
                self.playlists.clear();
                let _ = self.ui.upgrade_in_event_loop(|ui| {
                    ui.global::<AppState>().set_signing_in(false);
                    ui.global::<AppState>().set_signin_input("".into());
                    ui.global::<AppState>().set_auth_note("".into());
                });
                if self.signed_in {
                    self.load_library();
                }
            }
            Err(e) => {
                let _ = self.ui.upgrade_in_event_loop(move |ui| {
                    ui.global::<AppState>().set_signing_in(false);
                    ui.global::<AppState>().set_auth_note(e.into());
                });
            }
        }
    }

    fn sign_out(&mut self) {
        let _ = std::fs::remove_file(&self.cookie_path);
        self.load_stored_auth();
        self.playlists.clear();
        self.playlist_tracks.clear();
        self.playlist_id.clear();
        self.library_loading = false;
        let _ = self.ui.upgrade_in_event_loop(|ui| {
            ui.global::<AppState>().set_in_playlist(false);
            ui.global::<AppState>().set_playlists(ModelRc::new(VecModel::<TrackItem>::default()));
            ui.global::<AppState>().set_playlist_tracks(ModelRc::new(VecModel::<TrackItem>::default()));
            ui.global::<AppState>().set_library_note("".into());
        });
    }

    fn load_library(&mut self) {
        self.library_loading = true;
        let _ = self.ui.upgrade_in_event_loop(|ui| {
            ui.global::<AppState>().set_library_loading(true);
            ui.global::<AppState>().set_library_note("".into());
        });
        let (client, tx) = (self.client.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Cmd::LibraryLoaded(client.library_playlists()));
        });
    }

    fn library_loaded(&mut self, result: Result<Vec<PlaylistInfo>, String>) {
        self.library_loading = false;
        let note = match result {
            Ok(list) => {
                self.playlists = list;
                String::new()
            }
            Err(e) => {
                self.playlists.clear();
                e
            }
        };
        let playlists = self.playlists.clone();
        let thumbs = self.thumbs.clone();
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            ui.global::<AppState>().set_playlists(playlist_items(&playlists, &thumbs));
            ui.global::<AppState>().set_library_loading(false);
            ui.global::<AppState>().set_library_note(note.into());
        });
    }

    fn open_playlist(&mut self, i: usize) {
        let Some(info) = self.playlists.get(i).cloned() else { return };
        self.playlist_id = info.id.clone();
        self.playlist_tracks.clear();
        let title = info.title.clone();
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            ui.global::<AppState>().set_playlist_title(title.into());
            ui.global::<AppState>().set_playlist_tracks(ModelRc::new(VecModel::<TrackItem>::default()));
            ui.global::<AppState>().set_playlist_loading(true);
            ui.global::<AppState>().set_playlist_note("".into());
        });
        let (client, tx) = (self.client.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let result = client.playlist_tracks(&info.id);
            let _ = tx.send(Cmd::PlaylistLoaded { id: info.id, result });
        });
    }

    fn playlist_loaded(&mut self, id: String, result: Result<Vec<Track>, String>) {
        // the user may have opened another playlist while this one loaded
        if id != self.playlist_id {
            return;
        }
        let note = match result {
            Ok(tracks) => {
                self.playlist_tracks = tracks;
                String::new()
            }
            Err(e) => e,
        };
        let tracks = self.playlist_tracks.clone();
        let thumbs = self.thumbs.clone();
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            ui.global::<AppState>().set_playlist_tracks(items(&tracks, &thumbs));
            ui.global::<AppState>().set_playlist_loading(false);
            ui.global::<AppState>().set_playlist_note(note.into());
        });
    }

    // ---- artist pages -------------------------------------------------------

    fn open_artist(&mut self, id: String) {
        if !party::valid_artist_id(&id) {
            return;
        }
        // a playlist still loading must not fill this page
        self.playlist_id.clear();
        self.playlist_tracks.clear();
        self.artist = None;
        self.album_open = None;
        self.album_wanted.clear();
        self.artist_wanted = id.clone();
        let _ = self.ui.upgrade_in_event_loop(|ui| {
            ui.global::<AppState>().set_in_playlist(false);
            ui.global::<AppState>().set_artist_open(true);
            ui.global::<AppState>().set_artist_album(false);
            ui.global::<AppState>().set_artist_loading(true);
            ui.global::<AppState>().set_artist_note("".into());
            ui.global::<AppState>().set_artist_name("".into());
            ui.global::<AppState>().set_artist_sub("".into());
            ui.global::<AppState>().set_artist_image(Image::default());
            ui.global::<AppState>().set_artist_albums(ModelRc::new(VecModel::<TrackItem>::default()));
            ui.global::<AppState>().set_playlist_tracks(ModelRc::new(VecModel::<TrackItem>::default()));
        });
        let (client, tx) = (self.client.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let result = client.artist(&id);
            let _ = tx.send(Cmd::ArtistLoaded { id, result });
        });
    }

    fn artist_loaded(&mut self, id: String, result: Result<ArtistPage, String>) {
        if id != self.artist_wanted {
            return;
        }
        match result {
            Ok(page) => {
                self.playlist_tracks = page.songs.clone();
                self.artist = Some(ArtistView { id, page });
                self.push_artist();
                let _ = self.ui.upgrade_in_event_loop(|ui| ui.global::<AppState>().set_artist_loading(false));
            }
            Err(note) => {
                let _ = self.ui.upgrade_in_event_loop(move |ui| {
                    ui.global::<AppState>().set_artist_loading(false);
                    ui.global::<AppState>().set_artist_note(note.into());
                });
            }
        }
    }

    fn open_album(&mut self, i: usize) {
        let Some(album) = self.artist.as_ref().and_then(|v| v.page.albums.get(i)).cloned() else { return };
        self.album_wanted = album.id.clone();
        self.album_open = Some(album.clone());
        self.playlist_tracks.clear();
        self.push_artist();
        let _ = self.ui.upgrade_in_event_loop(|ui| {
            ui.global::<AppState>().set_artist_loading(true);
            ui.global::<AppState>().set_artist_note("".into());
        });
        let (client, tx) = (self.client.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let result = client.album_tracks(&album.id, &album.title, &album.thumb);
            let _ = tx.send(Cmd::AlbumLoaded { id: album.id, result });
        });
    }

    fn album_loaded(&mut self, id: String, result: Result<Vec<Track>, String>) {
        if id != self.album_wanted || self.album_open.is_none() {
            return;
        }
        let note = match result {
            Ok(tracks) => {
                self.playlist_tracks = tracks;
                String::new()
            }
            Err(e) => e,
        };
        self.push_artist();
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            ui.global::<AppState>().set_artist_loading(false);
            ui.global::<AppState>().set_artist_note(note.into());
        });
    }

    fn artist_back(&mut self) {
        if self.album_open.take().is_some() {
            // from an album back to the artist's own page
            self.album_wanted.clear();
            self.playlist_tracks = self.artist.as_ref().map(|v| v.page.songs.clone()).unwrap_or_default();
            self.push_artist();
            let _ = self.ui.upgrade_in_event_loop(|ui| {
                ui.global::<AppState>().set_artist_loading(false);
                ui.global::<AppState>().set_artist_note("".into());
            });
        } else {
            self.close_artist();
        }
    }

    fn close_artist(&mut self) {
        self.artist = None;
        self.album_open = None;
        self.artist_wanted.clear();
        self.album_wanted.clear();
        self.playlist_tracks.clear();
        let _ = self.ui.upgrade_in_event_loop(|ui| {
            ui.global::<AppState>().set_artist_open(false);
            ui.global::<AppState>().set_artist_album(false);
            ui.global::<AppState>().set_artist_loading(false);
            ui.global::<AppState>().set_artist_albums(ModelRc::new(VecModel::<TrackItem>::default()));
            ui.global::<AppState>().set_playlist_tracks(ModelRc::new(VecModel::<TrackItem>::default()));
        });
    }

    /// Fills the artist page: the header, the song list (the artist's or the open album's) and the albums
    fn push_artist(&self) {
        let Some(view) = self.artist.as_ref() else { return };
        let (title, sub, image_id) = match &self.album_open {
            Some(a) => (a.title.clone(), format!("{} · {}", view.page.name, a.subtitle), a.id.clone()),
            None => (view.page.name.clone(), view.page.subtitle.clone(), view.id.clone()),
        };
        let is_album = self.album_open.is_some();
        let image = self.thumbs.get(&image_id).cloned();
        let albums: Vec<PlaylistInfo> = view
            .page
            .albums
            .iter()
            .map(|a| PlaylistInfo { id: a.id.clone(), title: a.title.clone(), subtitle: a.subtitle.clone(), thumb: a.thumb.clone() })
            .collect();
        let (tracks, thumbs) = (self.playlist_tracks.clone(), self.thumbs.clone());
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            ui.global::<AppState>().set_artist_album(is_album);
            ui.global::<AppState>().set_artist_name(title.into());
            ui.global::<AppState>().set_artist_sub(sub.into());
            ui.global::<AppState>().set_artist_image(image.map(|b| cover(&b)).unwrap_or_default());
            ui.global::<AppState>().set_artist_albums(playlist_items(&albums, &thumbs));
            ui.global::<AppState>().set_playlist_tracks(items(&tracks, &thumbs));
        });
    }

    // ---- updates ------------------------------------------------------------

    fn check_update(&mut self, manual: bool) {
        if self.update_checking {
            return;
        }
        self.update_checking = true;
        if manual {
            self.update_note.clear();
        }
        self.push_update();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            // YTMUSIC_UPDATE_URL points the check at a test manifest
            let url = std::env::var("YTMUSIC_UPDATE_URL").unwrap_or_else(|_| crate::config::UPDATE_MANIFEST_URL.into());
            let result = update::fetch(&url, update::CURRENT_VERSION);
            let _ = tx.send(Cmd::UpdateChecked { manual, result });
        });
    }

    fn push_update(&self) {
        let (release, checking, note, auto) = (self.update.clone(), self.update_checking, self.update_note.clone(), self.auto_update);
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            ui.global::<AppState>().set_app_version(update::CURRENT_VERSION.into());
            ui.global::<AppState>().set_update_available(release.is_some());
            ui.global::<AppState>().set_update_version(release.as_ref().map(|r| r.version.clone()).unwrap_or_default().into());
            ui.global::<AppState>().set_update_notes(release.as_ref().map(|r| r.notes.clone()).unwrap_or_default().into());
            ui.global::<AppState>().set_update_host(release.as_ref().map(|r| r.host()).unwrap_or_default().into());
            ui.global::<AppState>().set_update_checking(checking);
            ui.global::<AppState>().set_update_note(note.into());
            ui.global::<AppState>().set_auto_update_check(auto);
        });
    }

    // ---- listening party ----------------------------------------------------

    fn is_guest(&self) -> bool {
        self.party.as_ref().is_some_and(|p| p.role == PartyRole::Guest)
    }

    /// Is the host currently playing (as far as we last heard)?
    fn guest_host_playing(&self) -> bool {
        self.party.as_ref().and_then(|p| p.last_host.as_ref()).is_some_and(|h| h.playing)
    }

    /// Where the host's song is right now, if the host is on the song we have loaded
    fn guest_target(&self) -> Option<f64> {
        let p = self.party.as_ref()?;
        let host = p.last_host.as_ref()?;
        let mine = self.cur.and_then(|c| self.queue.get(c))?;
        (host.track.as_ref()?.id == mine.id).then(|| party::target_position(host, p.handle.server_now_ms()))
    }

    /// The address typed in Settings, or the built-in one when that is empty
    fn effective_party_server(&self) -> String {
        let custom = normalize_server(&self.party_server);
        if custom.is_empty() { normalize_server(crate::config::DEFAULT_PARTY_SERVER) } else { custom }
    }

    fn party_start_connection(&mut self) -> Option<party::Handle> {
        if self.party.is_some() {
            return None;
        }
        let address = self.effective_party_server();
        if address.is_empty() {
            self.party_note = "Set the party server address in Settings first".into();
            self.push_party();
            return None;
        }
        self.party_note.clear();
        Some(party::spawn(address, self.tx.clone()))
    }

    fn new_party(&self, role: PartyRole, handle: party::Handle) -> PartyState {
        PartyState {
            role,
            handle,
            ready: false,
            room: String::new(),
            host_name: String::new(),
            members: Vec::new(),
            note: String::new(),
            last_host: None,
            signature: String::new(),
            last_sent: Instant::now(),
            force: true,
            saved_queue: (role == PartyRole::Guest).then(|| (self.queue.clone(), self.cur)),
            sync_text: String::new(),
            last_check: Instant::now(),
            control: false,
        }
    }

    /// What a guest's button press means as a request to the host, if shared control covers it
    fn guest_request(&self, cmd: &Cmd) -> Option<Act> {
        let current = self.cur.and_then(|c| self.queue.get(c));
        let for_id = current.map(|t| t.id.clone()).unwrap_or_default();
        let act = |kind, pos: Option<f64>, track: Option<&Track>| Act { kind, for_id: for_id.clone(), pos, track: track.map(TrackInfo::from) };
        let host_playing = self.guest_host_playing();
        Some(match cmd {
            Cmd::TogglePause => act(if host_playing { ActKind::Pause } else { ActKind::Play }, None, None),
            Cmd::Resume => act(ActKind::Play, None, None),
            Cmd::PausePlayback => act(ActKind::Pause, None, None),
            Cmd::Next => act(ActKind::Next, None, None),
            Cmd::Prev => act(ActKind::Prev, None, None),
            Cmd::SeekFraction(f) => act(ActKind::Seek, Some(f.clamp(0.0, 1.0) as f64 * current?.duration as f64), None),
            Cmd::SeekSecs(s) => act(ActKind::Seek, Some(*s), None),
            Cmd::SeekBy(d) => act(ActKind::Seek, Some((self.mpv.property_f64("time-pos").unwrap_or(0.0) + d).max(0.0)), None),
            // a guest has no queue of their own, so "play this" can only mean "add this"
            Cmd::PlayResult(i) | Cmd::AddResult(i) => act(ActKind::Add, None, Some(self.results.get(*i)?)),
            Cmd::PlayPlaylistTrack(i) | Cmd::AddPlaylistTrack(i) => act(ActKind::Add, None, Some(self.playlist_tracks.get(*i)?)),
            _ => return None,
        })
    }

    fn set_party_control(&mut self, on: bool) {
        let Some(p) = self.party.as_mut().filter(|p| p.role == PartyRole::Host && p.ready) else { return };
        p.control = on;
        p.handle.send(PartyOut::Control(on));
        self.push_party();
    }

    /// The host carries out what a guest asked for, through the same code the host's own buttons use
    fn host_act(&mut self, from: String, act: Act) {
        if !self.party.as_ref().is_some_and(|p| p.role == PartyRole::Host && p.control) {
            return;
        }
        let current = self.cur.and_then(|c| self.queue.get(c)).map(|t| t.id.clone()).unwrap_or_default();
        // a request about a song that is already over (two people skipping at once) is stale
        let fresh = act.for_id == current;
        let note = match act.kind {
            ActKind::Play => {
                self.handle_cmd(Cmd::Resume);
                "pressed play".to_string()
            }
            ActKind::Pause => {
                self.handle_cmd(Cmd::PausePlayback);
                "paused".to_string()
            }
            ActKind::Next if fresh => {
                self.handle_cmd(Cmd::Next);
                "skipped to the next song".to_string()
            }
            ActKind::Prev if fresh => {
                self.handle_cmd(Cmd::Prev);
                "went back".to_string()
            }
            ActKind::Seek if fresh => {
                let secs = act.pos.unwrap_or(0.0);
                if self.playing {
                    self.handle_cmd(Cmd::SeekSecs(secs));
                } else if self.cur.is_some() {
                    self.resume_pos = Some(secs);
                    self.push_progress();
                    if let Some(p) = self.party.as_mut() {
                        p.force = true;
                    }
                }
                "moved the song".to_string()
            }
            ActKind::Add => match act.track.as_ref().map(Track::from) {
                // the same song twice in a row is a double click, not two requests
                Some(track) if !self.queue.iter().skip(self.cur.map_or(0, |c| c + 1)).any(|t| t.id == track.id) && track.id != current => {
                    let title = track.title.clone();
                    self.enqueue(vec![track]);
                    format!("added {title}")
                }
                _ => return,
            },
            _ => return,
        };
        // shown under the song title, not as the red party error line
        self.status = format!("{from} {note}");
        self.sync();
        self.push_party();
    }

    fn party_host(&mut self) {
        let Some(handle) = self.party_start_connection() else { return };
        handle.send(PartyOut::Create(self.display_name.clone()));
        self.party = Some(self.new_party(PartyRole::Host, handle));
        self.push_party();
    }

    fn party_join(&mut self, code: String) {
        let room = clean_room_code(&code);
        if room.is_empty() {
            self.party_note = "Enter the party code first".into();
            self.push_party();
            return;
        }
        let Some(handle) = self.party_start_connection() else { return };
        handle.send(PartyOut::Join { room, name: self.display_name.clone() });
        self.party = Some(self.new_party(PartyRole::Guest, handle));
        self.push_party();
    }

    fn handle_party_event(&mut self, event: party::Event) {
        use party::Event;
        match event {
            Event::Created { room } => {
                if let Some(p) = self.party.as_mut() {
                    p.room = room;
                    p.ready = true;
                    p.force = true;
                }
            }
            Event::Joined { room, host } => {
                let guest_resets_playback = self.party.as_ref().is_some_and(|p| p.role == PartyRole::Guest);
                if let Some(p) = self.party.as_mut() {
                    p.room = room;
                    p.host_name = host;
                    p.ready = true;
                }
                if guest_resets_playback {
                    // our own music stops; the host's takes over
                    let _ = self.mpv.command(&["stop"]);
                    self.playing = false;
                    self.paused = false;
                    self.loading = None;
                    self.status = "Waiting for the host…".into();
                    self.sync();
                    self.push_progress();
                }
            }
            Event::Members(members) => {
                if let Some(p) = self.party.as_mut() {
                    p.members = members;
                }
            }
            Event::State(host_state) => self.apply_host_state(host_state),
            Event::Control(on) => {
                if let Some(p) = self.party.as_mut().filter(|p| p.role == PartyRole::Guest) {
                    p.control = on;
                }
            }
            Event::Act { from, act } => self.host_act(from, act),
            Event::Hello => {
                if let Some(p) = self.party.as_mut() {
                    p.force = true;
                }
            }
            Event::Error(message) => {
                let ready = self.party.as_ref().is_some_and(|p| p.ready);
                if ready {
                    if let Some(p) = self.party.as_mut() {
                        p.note = message;
                    }
                } else {
                    // e.g. a wrong party code: the party never started
                    self.end_party(message);
                    return;
                }
            }
            Event::Closed(reason) | Event::Disconnected(reason) => {
                self.end_party(reason);
                return;
            }
        }
        self.push_party();
    }

    /// A guest follows the host: same song, same place, same play/pause
    fn apply_host_state(&mut self, host: HostState) {
        if !self.is_guest() {
            return;
        }
        if let Some(p) = self.party.as_mut() {
            p.last_host = Some(host.clone());
        }
        // the host answered whatever this guest asked for
        if self.status == "Sent to the host" {
            self.status.clear();
        }
        let Some(track) = host.track.as_ref().map(Track::from) else {
            // the host has nothing playing
            let _ = self.mpv.command(&["stop"]);
            self.playing = false;
            self.paused = false;
            self.loading = None;
            self.cur = None;
            self.queue.clear();
            self.status = "Waiting for the host to play something".into();
            self.sync();
            self.push_progress();
            return;
        };
        let upcoming: Vec<Track> = host.next.iter().map(Track::from).collect();
        let on_same_song = self.cur.and_then(|c| self.queue.get(c)).is_some_and(|t| t.id == track.id);

        if !on_same_song {
            self.guest_status("Getting audio…");
            let _ = self.mpv.set_property("speed", "1.0");
            self.queue = std::iter::once(track).chain(upcoming).collect();
            self.cur = None;
            self.play_index(0);
            return;
        }

        // same song: keep the shared queue current, and mirror play/pause
        if let Some(c) = self.cur {
            self.queue.truncate(c + 1);
            self.queue.extend(upcoming);
        }
        if host.playing && self.paused && self.playing {
            self.toggle_pause();
        } else if !host.playing && self.playing && !self.paused {
            self.toggle_pause();
        } else {
            self.sync();
        }
        self.prefetch_next();
        self.guest_correct(true);
    }

    /// Keeps a guest's playback aligned with the host's: small drifts are absorbed by a
    /// slight speed change, big ones by a seek
    fn guest_correct(&mut self, force: bool) {
        if !self.is_guest() || !self.playing || self.paused {
            return;
        }
        let Some(p) = self.party.as_mut() else { return };
        if !force && p.last_check.elapsed() < Duration::from_secs(1) {
            return;
        }
        p.last_check = Instant::now();
        let Some(target) = self.guest_target() else { return };
        let actual = self.mpv.property_f64("time-pos").unwrap_or(target);
        let (speed, text) = match party::correction(target, actual) {
            Correction::InSync => ("1.0".to_string(), "In sync"),
            Correction::Speed(s) => (format!("{s}"), "Catching up…"),
            Correction::Seek(to) => {
                let _ = self.mpv.command(&["seek", &format!("{to:.2}"), "absolute"]);
                ("1.0".to_string(), "Resyncing…")
            }
        };
        let _ = self.mpv.set_property("speed", &speed);
        self.guest_status(text);
    }

    /// Tells the host (and this panel) how well this guest is keeping up, when that changes
    fn guest_status(&mut self, text: &str) {
        let Some(p) = self.party.as_mut() else { return };
        if p.sync_text == text {
            return;
        }
        p.sync_text = text.into();
        p.handle.send(PartyOut::Status(text.into()));
        self.push_party();
    }

    /// Runs on every pass of the engine loop: the host announces changes (and a periodic
    /// heartbeat), a guest corrects drift
    fn party_tick(&mut self) {
        self.guest_correct(false);
        let Some(p) = self.party.as_ref() else { return };
        if p.role != PartyRole::Host || !p.ready {
            return;
        }
        let playing = self.playing && !self.paused;
        let track = self.cur.and_then(|c| self.queue.get(c)).map(TrackInfo::from);
        // shuffle picks the next song at random, so there is nothing to share ahead of time
        let next: Vec<TrackInfo> = match (self.cur, self.shuffle) {
            (Some(c), false) => self.queue.iter().skip(c + 1).take(party::SHARED_UPCOMING).map(TrackInfo::from).collect(),
            _ => Vec::new(),
        };
        let signature = format!(
            "{}|{}|{}",
            track.as_ref().map_or("", |t| t.id.as_str()),
            playing,
            next.iter().map(|t| t.id.as_str()).collect::<Vec<_>>().join(",")
        );
        let heartbeat_due = playing && p.last_sent.elapsed() >= Duration::from_secs(5);
        if signature == p.signature && !p.force && !heartbeat_due {
            return;
        }
        let pos = if self.playing {
            self.mpv.property_f64("time-pos").unwrap_or(0.0)
        } else {
            self.resume_pos.unwrap_or(0.0)
        };
        let state = HostState { track, playing, pos, ts: p.handle.server_now_ms(), next };
        if let Some(p) = self.party.as_mut() {
            p.handle.send(PartyOut::State(state));
            p.signature = signature;
            p.last_sent = Instant::now();
            p.force = false;
        }
    }

    /// Leaves the party. Guests get their own queue back.
    fn end_party(&mut self, note: String) {
        let Some(p) = self.party.take() else { return };
        p.handle.send(PartyOut::Leave);
        if p.role == PartyRole::Guest {
            let _ = self.mpv.set_property("speed", "1.0");
            let _ = self.mpv.command(&["stop"]);
            self.playing = false;
            self.paused = false;
            self.loading = None;
            self.resume_pos = None;
            let (queue, cur) = p.saved_queue.unwrap_or_default();
            self.queue = queue;
            self.cur = cur;
            self.status.clear();
            self.sync();
            self.push_progress();
        }
        self.party_note = note;
        self.push_party();
    }

    fn push_party(&self) {
        let (mode, connecting, room, host, members, note, sync_text) = match &self.party {
            None => (0, false, String::new(), String::new(), Vec::new(), self.party_note.clone(), String::new()),
            Some(p) => (
                if p.role == PartyRole::Host { 1 } else { 2 },
                !p.ready,
                p.room.clone(),
                if p.role == PartyRole::Host { self.display_name.clone() } else { p.host_name.clone() },
                p.members
                    .iter()
                    .map(|m| {
                        let tag = if m.host { "Host".to_string() } else { m.sync.clone() };
                        let initial = m.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                        (m.name.clone(), initial, tag)
                    })
                    .collect::<Vec<_>>(),
                p.note.clone(),
                p.sync_text.clone(),
            ),
        };
        let control = self.party.as_ref().is_some_and(|p| p.control);
        let locked = mode == 2 && !control;
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            ui.global::<AppState>().set_party_control(control);
            ui.global::<AppState>().set_party_mode(mode);
            ui.global::<AppState>().set_party_connecting(connecting);
            ui.global::<AppState>().set_party_room(room.into());
            ui.global::<AppState>().set_party_host_name(host.into());
            let rows: Vec<PartyMember> = members
                .into_iter()
                .map(|(name, initial, tag)| PartyMember { name: name.into(), initial: initial.into(), tag: tag.into() })
                .collect();
            ui.global::<AppState>().set_party_members(ModelRc::new(VecModel::from(rows)));
            ui.global::<AppState>().set_party_note(note.into());
            ui.global::<AppState>().set_party_sync(sync_text.into());
            ui.global::<AppState>().set_controls_locked(locked);
        });
    }

    /// Fills the two text fields once at start-up (afterwards they belong to the user)
    fn push_party_settings(&self) {
        let (server, name) = (self.party_server.clone(), self.display_name.clone());
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            ui.global::<AppState>().set_party_server(server.into());
            ui.global::<AppState>().set_display_name(name.into());
        });
        self.push_party_server_state();
    }

    /// Tells the interface whether parties have a server to use, and what the built-in one is
    fn push_party_server_state(&self) {
        let usable = !self.effective_party_server().is_empty();
        let built_in = crate::config::DEFAULT_PARTY_SERVER.to_string();
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            ui.global::<AppState>().set_party_server_ready(usable);
            ui.global::<AppState>().set_party_server_built_in(!built_in.is_empty());
        });
    }

    /// Fetches lyrics for the current song unless the ones on screen already belong to it
    fn ensure_lyrics(&mut self) {
        let Some(track) = self.cur.and_then(|c| self.queue.get(c)).cloned() else { return };
        if self.lyrics_for.as_deref() == Some(track.id.as_str()) {
            return;
        }
        self.lyrics_for = Some(track.id.clone());
        self.lyrics = None;
        self.lyrics_cur = -1;
        let _ = self.ui.upgrade_in_event_loop(|ui| {
            ui.global::<AppState>().set_lyrics_lines(ModelRc::new(VecModel::<LyricLine>::default()));
            ui.global::<AppState>().set_lyrics_synced(false);
            ui.global::<AppState>().set_lyrics_current(-1);
            ui.global::<AppState>().set_lyrics_source("".into());
            ui.global::<AppState>().set_lyrics_note("".into());
            ui.global::<AppState>().set_lyrics_loading(true);
        });
        let (client, tx) = (self.client.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let result = lyrics::find(&client, &track);
            let _ = tx.send(Cmd::LyricsLoaded { id: track.id, result });
        });
    }

    fn lyrics_loaded(&mut self, id: String, result: Result<Option<Lyrics>, String>) {
        // the song may have changed while this was loading
        if self.lyrics_for.as_deref() != Some(id.as_str()) {
            return;
        }
        let (found, note) = match result {
            Ok(Some(l)) => (Some(l), String::new()),
            Ok(None) => (None, "No lyrics found for this song".to_string()),
            Err(e) => (None, e),
        };
        let (lines, synced, source) = match &found {
            Some(l) => (l.lines.clone(), l.synced, l.source.clone()),
            None => (Vec::new(), false, String::new()),
        };
        self.lyrics = found;
        self.lyrics_cur = -1;
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            let rows: Vec<LyricLine> =
                lines.iter().map(|(t, s)| LyricLine { text: s.as_str().into(), time: *t as f32 }).collect();
            ui.global::<AppState>().set_lyrics_lines(ModelRc::new(VecModel::from(rows)));
            ui.global::<AppState>().set_lyrics_synced(synced);
            ui.global::<AppState>().set_lyrics_source(source.into());
            ui.global::<AppState>().set_lyrics_note(note.into());
            ui.global::<AppState>().set_lyrics_loading(false);
            ui.global::<AppState>().set_lyrics_current(-1);
        });
        self.update_lyric_cursor();
    }

    /// Highlights the lyric line being sung (synced lyrics only)
    fn update_lyric_cursor(&mut self) {
        if !self.lyrics_wanted || !self.visible || !self.playing {
            return;
        }
        let Some(lyrics) = self.lyrics.as_ref().filter(|l| l.synced) else { return };
        let pos = self.mpv.property_f64("time-pos").unwrap_or(0.0);
        // a hair early, so the line is already lit as it starts
        let idx = lyrics.lines.partition_point(|(t, _)| *t <= pos + 0.15) as i32 - 1;
        if idx != self.lyrics_cur {
            self.lyrics_cur = idx;
            let _ = self.ui.upgrade_in_event_loop(move |ui| ui.global::<AppState>().set_lyrics_current(idx));
        }
    }

    fn set_perf(&mut self, on: bool) {
        self.perf = on;
        if on {
            self.thumbs.clear();
        }
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            ui.global::<Theme>().set_perf(on);
            ui.global::<AppState>().set_perf_mode(on);
        });
        self.push_lists();
        self.sync();
        self.dirty.set(true);
    }

    /// Rebuilds the result and playlist lists (after covers are switched on or off)
    fn push_lists(&self) {
        let (results, playlists, tracks) = (self.results.clone(), self.playlists.clone(), self.playlist_tracks.clone());
        let thumbs = self.thumbs.clone();
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            ui.global::<AppState>().set_results(items(&results, &thumbs));
            ui.global::<AppState>().set_playlists(playlist_items(&playlists, &thumbs));
            ui.global::<AppState>().set_playlist_tracks(items(&tracks, &thumbs));
        });
        self.push_artist();
    }

    fn push_settings(&self) {
        let bytes = dir_size(&self.cache_dir);
        let cache_text = if bytes == 0 {
            "Empty".to_string()
        } else if bytes < 1024 * 1024 {
            "Less than 1 MB of downloaded songs".to_string()
        } else {
            format!("{:.0} MB of downloaded songs", bytes as f64 / (1024.0 * 1024.0))
        };
        let (version, perf, updating) = (self.ytdlp_version.clone(), self.perf, self.ytdlp_updating);
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            ui.global::<AppState>().set_cache_text(cache_text.into());
            ui.global::<AppState>().set_ytdlp_version(version.into());
            ui.global::<AppState>().set_perf_mode(perf);
            ui.global::<AppState>().set_ytdlp_updating(updating);
        });
    }

    /// Deletes downloaded songs. The one mpv is playing is locked by Windows and simply stays.
    fn clear_cache(&mut self) {
        if let Ok(entries) = std::fs::read_dir(&self.cache_dir) {
            for e in entries.flatten() {
                let _ = std::fs::remove_file(e.path());
            }
        }
        self.push_settings();
    }

    fn fetch_ytdlp_version(&self) {
        let (ytdlp, tx) = (self.ytdlp.clone(), self.tx.clone());
        std::thread::spawn(move || {
            if let Ok(v) = ytdlp.version() {
                let _ = tx.send(Cmd::YtdlpVersion(v));
            }
        });
    }

    fn update_ytdlp(&mut self) {
        if self.ytdlp_updating {
            return;
        }
        self.ytdlp_updating = true;
        let _ = self.ui.upgrade_in_event_loop(|ui| {
            ui.global::<AppState>().set_ytdlp_updating(true);
            ui.global::<AppState>().set_ytdlp_note("Checking for updates…".into());
        });
        let (ytdlp, tx) = (self.ytdlp.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Cmd::YtdlpUpdated(ytdlp.update()));
        });
    }

    fn search(&mut self, query: String) {
        let query = query.trim().to_string();
        if query.is_empty() {
            return;
        }
        let _ = self.ui.upgrade_in_event_loop(|ui| {
            ui.global::<AppState>().set_searching(true);
            ui.global::<AppState>().set_search_note("".into());
        });
        let (client, ytdlp, tx) = (self.client.clone(), self.ytdlp.clone(), self.tx.clone());
        std::thread::spawn(move || {
            let result = match client.search_songs(&query) {
                Ok(songs) if !songs.is_empty() => Ok(songs),
                // YouTube Music had nothing (or changed its format): fall back to plain video search
                _ => ytdlp.search(&query, 20),
            };
            let _ = tx.send(Cmd::SearchDone(result));
        });
    }

    fn search_done(&mut self, res: Result<Vec<Track>, String>) {
        let (tracks, note) = match res {
            Ok(t) if t.is_empty() => (t, "No results".to_string()),
            Ok(t) => (t, String::new()),
            Err(e) => (Vec::new(), format!("Search failed: {}", e.lines().last().unwrap_or(""))),
        };
        self.results = tracks.clone();
        let thumbs = self.thumbs.clone();
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            ui.global::<AppState>().set_results(items(&tracks, &thumbs));
            ui.global::<AppState>().set_searching(false);
            ui.global::<AppState>().set_search_note(note.into());
        });
    }

    fn toggle_pause(&mut self) {
        if self.playing {
            self.paused = !self.paused;
            let _ = self.mpv.set_property("pause", if self.paused { "yes" } else { "no" });
            self.sync();
        } else if self.loading.is_none() {
            // nothing loaded yet: start the selected (or first) track, resuming a restored position
            if let Some(i) = self.cur.or(if self.queue.is_empty() { None } else { Some(0) }) {
                self.play_index(i);
            }
        }
    }

    fn seek_to(&mut self, secs: f64) {
        let _ = self.mpv.command(&["seek", &format!("{:.2}", secs.max(0.0)), "absolute"]);
        self.push_progress();
        self.push_media();
        self.update_lyric_cursor();
        // guests need the new position right away, not at the next heartbeat
        if let Some(p) = self.party.as_mut().filter(|p| p.role == PartyRole::Host) {
            p.force = true;
        }
    }

    fn play_index(&mut self, i: usize) {
        let Some(track) = self.queue.get(i).cloned() else { return };
        let _ = self.mpv.command(&["stop"]);
        // a restored position only applies to the track it was saved for
        self.pending_start = if self.cur == Some(i) { self.resume_pos.take() } else { None };
        self.resume_pos = None;
        self.cur = Some(i);
        self.playing = false;
        self.paused = false;
        self.loading = Some(track.id.clone());
        if let Some(path) = self.ytdlp.cached(&track.id, &self.cache_dir) {
            self.start_file(&path);
            return;
        }
        self.status = "Getting audio, new songs take a few seconds…".into();
        self.sync();
        self.fetch(track.id);
    }

    fn start_file(&mut self, path: &std::path::Path) {
        // don't "resume" within the last few seconds, that would just end the track
        let near_end = |t: &Track, at: f64| at + 3.0 >= t.duration as f64;
        // a guest starts wherever the host is by now, however long the download took
        let wanted = if self.is_guest() { self.guest_target() } else { self.pending_start.take() };
        let start = wanted
            .filter(|&at| at > 1.0 && !self.cur.and_then(|c| self.queue.get(c)).is_some_and(|t| near_end(t, at)));
        if self.mpv.load(path, start).is_err() {
            self.on_play_failed("couldn't open the audio file");
            return;
        }
        let host_paused = self.is_guest() && !self.guest_host_playing();
        let _ = self.mpv.set_property("pause", if host_paused { "yes" } else { "no" });
        self.loading = None;
        self.playing = true;
        self.paused = host_paused;
        self.skip_chain = 0;
        self.status.clear();
        self.trim_at = Some(Instant::now() + Duration::from_secs(3));
        self.sync();
        self.push_progress();
        self.prefetch_next();
        if self.lyrics_wanted {
            self.ensure_lyrics();
        }
    }

    fn fetch(&mut self, id: String) {
        if !self.in_flight.insert(id.clone()) {
            return;
        }
        let (ytdlp, dir, tx) = (self.ytdlp.clone(), self.cache_dir.clone(), self.tx.clone());
        let log = self.cache_dir.with_file_name("fetch-debug.txt");
        let signed_in = self.signed_in;
        std::thread::spawn(move || {
            let started = Instant::now();
            let result = ytdlp.fetch_audio(&id, &dir);
            // timing only: a video id, seconds taken, and whether it worked
            let line = format!(
                "{id} {:.1}s signed_in={signed_in} {}\n",
                started.elapsed().as_secs_f32(),
                if result.is_ok() { "ok" } else { "failed" }
            );
            if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(log) {
                use std::io::Write;
                let _ = f.write_all(line.as_bytes());
            }
            let _ = tx.send(Cmd::Fetched { id, result });
        });
    }

    /// Warms the cache for the next tracks in the queue so they start instantly
    fn prefetch_next(&mut self) {
        if self.shuffle {
            return;
        }
        let Some(cur) = self.cur else { return };
        let upcoming: Vec<String> = self.queue.iter().skip(cur + 1).take(2).map(|t| t.id.clone()).collect();
        for id in upcoming {
            if self.ytdlp.cached(&id, &self.cache_dir).is_none() {
                self.fetch(id);
            }
        }
    }

    /// Hover-triggered prefetch. Each fetch runs yt-dlp (about 80 MB while it
    /// runs), so only two may be in flight at once.
    fn prefetch_hovered(&mut self, id: Option<String>) {
        let Some(id) = id else { return };
        if self.in_flight.len() >= 2 || self.ytdlp.cached(&id, &self.cache_dir).is_some() {
            return;
        }
        self.fetch(id);
    }

    fn on_eof(&mut self) {
        self.playing = false;
        if self.is_guest() {
            // the host decides what comes next; wait for their announcement
            self.status = "Waiting for the host…".into();
            self.sync();
            return;
        }
        if self.repeat == Repeat::One {
            if let Some(c) = self.cur {
                self.play_index(c);
                return;
            }
        }
        match self.next_index() {
            Some(n) => self.play_index(n),
            None => {
                self.status = "End of queue".into();
                self.sync();
                self.push_progress();
            }
        }
    }

    fn on_play_failed(&mut self, why: &str) {
        let title = self.cur.and_then(|c| self.queue.get(c)).map(|t| t.title.clone()).unwrap_or_default();
        self.loading = None;
        self.playing = false;
        self.skip_chain += 1;
        if self.skip_chain < self.queue.len() {
            if let Some(n) = self.next_index() {
                self.play_index(n);
                return;
            }
        }
        self.status = format!("Can't play \"{title}\": {why}");
        self.sync();
    }

    fn next_index(&mut self) -> Option<usize> {
        let len = self.queue.len();
        if len == 0 {
            return None;
        }
        if self.shuffle && len > 1 {
            loop {
                let n = (self.next_rand() % len as u64) as usize;
                if Some(n) != self.cur {
                    return Some(n);
                }
            }
        }
        match self.cur {
            Some(c) if c + 1 < len => Some(c + 1),
            Some(_) if self.repeat == Repeat::All => Some(0),
            Some(_) => None,
            None => Some(0),
        }
    }

    fn prev(&mut self) {
        let pos = self.mpv.property_f64("time-pos").unwrap_or(0.0);
        match self.cur {
            Some(c) if pos <= 3.0 && c > 0 => self.play_index(c - 1),
            Some(_) if self.playing => self.seek_to(0.0),
            _ => {}
        }
    }

    fn remove_queue(&mut self, i: usize) {
        if i >= self.queue.len() {
            return;
        }
        self.queue.remove(i);
        match self.cur {
            Some(c) if i < c => self.cur = Some(c - 1),
            Some(c) if i == c => {
                if i < self.queue.len() {
                    self.play_index(i);
                } else {
                    self.stop();
                }
                return;
            }
            _ => {}
        }
        self.sync();
    }

    fn stop(&mut self) {
        let _ = self.mpv.command(&["stop"]);
        self.cur = None;
        self.loading = None;
        self.playing = false;
        self.paused = false;
        self.status.clear();
        self.sync();
        self.push_progress();
        self.lyrics_for = None;
        self.lyrics = None;
        self.lyrics_cur = -1;
        let _ = self.ui.upgrade_in_event_loop(|ui| {
            ui.global::<AppState>().set_lyrics_lines(ModelRc::new(VecModel::<LyricLine>::default()));
            ui.global::<AppState>().set_lyrics_note("".into());
            ui.global::<AppState>().set_lyrics_source("".into());
            ui.global::<AppState>().set_lyrics_current(-1);
        });
    }

    fn set_volume(&mut self, v: f32) {
        self.volume = v.clamp(0.0, 1.0);
        let _ = self.mpv.set_property("volume", &format!("{:.0}", self.volume * 100.0));
        self.dirty.set(true);
    }

    /// Loads the queue and settings saved by the previous run. Nothing starts
    /// playing; pressing play resumes the selected track where it stopped.
    fn restore(&mut self) {
        let Some(saved) = crate::state::load(&self.state_path) else {
            self.set_volume(0.8);
            return;
        };
        self.party_server = saved.party_server;
        self.auto_update = saved.auto_update_check;
        self.theme = saved.theme;
        let theme = saved.theme;
        self.layout = saved.layout;
        self.floating_player = saved.floating_player;
        let (layout, floating) = (saved.layout, saved.floating_player);
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            ui.global::<Theme>().set_palette(theme);
            ui.global::<Theme>().set_layout(layout);
            ui.global::<Theme>().set_floating_player(floating);
        });
        if !saved.display_name.is_empty() {
            self.display_name = saved.display_name;
        }
        self.queue = saved.queue;
        self.cur = saved.cur;
        self.shuffle = saved.shuffle;
        self.repeat = match saved.repeat {
            1 => Repeat::All,
            2 => Repeat::One,
            _ => Repeat::Off,
        };
        self.resume_pos = (self.cur.is_some() && saved.position > 1.0).then_some(saved.position);
        self.set_volume(saved.volume);
        if saved.perf {
            self.set_perf(true);
        }
        self.dirty.set(false);
        self.refresh_thumbs();
        let volume = self.volume;
        let _ = self.ui.upgrade_in_event_loop(move |ui| ui.global::<AppState>().set_volume(volume));
        self.sync();
        self.push_progress();
    }

    fn save_state(&mut self) {
        let position = if self.playing {
            self.mpv.property_f64("time-pos").unwrap_or(0.0)
        } else {
            self.resume_pos.unwrap_or(0.0)
        };
        let saved = crate::state::Saved {
            queue: self.queue.clone(),
            cur: self.cur,
            position,
            shuffle: self.shuffle,
            repeat: match self.repeat {
                Repeat::Off => 0,
                Repeat::All => 1,
                Repeat::One => 2,
            },
            volume: self.volume,
            perf: self.perf,
            party_server: self.party_server.clone(),
            display_name: self.display_name.clone(),
            auto_update_check: self.auto_update,
            theme: self.theme,
            layout: self.layout,
            floating_player: self.floating_player,
        };
        let _ = crate::state::save(&self.state_path, &saved);
        self.last_save = Instant::now();
        self.dirty.set(false);
    }

    fn next_rand(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    /// What the Windows media overlay should show right now
    fn now_playing(&self) -> crate::media::NowPlaying {
        use crate::media::State;
        let track = self.cur.and_then(|c| self.queue.get(c));
        let state = match (track, self.playing, self.paused) {
            (None, _, _) => State::Stopped,
            (Some(_), true, false) => State::Playing,
            _ => State::Paused,
        };
        let position_secs = if self.playing {
            self.mpv.property_f64("time-pos").unwrap_or(0.0)
        } else {
            self.resume_pos.unwrap_or(0.0)
        };
        crate::media::NowPlaying {
            id: track.map(|t| t.id.clone()).unwrap_or_default(),
            title: track.map(|t| t.title.clone()).unwrap_or_default(),
            artist: track.map(|t| t.channel.clone()).unwrap_or_default(),
            cover: track.map(|t| t.thumb.clone()).unwrap_or_default(),
            duration_secs: track.map_or(0, |t| t.duration),
            position_secs,
            state,
        }
    }

    fn push_media(&self) {
        let np = self.now_playing();
        let _ = self.ui.upgrade_in_event_loop(move |_| crate::media::update(&np));
    }

    /// Pushes everything except the progress bar to the UI
    fn sync(&self) {
        self.dirty.set(true);
        let np = self.now_playing();
        let queue = self.queue.clone();
        let cur = self.cur.map_or(-1, |c| c as i32);
        let (title, channel, artist_id) = match self.cur.and_then(|c| self.queue.get(c)) {
            Some(t) => (t.title.clone(), t.channel.clone(), t.artist_id.clone()),
            None => ("Nothing playing".to_string(), String::new(), String::new()),
        };
        // a status message takes the artist line's place, so nothing links then
        let (artist, rest) = if self.status.is_empty() { credit(&channel) } else { (String::new(), String::new()) };
        let artist_id = if self.status.is_empty() { artist_id } else { String::new() };
        let detail = if self.status.is_empty() { channel } else { self.status.clone() };
        let (paused, loading, shuffle) = (self.paused, self.loading.is_some(), self.shuffle);
        let active = self.playing || loading;
        let thumbs = self.thumbs.clone();
        let now_cover = self.cur.and_then(|c| self.queue.get(c)).and_then(|t| self.thumbs.get(&t.id)).cloned();
        let repeat = match self.repeat {
            Repeat::Off => 0,
            Repeat::All => 1,
            Repeat::One => 2,
        };
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            ui.global::<AppState>().set_queue(items(&queue, &thumbs));
            ui.global::<AppState>().set_now_tint(now_cover.as_ref().map_or(slint::Color::from_argb_u8(0, 0, 0, 0), |b| average_color(b)));
            ui.global::<AppState>().set_now_cover(now_cover.map(|b| cover(&b)).unwrap_or_default());
            ui.global::<AppState>().set_current_index(cur);
            ui.global::<AppState>().set_now_title(title.into());
            ui.global::<AppState>().set_now_detail(detail.into());
            ui.global::<AppState>().set_now_artist(artist.into());
            ui.global::<AppState>().set_now_rest(rest.into());
            ui.global::<AppState>().set_now_artist_id(artist_id.into());
            ui.global::<AppState>().set_active(active);
            ui.global::<AppState>().set_paused(paused);
            ui.global::<AppState>().set_loading(loading);
            ui.global::<AppState>().set_shuffle(shuffle);
            ui.global::<AppState>().set_repeat_mode(repeat);
            crate::media::update(&np);
        });
    }

    fn push_progress(&self) {
        let (pos, dur) = if self.playing {
            (
                self.mpv.property_f64("time-pos").unwrap_or(0.0),
                self.mpv.property_f64("duration").unwrap_or(0.0),
            )
        } else if let (Some(at), Some(track)) = (self.resume_pos, self.cur.and_then(|c| self.queue.get(c))) {
            (at, track.duration as f64)
        } else {
            (0.0, 0.0)
        };
        let fraction = if dur > 0.0 { (pos / dur) as f32 } else { 0.0 };
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            ui.global::<AppState>().set_pos_text(clock(pos).into());
            ui.global::<AppState>().set_dur_text(clock(dur).into());
            ui.global::<AppState>().set_duration_secs(dur as f32);
            ui.global::<AppState>().set_progress(fraction);
        });
    }
}

/// Playlists reuse the track row: name as title, "Playlist • 12 songs" as subtitle
fn playlist_items(playlists: &[PlaylistInfo], thumbs: &Thumbs) -> ModelRc<TrackItem> {
    let rows: Vec<TrackItem> = playlists
        .iter()
        .map(|p| TrackItem {
            id: p.id.clone().into(),
            title: p.title.clone().into(),
            channel: p.subtitle.clone().into(),
            thumb: thumbs.get(&p.id).map(|b| cover(b)).unwrap_or_default(),
            ..Default::default()
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

fn items(tracks: &[Track], thumbs: &Thumbs) -> ModelRc<TrackItem> {
    let rows: Vec<TrackItem> = tracks
        .iter()
        .map(|t| TrackItem {
            id: t.id.clone().into(),
            title: t.title.clone().into(),
            channel: t.channel.clone().into(),
            artist: credit(&t.channel).0.into(),
            rest: credit(&t.channel).1.into(),
            artist_id: t.artist_id.clone().into(),
            // an artist's top songs come without a length; the player learns it when the song starts
            duration: if t.duration == 0 { String::new() } else { clock(t.duration as f64) }.into(),
            thumb: thumbs.get(&t.id).map(|b| cover(b)).unwrap_or_default(),
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

/// "Daft Punk & Julian · Discovery" -> ("Daft Punk & Julian", "Discovery"): the part that links to the artist, and the rest
fn credit(channel: &str) -> (String, String) {
    match channel.split_once(" · ") {
        Some((artist, rest)) => (artist.to_string(), rest.to_string()),
        None => (channel.to_string(), String::new()),
    }
}

/// The average colour of a cover (RGBA pixels). The layouts that tint their background from the
/// cover use it, softened, behind everything.
fn average_color(rgba: &[u8]) -> slint::Color {
    let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
    for px in rgba.chunks_exact(4) {
        r += px[0] as u64;
        g += px[1] as u64;
        b += px[2] as u64;
        n += 1;
    }
    if n == 0 {
        return slint::Color::from_argb_u8(0, 0, 0, 0);
    }
    slint::Color::from_rgb_u8((r / n) as u8, (g / n) as u8, (b / n) as u8)
}

fn cover(rgba: &[u8]) -> Image {
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(crate::thumbs::SIZE, crate::thumbs::SIZE);
    buf.make_mut_bytes().copy_from_slice(rgba);
    Image::from_rgba8(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_covers_average_colour_is_its_mean() {
        // two pixels, red and blue, make purple; transparency is ignored
        let c = average_color(&[200, 0, 0, 255, 0, 0, 100, 255]);
        assert_eq!((c.red(), c.green(), c.blue()), (100, 0, 50));
        assert_eq!(average_color(&[]).alpha(), 0, "no cover, no tint");
    }

    #[test]
    fn moving_a_queue_row_keeps_the_playing_song_selected() {
        let run = |from, to, cur| {
            let mut q = vec!['a', 'b', 'c', 'd', 'e'];
            let mut c = Some(cur);
            assert!(move_in_queue(&mut q, &mut c, from, to));
            (q.iter().collect::<String>(), q[c.unwrap()])
        };
        // the playing song is 'c' (index 2) in the first four; 'b' is playing in the last two
        assert_eq!(run(2, 0, 2), ("cabde".to_string(), 'c'), "moving the playing song itself");
        assert_eq!(run(0, 4, 2), ("bcdea".to_string(), 'c'), "from above to below it");
        assert_eq!(run(4, 0, 2), ("eabcd".to_string(), 'c'), "from below to above it");
        assert_eq!(run(3, 4, 2), ("abced".to_string(), 'c'), "both below it");
        assert_eq!(run(1, 3, 1), ("acdbe".to_string(), 'b'));
        let mut q = vec![1, 2, 3];
        let mut c = None;
        assert!(!move_in_queue(&mut q, &mut c, 1, 1), "same place");
        assert!(!move_in_queue(&mut q, &mut c, 0, 3), "past the end");
        assert!(!move_in_queue(&mut q, &mut c, 5, 0), "from nowhere");
        assert!(move_in_queue(&mut q, &mut c, 0, 2) && c.is_none() && q == [2, 3, 1]);
    }

    #[test]
    fn server_addresses_are_made_into_websocket_urls() {
        assert_eq!(normalize_server("  relay.example.dev "), "wss://relay.example.dev");
        assert_eq!(normalize_server("https://relay.example.dev"), "wss://relay.example.dev");
        assert_eq!(normalize_server("http://localhost:8787"), "ws://localhost:8787");
        assert_eq!(normalize_server("ws://127.0.0.1:8787"), "ws://127.0.0.1:8787");
        assert_eq!(normalize_server(""), "");
    }

    #[test]
    fn room_codes_ignore_spacing_dashes_and_case() {
        assert_eq!(clean_room_code(" k7xq-29 "), "K7XQ29");
        assert_eq!(clean_room_code("--"), "");
    }

    #[test]
    fn guests_cannot_give_playback_commands_but_can_change_volume_and_browse() {
        assert!(is_playback_control(&Cmd::TogglePause));
        assert!(is_playback_control(&Cmd::Next));
        assert!(is_playback_control(&Cmd::PlayResult(0)));
        assert!(is_playback_control(&Cmd::SeekSecs(1.0)));
        assert!(!is_playback_control(&Cmd::SetVolume(0.5)));
        assert!(!is_playback_control(&Cmd::Search("x".into())));
        assert!(!is_playback_control(&Cmd::ViewChanged(1)));
        assert!(!is_playback_control(&Cmd::PartyLeave));
    }
}

fn dir_size(dir: &std::path::Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|rd| rd.flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum())
        .unwrap_or(0)
}

fn clock(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}
