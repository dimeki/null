//! Listening party client. A background thread keeps a WebSocket open to the
//! relay (see relay/relay.ts), estimates the relay's clock, and turns messages
//! into `Event`s for the engine. The sync maths lives in plain functions so it
//! can be tested without a network.

use crate::engine::Cmd;
use crate::ytdlp::Track;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tungstenite::http::Uri;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{client_tls, Message, WebSocket};

/// Upcoming songs shared with guests so they can fetch them ahead of time
pub const SHARED_UPCOMING: usize = 3;

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrackInfo {
    pub id: String,
    pub title: String,
    pub channel: String,
    pub duration: u32,
    pub thumb: String,
}

impl From<&Track> for TrackInfo {
    fn from(t: &Track) -> Self {
        TrackInfo { id: t.id.clone(), title: t.title.clone(), channel: t.channel.clone(), duration: t.duration, thumb: t.thumb.clone() }
    }
}

impl From<&TrackInfo> for Track {
    fn from(t: &TrackInfo) -> Self {
        Track { id: t.id.clone(), title: t.title.clone(), channel: t.channel.clone(), duration: t.duration, thumb: t.thumb.clone() }
    }
}

/// Ids end up in a yt-dlp URL and a file name, so a guest only accepts what YouTube issues
fn valid_id(id: &str) -> bool {
    (6..=20).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn clip(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

impl TrackInfo {
    fn to_json(&self) -> Value {
        json!({ "id": self.id, "title": self.title, "channel": self.channel, "duration": self.duration, "thumb": self.thumb })
    }

    fn from_json(v: &Value) -> Option<TrackInfo> {
        let id = v["id"].as_str().filter(|id| valid_id(id))?.to_string();
        Some(TrackInfo {
            id,
            title: clip(v["title"].as_str().unwrap_or(""), 200),
            channel: clip(v["channel"].as_str().unwrap_or(""), 200),
            duration: v["duration"].as_u64().unwrap_or(0).min(86_400) as u32,
            thumb: clip(v["thumb"].as_str().unwrap_or(""), 500),
        })
    }
}

/// What the host is doing, as sent to guests
#[derive(Clone, Debug, PartialEq)]
pub struct HostState {
    pub track: Option<TrackInfo>,
    pub playing: bool,
    /// Seconds into the song when this was sent
    pub pos: f64,
    /// Relay clock (ms) when this was sent
    pub ts: i64,
    pub next: Vec<TrackInfo>,
}

impl HostState {
    fn to_json(&self) -> Value {
        json!({
            "t": "state",
            "track": self.track.as_ref().map(TrackInfo::to_json),
            "playing": self.playing,
            "pos": self.pos,
            "ts": self.ts,
            "next": self.next.iter().map(TrackInfo::to_json).collect::<Vec<_>>(),
        })
    }

    fn from_json(v: &Value) -> Option<HostState> {
        Some(HostState {
            track: v["track"].is_object().then(|| TrackInfo::from_json(&v["track"])).flatten(),
            playing: v["playing"].as_bool()?,
            pos: v["pos"].as_f64().filter(|p| p.is_finite())?.clamp(0.0, 86_400.0),
            ts: v["ts"].as_i64()?,
            next: v["next"]
                .as_array()
                .map(|a| a.iter().filter_map(TrackInfo::from_json).take(SHARED_UPCOMING).collect())
                .unwrap_or_default(),
        })
    }
}

#[derive(Clone, Debug)]
pub struct Member {
    pub name: String,
    pub host: bool,
    /// How a guest says it is keeping up ("In sync", "Catching up…"); empty for the host
    pub sync: String,
}

pub enum Out {
    Create(String),
    Join { room: String, name: String },
    State(HostState),
    /// A guest's report of how well it is keeping up
    Status(String),
    Leave,
}

impl Out {
    fn to_json(&self) -> Value {
        match self {
            Out::Create(name) => json!({ "t": "create", "name": name }),
            Out::Join { room, name } => json!({ "t": "join", "room": room, "name": name }),
            Out::State(s) => s.to_json(),
            Out::Status(sync) => json!({ "t": "status", "sync": sync }),
            Out::Leave => json!({ "t": "leave" }),
        }
    }
}

pub enum Event {
    Created { room: String },
    Joined { room: String, host: String },
    Members(Vec<Member>),
    State(HostState),
    /// A guest joined: send a fresh state
    Hello,
    Closed(String),
    Error(String),
    Disconnected(String),
}

/// The engine's end of a party connection
pub struct Handle {
    tx: Sender<Out>,
    offset: Arc<AtomicI64>,
}

impl Handle {
    pub fn send(&self, out: Out) {
        let _ = self.tx.send(out);
    }

    /// The relay's clock (ms), estimated from ping round trips
    pub fn server_now_ms(&self) -> i64 {
        now_ms() + self.offset.load(Ordering::Relaxed)
    }
}

// ---- sync maths -------------------------------------------------------------

/// One ping round trip: `(round-trip ms, relay clock minus local clock in ms)`
pub fn offset_sample(sent_ms: i64, received_ms: i64, server_ms: i64) -> (i64, i64) {
    let rtt = (received_ms - sent_ms).max(0);
    (rtt, server_ms - (sent_ms + rtt / 2))
}

/// Keeps recent ping samples and trusts the one with the shortest round trip
#[derive(Default)]
pub struct ClockSync {
    samples: VecDeque<(i64, i64)>,
}

impl ClockSync {
    pub fn add(&mut self, rtt: i64, offset: i64) {
        self.samples.push_back((rtt, offset));
        if self.samples.len() > 8 {
            self.samples.pop_front();
        }
    }

    pub fn offset(&self) -> i64 {
        self.samples.iter().min_by_key(|s| s.0).map_or(0, |s| s.1)
    }
}

/// Where the host's song should be right now
pub fn target_position(host: &HostState, server_now_ms: i64) -> f64 {
    if host.playing {
        (host.pos + (server_now_ms - host.ts) as f64 / 1000.0).max(0.0)
    } else {
        host.pos
    }
}

#[derive(Debug, PartialEq)]
pub enum Correction {
    InSync,
    /// Far off: jump to this position
    Seek(f64),
    /// A little off: play at this speed until caught up
    Speed(f64),
}

/// How a guest should react to being `actual` seconds into a song that should be at `target`
pub fn correction(target: f64, actual: f64) -> Correction {
    let behind = target - actual;
    if behind.abs() > 1.5 {
        Correction::Seek(target)
    } else if behind > 0.15 {
        Correction::Speed(1.05)
    } else if behind < -0.15 {
        Correction::Speed(0.95)
    } else {
        Correction::InSync
    }
}

// ---- connection -------------------------------------------------------------

/// Starts the connection thread. Queue messages with `Handle::send`; replies arrive
/// as `Cmd::Party` on `events`. Dropping the handle closes the connection.
pub fn spawn(url: String, events: Sender<Cmd>) -> Handle {
    let (tx, rx) = channel::<Out>();
    let offset = Arc::new(AtomicI64::new(0));
    let shared_offset = offset.clone();
    std::thread::spawn(move || {
        if let Err(message) = run(&url, rx, &events, &shared_offset) {
            let _ = events.send(Cmd::Party(Event::Disconnected(message)));
        }
    });
    Handle { tx, offset }
}

fn connect(url: &str) -> Result<WebSocket<MaybeTlsStream<TcpStream>>, String> {
    let uri: Uri = url.trim().parse().map_err(|_| "That party server address isn't valid".to_string())?;
    let secure = match uri.scheme_str() {
        Some("wss") => true,
        Some("ws") => false,
        _ => return Err("The party server address must start with ws:// or wss://".into()),
    };
    let host = uri.host().ok_or("That party server address isn't valid")?.to_string();
    let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
    let addr = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| format!("Couldn't find the party server: {e}"))?
        .next()
        .ok_or("Couldn't find the party server")?;
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(8))
        .map_err(|e| format!("Couldn't reach the party server: {e}"))?;
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let (ws, _) = client_tls(url.trim(), stream).map_err(|e| format!("The party server refused the connection: {e}"))?;
    Ok(ws)
}

fn set_read_timeout(ws: &mut WebSocket<MaybeTlsStream<TcpStream>>, timeout: Duration) {
    match ws.get_mut() {
        MaybeTlsStream::Plain(s) => {
            let _ = s.set_read_timeout(Some(timeout));
        }
        MaybeTlsStream::Rustls(s) => {
            let _ = s.sock.set_read_timeout(Some(timeout));
        }
        _ => {}
    }
}

fn write(ws: &mut WebSocket<MaybeTlsStream<TcpStream>>, value: Value) -> Result<(), String> {
    ws.send(Message::text(value.to_string())).map_err(|_| LOST.to_string())
}

/// Shown when a party's connection drops. The OS error behind it is not useful to read.
const LOST: &str = "Lost the connection to the party server";

/// Returns `Ok` after an orderly leave and `Err(reason)` when the connection is lost
fn run(url: &str, rx: Receiver<Out>, events: &Sender<Cmd>, offset: &AtomicI64) -> Result<(), String> {
    let mut ws = connect(url)?;
    set_read_timeout(&mut ws, Duration::from_millis(100));
    let mut clock = ClockSync::default();
    let (mut next_ping, mut quick_pings) = (Instant::now(), 4u32);
    loop {
        loop {
            match rx.try_recv() {
                Ok(Out::Leave) => {
                    let _ = write(&mut ws, Out::Leave.to_json());
                    let _ = ws.close(None);
                    let _ = ws.flush();
                    return Ok(());
                }
                Ok(out) => write(&mut ws, out.to_json())?,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    let _ = write(&mut ws, Out::Leave.to_json());
                    let _ = ws.close(None);
                    return Ok(());
                }
            }
        }
        if Instant::now() >= next_ping {
            write(&mut ws, json!({ "t": "ping", "c": now_ms() }))?;
            // a few quick pings to settle the clock estimate, then a slow keep-alive
            next_ping = Instant::now() + if quick_pings > 0 { Duration::from_millis(300) } else { Duration::from_secs(15) };
            quick_pings = quick_pings.saturating_sub(1);
        }
        match ws.read() {
            Ok(Message::Text(text)) => {
                if let Some(event) = handle_message(&text, &mut clock, offset) {
                    if events.send(Cmd::Party(event)).is_err() {
                        return Ok(());
                    }
                }
            }
            Ok(Message::Close(_)) => return Err("The party server closed the connection".into()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(_) => return Err(LOST.to_string()),
        }
    }
}

fn handle_message(text: &str, clock: &mut ClockSync, offset: &AtomicI64) -> Option<Event> {
    let v: Value = serde_json::from_str(text).ok()?;
    match v["t"].as_str()? {
        "pong" => {
            let (rtt, off) = offset_sample(v["c"].as_i64()?, now_ms(), v["s"].as_i64()?);
            clock.add(rtt, off);
            offset.store(clock.offset(), Ordering::Relaxed);
            None
        }
        "created" => Some(Event::Created { room: v["room"].as_str()?.to_string() }),
        "joined" => Some(Event::Joined {
            room: v["room"].as_str()?.to_string(),
            host: clip(v["host"].as_str().unwrap_or("the host"), 40),
        }),
        "members" => Some(Event::Members(
            v["members"]
                .as_array()?
                .iter()
                .map(|m| Member {
                    name: clip(m["name"].as_str().unwrap_or("Guest"), 40),
                    host: m["host"].as_bool().unwrap_or(false),
                    sync: clip(m["sync"].as_str().unwrap_or(""), 24),
                })
                .collect(),
        )),
        "state" => HostState::from_json(&v).map(Event::State),
        "hello" => Some(Event::Hello),
        "closed" => Some(Event::Closed(clip(v["reason"].as_str().unwrap_or("The party ended"), 120))),
        "error" => Some(Event::Error(clip(v["message"].as_str().unwrap_or("Something went wrong"), 120))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn info(id: &str) -> TrackInfo {
        TrackInfo { id: id.into(), title: "T".into(), channel: "C".into(), duration: 200, thumb: String::new() }
    }

    fn state(playing: bool, pos: f64, ts: i64) -> HostState {
        HostState { track: Some(info("abcdefghijk")), playing, pos, ts, next: vec![] }
    }

    #[test]
    fn clock_offset_assumes_a_symmetric_trip() {
        // sent at 1000, answered at 1100: a 100 ms round trip. The relay stamped 5050 when
        // our clock read about 1050, so the relay is 4000 ms ahead.
        assert_eq!(offset_sample(1000, 1100, 5050), (100, 4000));
    }

    #[test]
    fn the_fastest_round_trip_wins() {
        let mut c = ClockSync::default();
        c.add(300, 4100);
        c.add(40, 4000);
        c.add(250, 3900);
        assert_eq!(c.offset(), 4000);
    }

    #[test]
    fn target_advances_only_while_the_host_is_playing() {
        assert_eq!(target_position(&state(true, 10.0, 1000), 3500), 12.5);
        assert_eq!(target_position(&state(false, 10.0, 1000), 3500), 10.0);
        assert_eq!(target_position(&state(true, 0.0, 5000), 4000), 0.0, "never negative");
    }

    #[test]
    fn small_drift_adjusts_speed_and_big_drift_seeks() {
        assert_eq!(correction(30.0, 30.05), Correction::InSync);
        assert_eq!(correction(30.0, 29.5), Correction::Speed(1.05), "behind: speed up");
        assert_eq!(correction(30.0, 30.6), Correction::Speed(0.95), "ahead: slow down");
        assert_eq!(correction(30.0, 20.0), Correction::Seek(30.0));
        assert_eq!(correction(10.0, 14.0), Correction::Seek(10.0));
    }

    #[test]
    fn state_survives_a_round_trip() {
        let mut s = state(true, 12.5, 99);
        s.next = vec![info("nextnextnext")];
        let back = HostState::from_json(&s.to_json()).unwrap();
        assert_eq!(back, s);
        let idle = HostState { track: None, playing: false, pos: 0.0, ts: 5, next: vec![] };
        assert_eq!(HostState::from_json(&idle.to_json()).unwrap(), idle);
    }

    #[test]
    fn a_guest_refuses_ids_that_could_be_abused() {
        for bad in ["", "short", "../../etc/passwd", "a b c d e f g", "x&y=1;rm -rf", "waytoolongtobeayoutubeid1234"] {
            let v = json!({ "t": "state", "playing": true, "pos": 1.0, "ts": 1, "track": { "id": bad } });
            let parsed = HostState::from_json(&v).unwrap();
            assert!(parsed.track.is_none(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn member_lists_carry_each_guests_sync_status() {
        let mut clock = ClockSync::default();
        let off = AtomicI64::new(0);
        let msg = r#"{"t":"members","members":[{"id":"1","name":"Host","host":true,"sync":""},{"id":"2","name":"Guest","host":false,"sync":"Catching up…"}]}"#;
        let Some(Event::Members(list)) = handle_message(msg, &mut clock, &off) else { panic!("expected members") };
        assert_eq!(list.len(), 2);
        assert!(list[0].host && list[0].sync.is_empty());
        assert_eq!(list[1].sync, "Catching up…");
        assert_eq!(Out::Status("In sync".into()).to_json(), json!({ "t": "status", "sync": "In sync" }));
    }

    #[test]
    fn garbage_from_the_network_is_ignored() {
        let mut clock = ClockSync::default();
        let off = AtomicI64::new(0);
        for junk in ["not json", "[]", "{}", r#"{"t":"state"}"#, r#"{"t":"state","playing":true,"pos":"x","ts":1}"#, r#"{"t":"nope"}"#] {
            assert!(handle_message(junk, &mut clock, &off).is_none(), "{junk}");
        }
    }

    #[test]
    fn talks_to_a_relay_and_learns_its_clock() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            while let Ok(msg) = ws.read() {
                let Message::Text(text) = msg else { continue };
                let v: Value = serde_json::from_str(&text).unwrap();
                let reply = match v["t"].as_str().unwrap() {
                    // pretend the relay's clock is 5 seconds ahead of ours
                    "ping" => json!({ "t": "pong", "c": v["c"], "s": now_ms() + 5000 }),
                    "create" => json!({ "t": "created", "room": "ABC234" }),
                    "leave" => break,
                    _ => continue,
                };
                ws.send(Message::text(reply.to_string())).unwrap();
            }
        });

        let (tx, rx) = channel::<Cmd>();
        let handle = spawn(format!("ws://127.0.0.1:{port}"), tx);
        handle.send(Out::Create("Me".into()));
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut room = None;
        while Instant::now() < deadline && room.is_none() {
            if let Ok(Cmd::Party(Event::Created { room: r })) = rx.recv_timeout(Duration::from_millis(200)) {
                room = Some(r);
            }
        }
        assert_eq!(room.as_deref(), Some("ABC234"));
        std::thread::sleep(Duration::from_millis(800)); // let the quick pings finish
        let skew = handle.server_now_ms() - now_ms();
        assert!((4900..=5100).contains(&skew), "clock skew learned: {skew} ms");
        handle.send(Out::Leave);
    }

    #[test]
    fn an_unreachable_server_is_reported_not_hung() {
        let (tx, rx) = channel::<Cmd>();
        let _handle = spawn("ws://127.0.0.1:1".into(), tx);
        match rx.recv_timeout(Duration::from_secs(12)) {
            Ok(Cmd::Party(Event::Disconnected(msg))) => assert!(msg.contains("party server"), "{msg}"),
            _ => panic!("expected a disconnect event"),
        }
        let (tx, rx) = channel::<Cmd>();
        let _h = spawn("http://example.com".into(), tx);
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Cmd::Party(Event::Disconnected(msg))) => assert!(msg.contains("ws://"), "{msg}"),
            _ => panic!("expected a disconnect event"),
        }
    }
}
