//! YouTube Music's internal "Innertube" API: song search and the signed-in
//! library. Responses are deeply nested and change shape from time to time, so
//! the parsers search for renderer objects by name instead of following fixed paths.

use crate::cookies::Auth;
use crate::ytdlp::Track;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const ORIGIN: &str = "https://music.youtube.com";
const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36";
/// Search filter that limits results to songs
const SONGS_FILTER: &str = "EgWKAQIIAWoMEA4QChADEAQQCRAF";
/// Extra pages of a long playlist to follow after the first one
const MAX_CONTINUATIONS: usize = 6;

#[derive(Clone)]
pub struct PlaylistInfo {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    /// Cover art URL; empty when there is none (e.g. "Liked songs")
    pub thumb: String,
}

#[derive(Clone)]
pub struct Client {
    agent: ureq::Agent,
    auth: Option<Auth>,
}

impl Client {
    pub fn new(auth: Option<Auth>) -> Self {
        let agent: ureq::Agent =
            ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(20))).build().into();
        Client { agent, auth }
    }

    fn post(&self, endpoint: &str, mut body: Value) -> Result<Value, String> {
        body["context"] = json!({
            "client": { "clientName": "WEB_REMIX", "clientVersion": "1.20250101.01.00", "hl": "en", "gl": "US" },
            "user": {}
        });
        let url = format!("{ORIGIN}/youtubei/v1/{endpoint}?prettyPrint=false");
        let mut req = self
            .agent
            .post(&url)
            .header("User-Agent", USER_AGENT)
            .header("Origin", ORIGIN)
            .header("Referer", format!("{ORIGIN}/"))
            .header("Content-Type", "application/json")
            .header("X-Goog-AuthUser", "0");
        if let Some(auth) = &self.auth {
            req = req.header("Cookie", &auth.cookie_header).header("Authorization", authorization(auth));
        }
        let mut resp = req.send(body.to_string().as_bytes()).map_err(describe_error)?;
        let text = resp.body_mut().read_to_string().map_err(|e| format!("Couldn't read the response: {e}"))?;
        serde_json::from_str(&text).map_err(|e| format!("Unexpected response: {e}"))
    }

    pub fn search_songs(&self, query: &str) -> Result<Vec<Track>, String> {
        let json = self.post("search", json!({ "query": query, "params": SONGS_FILTER }))?;
        Ok(tracks_in(&json))
    }

    /// An artist's page: top songs, albums and singles
    pub fn artist(&self, artist_id: &str) -> Result<ArtistPage, String> {
        if !crate::party::valid_artist_id(artist_id) {
            return Err("That isn't an artist page".into());
        }
        let page = artist_in(&self.post("browse", json!({ "browseId": artist_id }))?);
        if page.name.is_empty() {
            return Err("That artist page couldn't be read".into());
        }
        Ok(page)
    }

    /// The songs on an album or single. Album pages list no covers or album names of their
    /// own, so the cover and title the artist page showed are filled in.
    pub fn album_tracks(&self, album_id: &str, title: &str, cover: &str) -> Result<Vec<Track>, String> {
        if !valid_album_id(album_id) {
            return Err("That isn't an album".into());
        }
        let mut tracks = tracks_in(&self.post("browse", json!({ "browseId": album_id }))?);
        for t in &mut tracks {
            if t.thumb.is_empty() {
                t.thumb = cover.to_string();
            }
            if !t.channel.contains(" · ") && !title.is_empty() {
                t.channel = format!("{} · {title}", t.channel);
            }
        }
        Ok(tracks)
    }

    /// Plain-text lyrics from YouTube Music, if it has any for this song
    pub fn lyrics(&self, video_id: &str) -> Result<Option<String>, String> {
        let next = self.post("next", json!({ "videoId": video_id, "isAudioOnly": true }))?;
        // the "Lyrics" tab of the now-playing page points at a browse id starting with MPLYt
        let mut ids = Vec::new();
        find_all(&next, "browseId", &mut ids);
        let Some(browse_id) = ids.into_iter().filter_map(Value::as_str).find(|id| id.starts_with("MPLYt")) else {
            return Ok(None);
        };
        let page = self.post("browse", json!({ "browseId": browse_id }))?;
        let mut shelves = Vec::new();
        find_all(&page, "musicDescriptionShelfRenderer", &mut shelves);
        Ok(shelves
            .into_iter()
            .map(|s| runs_text(&s["description"]["runs"]))
            .find(|text| !text.trim().is_empty()))
    }

    /// The signed-in user's playlists, with "Liked songs" first
    pub fn library_playlists(&self) -> Result<Vec<PlaylistInfo>, String> {
        if self.auth.is_none() {
            return Err("Not signed in".into());
        }
        let json = self.post("browse", json!({ "browseId": "FEmusic_liked_playlists" }))?;
        ensure_logged_in(&json)?;
        let mut out = vec![PlaylistInfo {
            id: "LM".into(),
            title: "Liked songs".into(),
            subtitle: "Auto playlist".into(),
            thumb: String::new(),
        }];
        out.extend(playlists_in(&json));
        Ok(out)
    }

    pub fn playlist_tracks(&self, playlist_id: &str) -> Result<Vec<Track>, String> {
        if self.auth.is_none() {
            return Err("Not signed in".into());
        }
        let json = self.post("browse", json!({ "browseId": format!("VL{playlist_id}") }))?;
        ensure_logged_in(&json)?;
        let mut tracks = tracks_in(&json);
        let mut token = continuation_token(&json);
        for _ in 0..MAX_CONTINUATIONS {
            let Some(t) = token.take() else { break };
            // a failed extra page just means a shorter list
            let Ok(next) = self.post("browse", json!({ "continuation": t })) else { break };
            tracks.extend(tracks_in(&next));
            token = continuation_token(&next);
        }
        Ok(tracks)
    }
}

fn authorization(auth: &Auth) -> String {
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let digest = Sha1::digest(format!("{ts} {} {ORIGIN}", auth.sapisid).as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("SAPISIDHASH {ts}_{hex}")
}

/// YouTube answers 200 with an anonymous page when it doesn't accept the cookies,
/// but says so in the response's tracking params
fn ensure_logged_in(json: &Value) -> Result<(), String> {
    let mut params = Vec::new();
    find_all(json, "params", &mut params);
    let marker = params.into_iter().filter_map(Value::as_array).flatten().find(|p| p["key"] == "logged_in");
    match marker {
        Some(m) if m["value"] != "1" => {
            Err("YouTube didn't accept the sign-in. Sign out and import fresh cookies.".into())
        }
        _ => Ok(()),
    }
}

fn describe_error(e: ureq::Error) -> String {
    match e {
        ureq::Error::StatusCode(401 | 403) => "YouTube rejected the sign-in. Import your cookies again.".into(),
        ureq::Error::StatusCode(code) => format!("YouTube Music returned HTTP {code}"),
        other => format!("Couldn't reach YouTube Music: {other}"),
    }
}

/// Collects every value stored under `key`, at any depth
fn find_all<'a>(v: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(map) => {
            for (k, val) in map {
                if k == key {
                    out.push(val);
                } else {
                    find_all(val, key, out);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|i| find_all(i, key, out)),
        _ => {}
    }
}

fn first_string<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    let mut found = Vec::new();
    find_all(v, key, &mut found);
    found.into_iter().find_map(Value::as_str)
}

fn runs_text(runs: &Value) -> String {
    runs.as_array().map(|r| r.iter().filter_map(|x| x["text"].as_str()).collect::<String>()).unwrap_or_default()
}

fn continuation_token(v: &Value) -> Option<String> {
    let mut found = Vec::new();
    find_all(v, "continuationCommand", &mut found);
    found.into_iter().find_map(|c| c["token"].as_str().map(str::to_string))
}

/// The playlist cards on the library page, without the built-in "Liked songs"
fn playlists_in(json: &Value) -> Vec<PlaylistInfo> {
    let mut cards = Vec::new();
    find_all(json, "musicTwoRowItemRenderer", &mut cards);
    let mut out: Vec<PlaylistInfo> = Vec::new();
    for card in cards {
        let Some(browse_id) = card["navigationEndpoint"]["browseEndpoint"]["browseId"].as_str() else { continue };
        let Some(id) = browse_id.strip_prefix("VL") else { continue };
        if id == "LM" || out.iter().any(|p| p.id == id) {
            continue;
        }
        let title = runs_text(&card["title"]["runs"]);
        if title.is_empty() {
            continue;
        }
        out.push(PlaylistInfo {
            id: id.to_string(),
            title,
            subtitle: runs_text(&card["subtitle"]["runs"]),
            thumb: last_thumbnail(&card["thumbnailRenderer"]),
        });
    }
    out
}

/// The largest listed image under `v`, or an empty string
fn last_thumbnail(v: &Value) -> String {
    let mut found = Vec::new();
    find_all(v, "thumbnails", &mut found);
    found
        .into_iter()
        .filter_map(|t| t.as_array())
        .flatten()
        .filter_map(|t| t["url"].as_str())
        .last()
        .unwrap_or_default()
        .to_string()
}

fn tracks_in(json: &Value) -> Vec<Track> {
    let mut items = Vec::new();
    find_all(json, "musicResponsiveListItemRenderer", &mut items);
    items.into_iter().filter_map(parse_track).collect()
}

/// Splits one column's runs into segments at the " • " separators
fn segments(runs: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for text in runs.as_array().into_iter().flatten().filter_map(|r| r["text"].as_str()) {
        if text.trim() == "•" {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push_str(text);
        }
    }
    out.push(cur);
    out.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

fn parse_duration(s: &str) -> Option<u32> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() < 2 || parts.len() > 3 || parts.iter().any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    Some(parts.iter().fold(0u32, |acc, p| acc * 60 + p.parse::<u32>().unwrap_or(0)))
}

fn parse_track(item: &Value) -> Option<Track> {
    // rows without a video id are unavailable or not songs (artists, albums)
    let id = item["playlistItemData"]["videoId"].as_str().or_else(|| first_string(item, "videoId"))?.to_string();

    let flex: Vec<&Value> = item["flexColumns"].as_array()?.iter().collect();
    let title = runs_text(&flex.first()?["musicResponsiveListItemFlexColumnRenderer"]["text"]["runs"]);
    if title.is_empty() {
        return None;
    }

    let mut segs: Vec<String> = Vec::new();
    for col in flex.iter().skip(1) {
        segs.extend(segments(&col["musicResponsiveListItemFlexColumnRenderer"]["text"]["runs"]));
    }
    for col in item["fixedColumns"].as_array().into_iter().flatten() {
        segs.extend(segments(&col["musicResponsiveListItemFixedColumnRenderer"]["text"]["runs"]));
    }
    let duration = segs.iter().rev().find_map(|s| parse_duration(s)).unwrap_or(0);
    let mut details = segs.into_iter().filter(|s| {
        parse_duration(s).is_none()
            && !matches!(s.as_str(), "Song" | "Video")
            && !s.ends_with(" plays")
            && !s.ends_with(" views")
    });
    let artist = details.next().unwrap_or_default();
    let channel = match details.next() {
        Some(album) => format!("{artist} · {album}"),
        None => artist,
    };

    let thumb = last_thumbnail(&item["thumbnail"]);

    let artist_id = flex.get(1).map_or_else(String::new, |c| artist_id_in(&c["musicResponsiveListItemFlexColumnRenderer"]["text"]["runs"]));

    Some(Track { id, title, channel, duration, thumb, artist_id })
}

/// The page id of the first artist named in a column of runs ("A & B • Album" gives A's)
fn artist_id_in(runs: &Value) -> String {
    runs.as_array()
        .into_iter()
        .flatten()
        .find_map(|r| r["navigationEndpoint"]["browseEndpoint"]["browseId"].as_str().filter(|id| crate::party::valid_artist_id(id)))
        .unwrap_or_default()
        .to_string()
}

/// An album or single on an artist's page
#[derive(Clone, Debug, PartialEq)]
pub struct AlbumInfo {
    pub id: String,
    pub title: String,
    /// "Album • 2023"
    pub subtitle: String,
    pub thumb: String,
}

#[derive(Clone, Debug, Default)]
pub struct ArtistPage {
    pub name: String,
    /// "67.8M monthly audience", when YouTube Music says
    pub subtitle: String,
    pub image: String,
    pub songs: Vec<Track>,
    pub albums: Vec<AlbumInfo>,
}

/// Album ids are fetched like artist ids, so only YouTube Music's own form is accepted
fn valid_album_id(id: &str) -> bool {
    id.starts_with("MPRE") && id.len() <= 40 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A square crop of a (wide) artist photo, small enough for an avatar
fn avatar_url(url: &str) -> String {
    match url.split_once('=') {
        Some((base, _)) if !base.is_empty() => format!("{base}=w120-h120-l90-rj-c"),
        _ => url.to_string(),
    }
}

fn artist_in(json: &Value) -> ArtistPage {
    let mut headers = Vec::new();
    find_all(json, "musicImmersiveHeaderRenderer", &mut headers);
    find_all(json, "musicVisualHeaderRenderer", &mut headers);
    let header = headers.first().copied().unwrap_or(&Value::Null);
    let name = runs_text(&header["title"]["runs"]);
    let subtitle = runs_text(&header["monthlyListenerCount"]["runs"]);
    let image = avatar_url(&last_thumbnail(&header["thumbnail"]));

    // "Top songs" is the page's one list of song rows
    let mut shelves = Vec::new();
    find_all(json, "musicShelfRenderer", &mut shelves);
    let songs = shelves.first().map(|s| tracks_in(s)).unwrap_or_default();

    let mut albums: Vec<AlbumInfo> = Vec::new();
    let mut carousels = Vec::new();
    find_all(json, "musicCarouselShelfRenderer", &mut carousels);
    for shelf in carousels {
        let title = runs_text(&shelf["header"]["musicCarouselShelfBasicHeaderRenderer"]["title"]["runs"]);
        if !matches!(title.as_str(), "Albums" | "Singles & EPs") {
            continue;
        }
        for card in shelf["contents"].as_array().into_iter().flatten() {
            let card = &card["musicTwoRowItemRenderer"];
            let Some(id) = card["navigationEndpoint"]["browseEndpoint"]["browseId"].as_str().filter(|id| valid_album_id(id)) else { continue };
            let item_title = runs_text(&card["title"]["runs"]);
            if item_title.is_empty() || albums.iter().any(|a| a.id == id) {
                continue;
            }
            let sub = runs_text(&card["subtitle"]["runs"]);
            // albums are listed with just a year, singles already say "Single"
            let subtitle = if sub.contains('•') || sub.is_empty() { sub } else { format!("Album • {sub}") };
            albums.push(AlbumInfo { id: id.to_string(), title: item_title, subtitle, thumb: last_thumbnail(&card["thumbnailRenderer"]) });
        }
    }
    ArtistPage { name, subtitle, image, songs, albums }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_song_search_row() {
        let row = json!({
            "playlistItemData": { "videoId": "fa5IWHDbftI" },
            "thumbnail": { "musicThumbnailRenderer": { "thumbnail": { "thumbnails": [
                { "url": "https://yt3.googleusercontent.com/small=w60-h60" },
                { "url": "https://yt3.googleusercontent.com/big=w120-h120" } ] } } },
            "flexColumns": [
                { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [{ "text": "One More Time" }] } } },
                { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [
                    { "text": "Daft Punk" }, { "text": " • " }, { "text": "Discovery" }, { "text": " • " }, { "text": "5:21" }] } } },
                { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [{ "text": "854M plays" }] } } }
            ]
        });
        let t = parse_track(&row).unwrap();
        assert_eq!(t.id, "fa5IWHDbftI");
        assert_eq!(t.title, "One More Time");
        assert_eq!(t.channel, "Daft Punk · Discovery");
        assert_eq!(t.duration, 321);
        assert!(t.thumb.ends_with("w120-h120"));
    }

    #[test]
    fn parses_a_playlist_row_with_separate_columns() {
        let row = json!({
            "playlistItemData": { "videoId": "abc" },
            "flexColumns": [
                { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [{ "text": "Song name" }] } } },
                { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [
                    { "text": "A" }, { "text": " & " }, { "text": "B" }] } } },
                { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [{ "text": "The Album" }] } } }
            ],
            "fixedColumns": [
                { "musicResponsiveListItemFixedColumnRenderer": { "text": { "runs": [{ "text": "1:02:03" }] } } }
            ]
        });
        let t = parse_track(&row).unwrap();
        assert_eq!(t.channel, "A & B · The Album");
        assert_eq!(t.duration, 3723);
    }

    #[test]
    fn detects_an_anonymous_response() {
        let page = |v: &str| {
            json!({ "responseContext": { "serviceTrackingParams": [
                { "service": "GFEEDBACK", "params": [{ "key": "logged_in", "value": v }] } ] } })
        };
        assert!(ensure_logged_in(&page("1")).is_ok());
        assert!(ensure_logged_in(&page("0")).is_err());
        // a response without the marker is given the benefit of the doubt
        assert!(ensure_logged_in(&json!({})).is_ok());
    }

    #[test]
    fn parses_library_playlist_cards_with_covers() {
        let card = |id: &str, title: &str, url: Option<&str>| {
            let mut c = json!({
                "title": { "runs": [{ "text": title }] },
                "subtitle": { "runs": [{ "text": "Someone" }, { "text": " • " }, { "text": "38 tracks" }] },
                "navigationEndpoint": { "browseEndpoint": { "browseId": id } }
            });
            if let Some(u) = url {
                c["thumbnailRenderer"] = json!({ "musicThumbnailRenderer": { "thumbnail": { "thumbnails": [
                    { "url": "https://lh3.googleusercontent.com/small" }, { "url": u }] } } });
            }
            json!({ "musicTwoRowItemRenderer": c })
        };
        let page = json!({ "contents": [
            card("VLPLabc", "gym rx", Some("https://lh3.googleusercontent.com/big")),
            card("VLLM", "Liked Music", None),
            card("VLPLabc", "duplicate", None),
            card("MPREb_album", "not a playlist", None),
            card("VLPLnocover", "no cover", None),
        ] });
        let lists = playlists_in(&page);
        assert_eq!(lists.len(), 2, "Liked Music, duplicates and non-playlists are skipped");
        assert_eq!(lists[0].id, "PLabc");
        assert_eq!(lists[0].subtitle, "Someone • 38 tracks");
        assert_eq!(lists[0].thumb, "https://lh3.googleusercontent.com/big");
        assert_eq!(lists[1].thumb, "");
    }

    fn song_row(video: &str, title: &str, artist_runs: Value) -> Value {
        json!({ "musicResponsiveListItemRenderer": {
            "playlistItemData": { "videoId": video },
            "flexColumns": [
                { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [{ "text": title }] } } },
                { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": artist_runs } } },
                { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [{ "text": "1.2B plays" }] } } }
            ]
        } })
    }

    fn artist_link(name: &str, id: &str) -> Value {
        json!({ "text": name, "navigationEndpoint": { "browseEndpoint": { "browseId": id } } })
    }

    #[test]
    fn songs_know_their_first_artist() {
        let both = json!([artist_link("Daft Punk", "UCRr1xG_2WIDs18a6cIiCxeA"), { "text": " & " }, artist_link("Julian", "UCWhpbdGLnR8XtXLr0yl2nYA")]);
        let t = parse_track(&song_row("khnokW3Mw24", "Instant Crush", both)["musicResponsiveListItemRenderer"]).unwrap();
        assert_eq!(t.artist_id, "UCRr1xG_2WIDs18a6cIiCxeA");
        assert_eq!(t.channel, "Daft Punk & Julian");
        // an album link is not an artist, and a row with no link has no artist page
        let album_only = json!([{ "text": "Someone" }, artist_link("An album", "MPREb_K8qWMWVqXGi")]);
        let t = parse_track(&song_row("abcdefghijk", "Song", album_only)["musicResponsiveListItemRenderer"]).unwrap();
        assert_eq!(t.artist_id, "");
    }

    #[test]
    fn parses_an_artist_page() {
        let card = |id: &str, title: &str, sub: &str| {
            json!({ "musicTwoRowItemRenderer": {
                "title": { "runs": [{ "text": title }] },
                "subtitle": { "runs": [{ "text": sub }] },
                "navigationEndpoint": { "browseEndpoint": { "browseId": id } },
                "thumbnailRenderer": { "musicThumbnailRenderer": { "thumbnail": { "thumbnails": [{ "url": "https://x/small" }, { "url": "https://x/cover" }] } } }
            } })
        };
        let shelf = |title: &str, cards: Vec<Value>| {
            json!({ "musicCarouselShelfRenderer": {
                "header": { "musicCarouselShelfBasicHeaderRenderer": { "title": { "runs": [{ "text": title }] } } },
                "contents": cards
            } })
        };
        let page = json!({
            "header": { "musicImmersiveHeaderRenderer": {
                "title": { "runs": [{ "text": "Daft Punk" }] },
                "monthlyListenerCount": { "runs": [{ "text": "67.8M monthly audience" }] },
                "thumbnail": { "musicThumbnailRenderer": { "thumbnail": { "thumbnails": [
                    { "url": "https://lh3.googleusercontent.com/abc=w1080-h450-p-l90-rj" }] } } }
            } },
            "contents": [
                { "musicShelfRenderer": { "title": { "runs": [{ "text": "Top songs" }] }, "contents": [
                    song_row("khnokW3Mw24", "Instant Crush", json!([artist_link("Daft Punk", "UCRr1xG_2WIDs18a6cIiCxeA")])),
                    song_row("abcdefghijk", "Another", json!([artist_link("Daft Punk", "UCRr1xG_2WIDs18a6cIiCxeA")])) ] } },
                shelf("Albums", vec![card("MPREb_OgeEnoHTsCm", "Random Access Memories", "2013"), card("MPREb_OgeEnoHTsCm", "dup", "2013")]),
                shelf("Singles & EPs", vec![card("MPREb_X1DQ1j0PPrX", "GLBTM", "Single • 2023")]),
                shelf("Videos", vec![card("MPREb_video", "a video", "Daft Punk • 907M views")]),
                shelf("Fans might also like", vec![card("UCq7VAQmXDurV-9VN2bSFXvw", "Gesaffelstein", "23.9M monthly audience")]),
            ]
        });
        let a = artist_in(&page);
        assert_eq!(a.name, "Daft Punk");
        assert_eq!(a.subtitle, "67.8M monthly audience");
        assert_eq!(a.image, "https://lh3.googleusercontent.com/abc=w120-h120-l90-rj-c", "a small square crop");
        assert_eq!(a.songs.len(), 2);
        assert_eq!(a.songs[0].title, "Instant Crush");
        let ids: Vec<&str> = a.albums.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(ids, ["MPREb_OgeEnoHTsCm", "MPREb_X1DQ1j0PPrX"], "only albums and singles, once each");
        assert_eq!(a.albums[0].subtitle, "Album • 2013");
        assert_eq!(a.albums[1].subtitle, "Single • 2023");
        assert_eq!(a.albums[0].thumb, "https://x/cover");
        // a page that isn't an artist's yields no name, which the client reports as an error
        assert!(artist_in(&json!({ "contents": [] })).name.is_empty());
    }

    #[test]
    fn only_youtube_music_ids_are_fetched() {
        assert!(valid_album_id("MPREb_OgeEnoHTsCm"));
        for bad in ["", "UCRr1xG_2WIDs18a6cIiCxeA", "MPREb/../x", "MPRE b"] {
            assert!(!valid_album_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn skips_rows_without_a_video() {
        let row = json!({ "flexColumns": [
            { "musicResponsiveListItemFlexColumnRenderer": { "text": { "runs": [{ "text": "An artist" }] } } } ] });
        assert!(parse_track(&row).is_none());
    }
}
