//! Spotify track, album and playlist links, without any Spotify account.
//!
//! Metadata comes from Spotify's public pages; audio comes from YouTube
//! Music (or YouTube) through yt-dlp, matched by title, artists and duration.
//!
//! 1. The embed page (`open.spotify.com/embed/<kind>/<id>`) carries a
//!    `__NEXT_DATA__` JSON with the name, up to 100 tracks and an anonymous
//!    access token.
//! 2. With that token, the web player's GraphQL endpoint
//!    (`api-partner.spotify.com/pathfinder/v1/query`, operations
//!    `fetchPlaylist`, `getAlbum`, `getTrack`) pages through the full track
//!    list with album names and track numbers. It only accepts "persisted
//!    queries" identified by a hash that changes when Spotify updates its
//!    web player: the worker tries known hashes, then reads the current
//!    ones from the web player's JavaScript and caches them in the work
//!    directory. Variables a new query version requires (booleans) are
//!    added as `false` when the error names them.
//! 3. If step 2 fails, the request goes on with the embed's tracks and says
//!    so in `warnings` when that list may be cut at 100.
//!
//! None of this is a documented API; Spotify can change it at any time.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use lofty::config::WriteOptions;
use lofty::prelude::*;
use lofty::tag::{ItemKey, Tag};
use serde_json::{Value, json};

use crate::child::Stop;
use crate::http::Http;
use crate::job::{Job, SourceResult};
use crate::log;
use crate::ytdlp::{self, Fail, Fetch, Target, limit_note, plural};

/// The embed page lists at most this many tracks.
const EMBED_LIMIT: usize = 100;
const PAGE: usize = 100;
const PATHFINDER: &str = "https://api-partner.spotify.com/pathfinder/v1/query";

/// Persisted query hashes known to work (newest first); refreshed from the
/// web player when all of them are rejected.
const KNOWN_HASHES: &[(&str, &str)] = &[
    (
        "fetchPlaylist",
        "21ea11a6ac75a5f3e01fd365d29c18e0a4a8b7dc09f89bbb5d73910190c6acaf",
    ),
    (
        "fetchPlaylist",
        "19ff1327c29e99c208c86d7a9d8f1929cfdf3d3202a0ff4253c821f1901aa94d",
    ),
    (
        "getAlbum",
        "0b6ce3511e1b02e4a40fa95440f992acbe85080eb308af9194322d708fc1914f",
    ),
    (
        "getTrack",
        "a8ef9e9f02b836feb0da3003c31dbb30decc6f4b473ef89ca88c882386d668de",
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Track,
    Album,
    Playlist,
}

impl Kind {
    fn path(self) -> &'static str {
        match self {
            Kind::Track => "track",
            Kind::Album => "album",
            Kind::Playlist => "playlist",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SpTrack {
    pub id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub duration_ms: u64,
    pub track_number: Option<u32>,
    pub disc_number: Option<u32>,
}

#[derive(Debug, Default)]
pub struct Source {
    pub title: Option<String>,
    pub tracks: Vec<SpTrack>,
    /// Entries that are not Spotify tracks (local files, podcast episodes).
    pub skipped: Vec<(String, String)>,
    /// Tracks in the whole source, when known.
    pub total: Option<usize>,
    pub warnings: Vec<String>,
}

fn is_spotify_id(s: &str) -> bool {
    s.len() == 22 && s.chars().all(|c| c.is_ascii_alphanumeric())
}

/// `(kind, id)` from an `open.spotify.com` URL, including `/intl-xx/` and
/// `/embed/` forms and query strings.
pub fn parse_url(url: &str) -> Result<(Kind, String), String> {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let path = rest.split(['?', '#']).next().unwrap_or("");
    let mut parts = path
        .split('/')
        .skip(1)
        .filter(|p| !p.is_empty() && !p.starts_with("intl-") && *p != "embed");
    let kind = parts.next().unwrap_or("");
    let id = parts.next().unwrap_or("");
    let kind = match kind {
        "track" => Kind::Track,
        "album" => Kind::Album,
        "playlist" => Kind::Playlist,
        "" => {
            return Err(
                "This Spotify link does not point to a track, album or playlist".to_owned(),
            );
        }
        other => {
            return Err(format!(
                "Spotify {other} links are not supported; use a track, album or playlist link"
            ));
        }
    };
    if !is_spotify_id(id) {
        return Err("This Spotify link has no valid id".to_owned());
    }
    Ok((kind, id.to_owned()))
}

fn s(v: &Value, ptr: &str) -> Option<String> {
    v.pointer(ptr)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn u(v: &Value, ptr: &str) -> Option<u64> {
    v.pointer(ptr).and_then(Value::as_u64)
}

fn names(v: &Value, ptr: &str) -> Vec<String> {
    v.pointer(ptr)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| s(x, "/profile/name").or_else(|| s(x, "/name")))
                .collect()
        })
        .unwrap_or_default()
}

fn id_from_uri(uri: &str) -> Option<String> {
    uri.strip_prefix("spotify:track:")
        .filter(|id| is_spotify_id(id))
        .map(str::to_owned)
}

/// The `__NEXT_DATA__` JSON of an embed page.
fn next_data(html: &str) -> Option<Value> {
    let start = html.find("id=\"__NEXT_DATA__\"")?;
    let json_start = start + html[start..].find('>')? + 1;
    let end = json_start + html[json_start..].find("</script>")?;
    serde_json::from_str(&html[json_start..end]).ok()
}

struct Embed {
    name: Option<String>,
    token: Option<String>,
    tracks: Vec<SpTrack>,
    skipped: Vec<(String, String)>,
}

fn split_artists(subtitle: &str) -> Vec<String> {
    subtitle
        .split(',')
        .map(|a| a.replace('\u{a0}', " ").trim().to_owned())
        .filter(|a| !a.is_empty())
        .collect()
}

fn parse_embed(kind: Kind, id: &str, data: &Value) -> Embed {
    let entity = data
        .pointer("/props/pageProps/state/data/entity")
        .cloned()
        .unwrap_or(Value::Null);
    let name = s(&entity, "/name").or_else(|| s(&entity, "/title"));
    let token = s(data, "/props/pageProps/state/settings/session/accessToken");
    let mut tracks = Vec::new();
    let mut skipped = Vec::new();
    if kind == Kind::Track {
        tracks.push(SpTrack {
            id: id.to_owned(),
            title: name.clone().unwrap_or_default(),
            artists: names(&entity, "/artists"),
            duration_ms: u(&entity, "/duration").unwrap_or(0),
            ..SpTrack::default()
        });
    } else {
        let list = entity
            .get("trackList")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for (i, t) in list.iter().enumerate() {
            let title = s(t, "/title").unwrap_or_default();
            let artists = s(t, "/subtitle")
                .map(|a| split_artists(&a))
                .unwrap_or_default();
            match s(t, "/uri").as_deref().and_then(id_from_uri) {
                Some(track_id) => tracks.push(SpTrack {
                    id: track_id,
                    title,
                    album: (kind == Kind::Album).then(|| name.clone()).flatten(),
                    album_artist: (kind == Kind::Album)
                        .then(|| s(&entity, "/subtitle"))
                        .flatten(),
                    artists,
                    duration_ms: u(t, "/duration").unwrap_or(0),
                    track_number: (kind == Kind::Album).then_some(i as u32 + 1),
                    ..SpTrack::default()
                }),
                None => skipped.push((
                    title,
                    "Not a Spotify track (local file or episode)".to_owned(),
                )),
            }
        }
    }
    Embed {
        name,
        token,
        tracks,
        skipped,
    }
}

/// Persisted query hashes, learned ones first, stored in the work directory.
struct Hashes {
    file: PathBuf,
    /// Hashes to try first: saved ones, then freshly discovered ones.
    learned: BTreeMap<String, String>,
    /// What the file holds.
    saved: BTreeMap<String, String>,
    discovered: bool,
}

impl Hashes {
    fn load(work_dir: &Path) -> Self {
        let file = work_dir.join("spotify-queries.json");
        let saved: BTreeMap<String, String> = fs::read(&file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            file,
            learned: saved.clone(),
            saved,
            discovered: false,
        }
    }

    fn candidates(&self, op: &str) -> Vec<String> {
        let mut v: Vec<String> = self.learned.get(op).cloned().into_iter().collect();
        for (o, h) in KNOWN_HASHES {
            if *o == op && !v.iter().any(|x| x == h) {
                v.push((*h).to_owned());
            }
        }
        v
    }

    /// Records a hash that worked, in memory and in the work directory.
    fn remember(&mut self, op: &str, hash: &str) {
        self.learned.insert(op.to_owned(), hash.to_owned());
        if self.saved.get(op).map(String::as_str) != Some(hash) {
            self.saved.insert(op.to_owned(), hash.to_owned());
            if let Ok(json) = serde_json::to_vec_pretty(&self.saved) {
                let _ = tunemesh_core::atomic::write(&self.file, &json);
            }
        }
    }

    /// Reads the current hashes out of the web player's JavaScript.
    fn discover(&mut self, http: &Http) -> Result<BTreeMap<String, String>, String> {
        self.discovered = true;
        let html = http.text("https://open.spotify.com/", &[], 4 << 20)?;
        let mut found = BTreeMap::new();
        let scripts: Vec<&str> = html
            .split('"')
            .filter(|p| {
                p.starts_with("https://open.spotifycdn.com/cdn/build/web-player/")
                    && p.ends_with(".js")
            })
            .collect();
        for script in scripts {
            let Ok(js) = http.text(script, &[], 32 << 20) else {
                continue;
            };
            for op in ["fetchPlaylist", "getAlbum", "getTrack"] {
                let needle = format!("\"{op}\",\"query\",\"");
                if let Some(i) = js.find(&needle) {
                    let hash = &js[i + needle.len()..];
                    if let Some(hash) = hash
                        .get(..64)
                        .filter(|h| h.chars().all(|c| c.is_ascii_hexdigit()))
                    {
                        found.insert(op.to_owned(), hash.to_owned());
                    }
                }
            }
        }
        if found.is_empty() {
            Err("no query hashes found in the web player".to_owned())
        } else {
            Ok(found)
        }
    }
}

/// A variable named by a "missing variable `$name`: … `Boolean!`" error.
fn missing_boolean(errors: &Value) -> Option<String> {
    let msg = errors.pointer("/0/message")?.as_str()?;
    let name = msg.strip_prefix("missing variable `$")?.split('`').next()?;
    (msg.contains("Boolean") && !name.is_empty()).then(|| name.to_owned())
}

struct Pathfinder<'a> {
    http: &'a Http,
    token: String,
    hashes: Hashes,
}

impl Pathfinder<'_> {
    fn call(&self, op: &str, hash: &str, vars: &Value) -> Result<Value, String> {
        let url = format!(
            "{PATHFINDER}?operationName={op}&variables={}&extensions={}",
            ytdlp::percent_encode(&vars.to_string()),
            ytdlp::percent_encode(
                &json!({"persistedQuery": {"version": 1, "sha256Hash": hash}}).to_string()
            )
        );
        let auth = format!("Bearer {}", self.token);
        let text = self.http.text_any_status(
            &url,
            &[("Authorization", &auth), ("app-platform", "WebPlayer")],
            32 << 20,
        )?;
        serde_json::from_str(&text).map_err(|e| format!("Spotify: unexpected answer: {e}"))
    }

    /// Runs `op`, trying hashes and adding required variables as needed.
    fn query(&mut self, op: &str, vars: &Value) -> Result<Value, String> {
        let mut tried: Vec<String> = Vec::new();
        let mut last = "no query hash".to_owned();
        loop {
            let candidates: Vec<String> = self
                .hashes
                .candidates(op)
                .into_iter()
                .filter(|h| !tried.contains(h))
                .collect();
            for hash in candidates {
                tried.push(hash.clone());
                let mut vars = vars.clone();
                for _ in 0..6 {
                    let v = self.call(op, &hash, &vars)?;
                    let Some(errors) = v.get("errors").filter(|e| !e.is_null()) else {
                        self.hashes.remember(op, &hash);
                        return Ok(v);
                    };
                    if let Some(name) = missing_boolean(errors) {
                        vars[name] = Value::Bool(false);
                        continue;
                    }
                    last = s(errors, "/0/message").unwrap_or_else(|| errors.to_string());
                    break;
                }
                if last != "PersistedQueryNotFound" {
                    return Err(format!("Spotify: {last}"));
                }
            }
            if self.hashes.discovered {
                return Err(format!("Spotify: {last}"));
            }
            let found = self.hashes.discover(self.http)?;
            for (o, h) in found {
                if !tried.contains(&h) {
                    self.hashes.learned.insert(o, h);
                }
            }
        }
    }

    fn playlist(&mut self, id: &str, max: usize) -> Result<Source, String> {
        let mut src = Source::default();
        let mut offset = 0;
        loop {
            let vars =
                json!({"uri": format!("spotify:playlist:{id}"), "offset": offset, "limit": PAGE});
            let v = self.query("fetchPlaylist", &vars)?;
            let p = v
                .pointer("/data/playlistV2")
                .ok_or("Spotify: no playlist in the answer")?;
            if p.get("__typename").and_then(Value::as_str) == Some("NotFound") {
                return Err("Spotify: playlist not found".to_owned());
            }
            src.title = s(p, "/name");
            let total = u(p, "/content/totalCount").map(|n| n as usize);
            src.total = total;
            let items = p
                .pointer("/content/items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if items.is_empty() {
                break;
            }
            for item in &items {
                let d = item.pointer("/itemV2/data").cloned().unwrap_or(Value::Null);
                let title = s(&d, "/name").unwrap_or_default();
                match (
                    d.get("__typename").and_then(Value::as_str),
                    s(&d, "/uri").as_deref().and_then(id_from_uri),
                ) {
                    (Some("Track"), Some(track_id)) => {
                        let album_artists = names(&d, "/albumOfTrack/artists/items");
                        src.tracks.push(SpTrack {
                            id: track_id,
                            title,
                            artists: names(&d, "/artists/items"),
                            album: s(&d, "/albumOfTrack/name"),
                            album_artist: album_artists.into_iter().next(),
                            duration_ms: u(&d, "/trackDuration/totalMilliseconds").unwrap_or(0),
                            track_number: u(&d, "/trackNumber").map(|n| n as u32),
                            disc_number: u(&d, "/discNumber").map(|n| n as u32),
                        })
                    }
                    _ => src.skipped.push((
                        title,
                        "Not a Spotify track (local file or episode)".to_owned(),
                    )),
                }
            }
            offset += items.len();
            if src.tracks.len() + src.skipped.len() >= max || total.is_some_and(|t| offset >= t) {
                break;
            }
        }
        Ok(src)
    }

    fn album(&mut self, id: &str, max: usize) -> Result<Source, String> {
        let mut src = Source::default();
        let mut offset = 0;
        loop {
            let vars = json!({"uri": format!("spotify:album:{id}"), "locale": "", "offset": offset, "limit": PAGE});
            let v = self.query("getAlbum", &vars)?;
            let a = v
                .pointer("/data/albumUnion")
                .ok_or("Spotify: no album in the answer")?;
            src.title = s(a, "/name");
            let album_artist = names(a, "/artists/items").into_iter().next();
            let total = u(a, "/tracksV2/totalCount").map(|n| n as usize);
            src.total = total;
            let items = a
                .pointer("/tracksV2/items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if items.is_empty() {
                break;
            }
            for item in &items {
                let t = item.get("track").cloned().unwrap_or(Value::Null);
                let title = s(&t, "/name").unwrap_or_default();
                match s(&t, "/uri").as_deref().and_then(id_from_uri) {
                    Some(track_id) => src.tracks.push(SpTrack {
                        id: track_id,
                        title,
                        artists: names(&t, "/artists/items"),
                        album: src.title.clone(),
                        album_artist: album_artist.clone(),
                        duration_ms: u(&t, "/duration/totalMilliseconds").unwrap_or(0),
                        track_number: u(&t, "/trackNumber").map(|n| n as u32),
                        disc_number: u(&t, "/discNumber").map(|n| n as u32),
                    }),
                    None => src.skipped.push((title, "Not a Spotify track".to_owned())),
                }
            }
            offset += items.len();
            if src.tracks.len() >= max || total.is_some_and(|t| offset >= t) {
                break;
            }
        }
        Ok(src)
    }

    fn track(&mut self, id: &str) -> Result<SpTrack, String> {
        let v = self.query("getTrack", &json!({"uri": format!("spotify:track:{id}")}))?;
        let t = v
            .pointer("/data/trackUnion")
            .ok_or("Spotify: no track in the answer")?;
        let mut artists = names(t, "/firstArtist/items");
        artists.extend(names(t, "/otherArtists/items"));
        Ok(SpTrack {
            id: id.to_owned(),
            title: s(t, "/name").unwrap_or_default(),
            album: s(t, "/albumOfTrack/name"),
            album_artist: names(t, "/albumOfTrack/artists/items")
                .into_iter()
                .next()
                .or_else(|| artists.first().cloned()),
            artists,
            duration_ms: u(t, "/duration/totalMilliseconds").unwrap_or(0),
            track_number: u(t, "/trackNumber").map(|n| n as u32),
            disc_number: u(t, "/discNumber").map(|n| n as u32),
        })
    }
}

/// Fetches the tracks of a Spotify link, at most `max` of them.
pub fn fetch_source(
    http: &Http,
    work_dir: &Path,
    kind: Kind,
    id: &str,
    max: usize,
) -> Result<Source, String> {
    let html = http.text(
        &format!("https://open.spotify.com/embed/{}/{id}", kind.path()),
        &[],
        8 << 20,
    )?;
    let data = next_data(&html).ok_or("Spotify: the embed page has an unexpected format")?;
    let embed = parse_embed(kind, id, &data);
    if embed.tracks.is_empty() && embed.skipped.is_empty() {
        return Err(format!(
            "Spotify: this {} has no tracks or is not public",
            kind.path()
        ));
    }
    let full = match &embed.token {
        Some(token) => {
            let mut pf = Pathfinder {
                http,
                token: token.clone(),
                hashes: Hashes::load(work_dir),
            };
            match kind {
                Kind::Playlist => pf.playlist(id, max),
                Kind::Album => pf.album(id, max),
                Kind::Track => pf.track(id).map(|t| Source {
                    title: Some(t.title.clone()),
                    tracks: vec![t],
                    total: Some(1),
                    ..Source::default()
                }),
            }
        }
        None => Err("Spotify: no anonymous token on the embed page".to_owned()),
    };
    match full {
        Ok(mut src) if !src.tracks.is_empty() => {
            src.title = src.title.or(embed.name);
            Ok(src)
        }
        result => {
            let why = result
                .err()
                .unwrap_or_else(|| "the full track list was empty".to_owned());
            log!("spotify: full track list unavailable ({why}); using the embed page");
            let n = embed.tracks.len() + embed.skipped.len();
            let mut warnings = Vec::new();
            if kind != Kind::Track && n >= EMBED_LIMIT {
                warnings.push(format!(
                    "Only the first {EMBED_LIMIT} tracks could be read: Spotify's public page lists at most \
                     {EMBED_LIMIT} and the full list could not be fetched ({why})"
                ));
            }
            Ok(Source {
                title: embed.name,
                tracks: embed.tracks,
                skipped: embed.skipped,
                total: None,
                warnings,
            })
        }
    }
}

/// Spotify titles carry remaster notes that YouTube Music titles lack.
fn search_title(title: &str) -> &str {
    match title.split_once(" - ") {
        Some((head, tail))
            if ["remaster", "mono", "stereo", "single version", "bonus"]
                .iter()
                .any(|w| tail.to_lowercase().contains(w)) =>
        {
            head
        }
        _ => title,
    }
}

fn norm(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// How well a candidate title fits: exact > contained > other, minus
/// version words the Spotify title does not have.
fn title_score(spotify: &str, candidate: &str) -> i32 {
    let (s, c) = (norm(search_title(spotify)), norm(candidate));
    let mut score = if s == c {
        3
    } else if c.contains(&s) || s.contains(&c) {
        2
    } else {
        0
    };
    for word in [
        "live",
        "remix",
        "cover",
        "karaoke",
        "instrumental",
        "acoustic",
        "sped up",
        "slowed",
    ] {
        if c.contains(word) && !norm(spotify).contains(word) {
            score -= 2;
        }
    }
    score
}

/// Accepted duration range for a match: ±max(10 s, 10 %).
fn window(duration_ms: u64) -> Option<(u64, u64)> {
    (duration_ms > 0).then(|| {
        let secs = duration_ms / 1000;
        let slack = (secs / 10).max(10);
        (secs.saturating_sub(slack), secs + slack)
    })
}

/// `%` doubled so text is literal in a yt-dlp output template.
fn literal(s: &str) -> String {
    s.replace('%', "%%")
}

fn output_template(t: &SpTrack) -> String {
    let artist = t
        .album_artist
        .as_deref()
        .or(t.artists.first().map(String::as_str))
        .unwrap_or("Unknown Artist");
    let album = match &t.album {
        Some(a) => literal(&tunemesh_core::sanitize::component(a)),
        None => "%(album|Singles).80B".to_owned(),
    };
    format!(
        "{}/{album}/{} [{}].%(ext)s",
        literal(&tunemesh_core::sanitize::component(artist)),
        literal(&tunemesh_core::sanitize::component(&t.title)),
        t.id
    )
}

/// Writes Spotify's metadata over what YouTube supplied. A lofty panic
/// counts as an error (see `library::guarded`).
fn retag(path: &Path, t: &SpTrack) -> Result<(), String> {
    tunemesh_core::library::guarded(&path.display().to_string(), || retag_unguarded(path, t))
        .unwrap_or_else(|| Err("the tag writer crashed on this file".to_owned()))
}

fn retag_unguarded(path: &Path, t: &SpTrack) -> Result<(), String> {
    let mut file = lofty::read_from_path(path).map_err(|e| e.to_string())?;
    if file.primary_tag_mut().is_none() {
        let tag_type = file.primary_tag_type();
        file.insert_tag(Tag::new(tag_type));
    }
    let tag = file.primary_tag_mut().ok_or("no tag")?;
    tag.set_title(t.title.clone());
    if !t.artists.is_empty() {
        tag.set_artist(t.artists.join(", "));
    }
    if let Some(album) = &t.album {
        tag.set_album(album.clone());
    }
    if let Some(aa) = t.album_artist.as_ref().or(t.artists.first()) {
        tag.insert_text(ItemKey::AlbumArtist, aa.clone());
    }
    if let Some(n) = t.track_number {
        tag.set_track(n);
    }
    if let Some(n) = t.disc_number {
        tag.set_disk(n);
    }
    file.save_to_path(path, WriteOptions::default())
        .map_err(|e| e.to_string())
}

/// Finds the track on YouTube Music (then YouTube) and downloads it.
fn find_and_download(
    job: &mut Job,
    t: &SpTrack,
    index: usize,
    total: usize,
) -> Result<String, Fail> {
    job.check_space().map_err(Fail::Error)?;
    let artists = t
        .artists
        .iter()
        .take(2)
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    let query = format!("{} {artists}", search_title(&t.title));
    let output = output_template(t);
    let range = window(t.duration_ms);
    let mut last_error = None;

    let mut candidates: Vec<ytdlp::Item> = Vec::new();
    match ytdlp::list(job, &ytdlp::music_search_url(&query), Some("1:5"), None) {
        Ok(l) => {
            let mut items: Vec<(i32, usize, ytdlp::Item)> = l
                .items
                .into_iter()
                .enumerate()
                .map(|(i, it)| {
                    (
                        title_score(&t.title, it.title.as_deref().unwrap_or("")),
                        i,
                        it,
                    )
                })
                .collect();
            items.sort_by_key(|(score, i, _)| (-score, *i));
            candidates.extend(items.into_iter().take(3).map(|(_, _, it)| it));
        }
        Err(Fail::Stopped(s)) => return Err(Fail::Stopped(s)),
        Err(Fail::Error(e)) => last_error = Some(e),
    }
    let mut tried_fallback = false;
    let mut i = 0;
    loop {
        if i == candidates.len() {
            if tried_fallback {
                break;
            }
            tried_fallback = true;
            // Plain YouTube results come with durations: closest first.
            match ytdlp::list(job, &format!("ytsearch5:{query}"), None, None) {
                Ok(l) => {
                    let target = (t.duration_ms / 1000) as f64;
                    let mut items: Vec<ytdlp::Item> = l
                        .items
                        .into_iter()
                        .filter(|it| {
                            range.is_none_or(|(lo, hi)| {
                                it.duration.is_none_or(|d| d >= lo as f64 && d <= hi as f64)
                            })
                        })
                        .collect();
                    items.sort_by(|a, b| {
                        let da = a.duration.map_or(f64::MAX, |d| (d - target).abs());
                        let db = b.duration.map_or(f64::MAX, |d| (d - target).abs());
                        da.total_cmp(&db)
                    });
                    candidates.extend(items.into_iter().take(2));
                }
                Err(Fail::Stopped(s)) => return Err(Fail::Stopped(s)),
                Err(Fail::Error(e)) => last_error = Some(e),
            }
            continue;
        }
        let url = candidates[i].url.clone();
        i += 1;
        if let Err(e) = crate::netguard::check_url(&url) {
            last_error = Some(e);
            continue;
        }
        match ytdlp::fetch(
            job,
            &Target {
                url: &url,
                output: &output,
                window: range,
            },
            index,
            total,
        ) {
            Fetch::File(path) => {
                if let Err(e) = retag(&path, t) {
                    log!(
                        "[{}] could not write Spotify tags to {}: {e}",
                        job.req.id,
                        path.display()
                    );
                }
                return job.place(&path).map_err(|e| {
                    Fail::Error(format!("Could not move the download into the library: {e}"))
                });
            }
            Fetch::Skipped(why) | Fetch::Failed(why) => {
                log!("[{}] {url}: {why}", job.req.id);
                last_error = Some(why);
            }
            Fetch::Stopped(s) => return Err(Fail::Stopped(s)),
        }
    }
    Err(Fail::Error(match last_error {
        Some(e) => format!("No matching audio found ({e})"),
        None => "No matching audio found on YouTube Music or YouTube".to_owned(),
    }))
}

fn label(t: &SpTrack) -> String {
    if t.artists.is_empty() {
        t.title.clone()
    } else {
        format!("{} - {}", t.artists.join(", "), t.title)
    }
}

pub fn run(job: &mut Job, url: &str) -> SourceResult {
    let (kind, id) = match parse_url(url) {
        Ok(x) => x,
        Err(e) => return SourceResult::fatal(e),
    };
    let max = job.ctx.limits.max_items;
    let ctx = job.ctx;
    let mut src = match fetch_source(&ctx.http, &ctx.work_dir, kind, &id, max) {
        Ok(s) => s,
        Err(e) => return SourceResult::fatal(e),
    };
    if let Some(s) = job.stop_reason() {
        return SourceResult::stopped(s);
    }
    job.set_title(src.title.clone());
    let mut result = SourceResult::default();
    if kind != Kind::Track {
        result.source_playlist = src.title.clone();
    }
    for w in std::mem::take(&mut src.warnings) {
        job.warn(w);
    }
    let listed = src.tracks.len() + src.skipped.len();
    if src.tracks.len() > max || src.total.is_some_and(|t| t > max) {
        src.tracks.truncate(max);
        let of = src.total.map(|t| format!(" of {t}")).unwrap_or_default();
        job.warn(format!(
            "Imported the first {}{of}: {}",
            plural(max, "track", "tracks"),
            limit_note(max)
        ));
    } else if let Some(t) = src.total.filter(|t| *t > listed) {
        job.warn(format!("Spotify listed {listed} of {t} tracks"));
    }
    let total = src.tracks.len() + src.skipped.len();
    result.total = total;
    job.set_total(total);
    let mut done = 0;
    for (title, why) in std::mem::take(&mut src.skipped) {
        job.fail_item(title, why);
        job.item_done(done, total);
        done += 1;
    }
    for t in &src.tracks {
        if let Some(s) = job.stop_reason() {
            result.stopped = Some(s);
            return result;
        }
        if let Some(existing) = job.existing(&t.id) {
            log!("[{}] already in library: {existing}", job.req.id);
            job.add_track(existing);
        } else {
            match find_and_download(job, t, done, total) {
                Ok(rel) => {
                    log!("[{}] downloaded {rel}", job.req.id);
                    job.add_track(rel);
                }
                Err(Fail::Error(e)) => job.fail_item(label(t), e),
                Err(Fail::Stopped(Stop::Limit(e))) => job.fail_item(label(t), e),
                Err(Fail::Stopped(s)) => {
                    result.stopped = Some(s);
                    return result;
                }
            }
        }
        job.item_done(done, total);
        done += 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_urls() {
        let id = "37i9dQZF1DWXRqgorJj26U";
        assert_eq!(
            parse_url(&format!("https://open.spotify.com/playlist/{id}?si=abc")).unwrap(),
            (Kind::Playlist, id.into())
        );
        assert_eq!(
            parse_url(&format!("https://open.spotify.com/intl-de/album/{id}")).unwrap(),
            (Kind::Album, id.into())
        );
        assert_eq!(
            parse_url(&format!("https://open.spotify.com/embed/track/{id}")).unwrap(),
            (Kind::Track, id.into())
        );
        assert!(
            parse_url(&format!("https://open.spotify.com/artist/{id}"))
                .unwrap_err()
                .contains("artist")
        );
        assert!(parse_url("https://open.spotify.com/playlist/x").is_err());
        assert!(parse_url("https://open.spotify.com/").is_err());
    }

    #[test]
    fn reads_embed_pages() {
        let html = r#"<html><script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"state":{
            "data":{"entity":{"type":"playlist","name":"Mix","trackList":[
              {"uri":"spotify:track:0ofHAoxe9vBkTCp2UQIavz","title":"Dreams - 2004 Remaster","subtitle":"Fleetwood Mac","duration":257800},
              {"uri":"spotify:local:x","title":"Local","subtitle":"Me","duration":1000},
              {"uri":"spotify:track:4KfSdst7rW39C0sfhArdrz","title":"Duet","subtitle":"A,\u00a0B","duration":1000}]}},
            "settings":{"session":{"accessToken":"tok"}}}}}}</script></html>"#;
        let e = parse_embed(Kind::Playlist, "x", &next_data(html).unwrap());
        assert_eq!(e.name.as_deref(), Some("Mix"));
        assert_eq!(e.token.as_deref(), Some("tok"));
        assert_eq!(e.tracks.len(), 2);
        assert_eq!(e.skipped.len(), 1);
        assert_eq!(e.tracks[1].artists, vec!["A", "B"]);
        assert_eq!(e.tracks[0].duration_ms, 257800);
    }

    #[test]
    fn matching_helpers() {
        assert_eq!(search_title("Dreams - 2004 Remaster"), "Dreams");
        assert_eq!(search_title("Never - Say Never"), "Never - Say Never");
        assert!(
            title_score("Barracuda", "Barracuda") > title_score("Barracuda", "Barracuda (Live)")
        );
        assert!(title_score("Barracuda (Live)", "Barracuda (Live)") >= 2);
        assert_eq!(window(257_800), Some((232, 282)));
        assert_eq!(window(60_000), Some((50, 70)));
        assert_eq!(window(0), None);
        let mut v = json!({"errors":[{"message":"missing variable `$enableWatchFeedEntrypoint`: for required GraphQL type `Boolean!`"}]});
        assert_eq!(
            missing_boolean(&v["errors"]).as_deref(),
            Some("enableWatchFeedEntrypoint")
        );
        v = json!([{"message":"PersistedQueryNotFound"}]);
        assert_eq!(missing_boolean(&v), None);
    }

    #[test]
    fn working_hashes_are_saved_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Hashes::load(dir.path());
        assert_eq!(h.candidates("getTrack").len(), 1);
        h.learned.insert("getTrack".into(), "a".repeat(64));
        assert_eq!(h.candidates("getTrack")[0], "a".repeat(64));
        h.remember("getTrack", &"a".repeat(64));
        let again = Hashes::load(dir.path());
        assert_eq!(again.candidates("getTrack")[0], "a".repeat(64));
        assert_eq!(again.candidates("getTrack").len(), 2);
    }

    #[test]
    fn output_templates_are_literal() {
        let t = SpTrack {
            id: "0ofHAoxe9vBkTCp2UQIavz".into(),
            title: "100% / Pure: %(id)s".into(),
            artists: vec!["AC/DC".into()],
            album: Some("Album.".into()),
            ..SpTrack::default()
        };
        assert_eq!(
            output_template(&t),
            "AC_DC/Album_/100%% _ Pure_ %%(id)s [0ofHAoxe9vBkTCp2UQIavz].%(ext)s"
        );
        let t = SpTrack { album: None, ..t };
        assert!(output_template(&t).contains("/%(album|Singles).80B/"));
    }
}
