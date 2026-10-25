//! Downloads through yt-dlp: URLs other than Spotify, searches, and the
//! audio for Spotify tracks.
//!
//! yt-dlp only reaches the network through the worker's egress proxy
//! (`--proxy`, plus the proxy variables for the ffmpeg it may start), and
//! only formats served over plain HTTP(S), HLS or DASH fetched by yt-dlp's
//! own downloader are accepted, so every request it makes passes the
//! proxy's address checks. Its ffmpeg and ffprobe runs go through
//! `ffshim`, which confines them to plain media files in the job directory.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

use crate::child::{self, End, Stop, Stream};
use crate::job::{Job, SourceResult};
use crate::{log, netguard};

/// `<Artist>/<Album or Singles>/<Title> [<id>].<ext>` from YouTube Music
/// fields with uploader/title fallbacks. Fields are capped in bytes so that
/// even after `--windows-filenames` turns ASCII characters into 3-byte
/// full-width ones a name stays below 255 bytes and the `[<id>]` suffix
/// survives whole; the worker shortens names to the contract's 120 bytes
/// (keeping the suffix) when it moves the file into the library.
const OUTPUT_TEMPLATE: &str = "%(artists.0,artist,creator,uploader,channel).80B/%(album|Singles).80B/%(track,title).70B [%(id)s].%(ext)s";

/// Crops the thumbnail to a centred square while converting it to JPEG.
const SQUARE_THUMBNAIL: &str = "ThumbnailsConvertor+ffmpeg_o:-c:v mjpeg -qmin 1 -q:v 2 -vf crop=\"'if(gt(ih,iw),iw,ih)':'if(gt(iw,ih),ih,iw)'\"";

/// Same fallbacks as the `<Artist>` directory in `OUTPUT_TEMPLATE`.
const PRIMARY_ARTIST_AS_ALBUM_ARTIST: &str =
    "%(artists.0,artist,creator,uploader,channel)s:%(meta_album_artist)s";

const PROGRESS_TEMPLATE: &str = "download:TMPROG %(progress.downloaded_bytes)s %(progress.total_bytes)s %(progress.total_bytes_estimate)s";

/// Audio first; only protocols yt-dlp downloads itself (no ffmpeg, RTMP or
/// other network input that could bypass the proxy).
const FORMAT: &str =
    "ba[protocol^=http]/ba[protocol=m3u8_native]/b[protocol^=http]/b[protocol=m3u8_native]";

/// The base yt-dlp command for a job.
pub fn command(job: &Job) -> Command {
    let t = &job.tools;
    let proxy = job.ctx.proxy.url();
    let work = &job.ctx.work_dir;
    let run = &job.ctx.run;
    let mut c = Command::new(&t.yt_dlp);
    // The proxy (with its per-run password) reaches yt-dlp through the
    // environment only, which other users cannot read, unlike argv.
    c.args([
        "--ignore-config",
        "--no-playlist",
        "--no-colors",
        "--cache-dir",
    ])
    .arg(work.join("cache/yt-dlp"));
    if let Some(deno) = &t.deno {
        c.arg("--js-runtimes")
            .arg(format!("deno:{}", deno.display()));
    }
    // ffmpeg and ffprobe only through the confining shim; without it the
    // postprocessors fail rather than run unconfined.
    match crate::ffshim::install(run.path()) {
        Ok(dir) => {
            c.arg("--ffmpeg-location").arg(dir);
        }
        Err(e) => {
            log!("[{}] could not set up the ffmpeg shim: {e}", job.req.id);
            c.arg("--ffmpeg-location")
                .arg(run.path().join("ffmpeg-shim"));
        }
    }
    c.envs(crate::ffshim::env(&t.ffmpeg, &t.ffprobe, &job.dir));
    for var in [
        "http_proxy",
        "https_proxy",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "all_proxy",
        "ALL_PROXY",
    ] {
        c.env(var, &proxy);
    }
    let tmp = run.tmp();
    let _ = std::fs::create_dir_all(&tmp);
    c.env_remove("no_proxy")
        .env_remove("NO_PROXY")
        .env("TMPDIR", tmp);
    c
}

pub struct Item {
    pub id: Option<String>,
    pub url: String,
    pub title: Option<String>,
    pub duration: Option<f64>,
}

pub struct Listing {
    pub title: Option<String>,
    pub is_playlist: bool,
    pub items: Vec<Item>,
    /// Items in the whole playlist, when the site says.
    pub count: Option<usize>,
}

pub enum Fail {
    Stopped(Stop),
    Error(String),
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

pub fn parse_listing(v: &Value, input: &str) -> Listing {
    let entries = v.get("entries").and_then(Value::as_array);
    match entries {
        Some(entries) => {
            let items = entries
                .iter()
                .filter(|e| !e.is_null())
                .filter_map(|e| {
                    let id = str_field(e, "id");
                    let url = str_field(e, "url")
                        .or_else(|| str_field(e, "webpage_url"))
                        .or_else(|| {
                            let ie = str_field(e, "ie_key").unwrap_or_default();
                            id.as_ref()
                                .filter(|_| ie.starts_with("Youtube"))
                                .map(|id| format!("https://www.youtube.com/watch?v={id}"))
                        })?;
                    Some(Item {
                        id,
                        url,
                        title: str_field(e, "title"),
                        duration: e.get("duration").and_then(Value::as_f64),
                    })
                })
                .collect();
            let count = v
                .get("playlist_count")
                .and_then(Value::as_u64)
                .map(|n| n as usize);
            Listing {
                title: str_field(v, "title"),
                is_playlist: true,
                items,
                count,
            }
        }
        None => {
            let url = str_field(v, "webpage_url")
                .or_else(|| str_field(v, "original_url"))
                .unwrap_or_else(|| input.to_owned());
            let title = str_field(v, "track").or_else(|| str_field(v, "title"));
            let item = Item {
                id: str_field(v, "id"),
                url,
                title: title.clone(),
                duration: v.get("duration").and_then(Value::as_f64),
            };
            Listing {
                title,
                is_playlist: false,
                items: vec![item],
                count: Some(1),
            }
        }
    }
}

/// An error message for a failed run, preferring a proxy refusal.
fn failure_text(job: &Job, outcome: &child::Outcome) -> String {
    match job.ctx.proxy.take_refusals().pop() {
        Some(refusal) => format!("Blocked a connection: {refusal}"),
        None => outcome.error_message(),
    }
}

/// Lists what `target` refers to without downloading: at most `end` entries
/// (`--playlist-end`), or the entries in `items` (`--playlist-items`).
pub fn list(
    job: &Job,
    target: &str,
    items: Option<&str>,
    end: Option<usize>,
) -> Result<Listing, Fail> {
    let mut cmd = command(job);
    cmd.args(["--flat-playlist", "--dump-single-json", "--no-warnings"]);
    if let Some(items) = items {
        cmd.args(["--playlist-items", items]);
    }
    if let Some(end) = end {
        cmd.arg("--playlist-end").arg(end.to_string());
    }
    cmd.arg("--").arg(target);
    job.ctx.proxy.take_refusals();
    let mut json = String::new();
    let id = job.req.id.clone();
    let outcome = child::run(
        &mut cmd,
        job.ctx.limits.child_timeout,
        |stream, line| match stream {
            Stream::Out => json.push_str(line),
            Stream::Err => log!("[{id}] yt-dlp: {line}"),
        },
        || job.stop_reason(),
    )
    .map_err(|e| Fail::Error(format!("Could not run yt-dlp: {e}")))?;
    match outcome.end {
        End::Stopped(Stop::Limit(why)) => Err(Fail::Error(why)),
        End::Stopped(s) => Err(Fail::Stopped(s)),
        End::Exited(s) if s.success() => {
            let v: Value = serde_json::from_str(&json)
                .map_err(|e| Fail::Error(format!("Unexpected yt-dlp output: {e}")))?;
            Ok(parse_listing(&v, target))
        }
        End::Exited(_) => Err(Fail::Error(failure_text(job, &outcome))),
    }
}

/// The fraction (0 to 1) a `PROGRESS_TEMPLATE` line reports.
pub fn parse_progress(line: &str) -> Option<f64> {
    let mut it = line.strip_prefix("TMPROG ")?.split(' ');
    let done = it.next()?.parse::<f64>().ok().filter(|d| *d >= 0.0)?;
    let total = it.next().and_then(|t| t.parse::<f64>().ok());
    let estimate = it.next().and_then(|t| t.parse::<f64>().ok());
    let total = total.or(estimate).filter(|t| *t > 0.0)?;
    let fraction = done / total; // NaN for inf / inf
    (!fraction.is_nan()).then(|| fraction.clamp(0.0, 1.0))
}

pub enum Fetch {
    File(PathBuf),
    /// yt-dlp skipped the item (match filter or size limit); the text says why.
    Skipped(String),
    Failed(String),
    Stopped(Stop),
}

/// What to download and how to name it.
pub struct Target<'a> {
    pub url: &'a str,
    /// yt-dlp output template, relative to the job directory.
    pub output: &'a str,
    /// Accepted duration range in seconds (Spotify matches).
    pub window: Option<(u64, u64)>,
}

/// Downloads one item into the job's scratch directory. `index`/`total`
/// place its progress within the request.
pub fn fetch(job: &mut Job, target: &Target, index: usize, total: usize) -> Fetch {
    let ctx = job.ctx;
    let limits = &ctx.limits;
    let max_secs = limits.max_duration.as_secs();
    let mut filter = format!("!is_live & duration <=? {max_secs}");
    if let Some((lo, hi)) = target.window {
        filter.push_str(&format!(" & duration >=? {lo} & duration <=? {hi}"));
    }
    let mut cmd = command(job);
    cmd.args([
        "--newline",
        "--progress",
        "--progress-template",
        PROGRESS_TEMPLATE,
        "--format",
        FORMAT,
        "--max-filesize",
        &limits.max_file_size.to_string(),
        "--match-filter",
        &filter,
        "--extract-audio",
        "--embed-metadata",
        // YouTube Music joins all artists into one tag ("A, B"); the album
        // artist names the primary one so albums group under it.
        "--parse-metadata",
        PRIMARY_ARTIST_AS_ALBUM_ARTIST,
        "--embed-thumbnail",
        "--convert-thumbnails",
        "jpg",
        "--postprocessor-args",
        SQUARE_THUMBNAIL,
        "--windows-filenames",
        "--no-mtime",
        "--no-part",
        // Printed before the match filter runs, so a skip can be explained.
        "--print",
        "pre_process:TMINFO %(duration)s %(is_live)s",
        "--print",
        "after_move:TMFILE %(filepath)s",
        "--no-simulate",
        "--paths",
    ])
    .arg(&job.dir)
    .args(["--output", target.output, "--"])
    .arg(target.url);

    job.ctx.proxy.take_refusals();
    let mut produced: Option<PathBuf> = None;
    let mut info: Option<(Option<f64>, bool)> = None;
    let id = job.req.id.clone();
    let timeout = limits.child_timeout;
    let outcome = {
        let job_ref = &mut *job;
        let progress_cell = std::cell::RefCell::new(None::<f64>);
        child::run(
            &mut cmd,
            timeout,
            |stream, line| match stream {
                Stream::Out => {
                    if let Some(frac) = parse_progress(line) {
                        *progress_cell.borrow_mut() = Some(frac);
                    } else if let Some(path) = line.strip_prefix("TMFILE ") {
                        produced = Some(PathBuf::from(path));
                    } else if let Some(rest) = line.strip_prefix("TMINFO ") {
                        let mut parts = rest.split(' ');
                        let duration = parts.next().and_then(|d| d.parse().ok());
                        info = Some((duration, parts.next() == Some("True")));
                    } else {
                        log!("[{id}] yt-dlp: {line}");
                    }
                }
                Stream::Err => log!("[{id}] yt-dlp: {line}"),
            },
            || {
                if let Some(frac) = progress_cell.borrow_mut().take() {
                    // Leave headroom for post-processing.
                    job_ref.status.progress =
                        Some(((index as f64 + frac * 0.9) / total.max(1) as f64).clamp(0.0, 1.0));
                    job_ref.flush(false);
                }
                job_ref.child_check()
            },
        )
    };
    let outcome = match outcome {
        Ok(o) => o,
        Err(e) => return Fetch::Failed(format!("Could not run yt-dlp: {e}")),
    };
    match outcome.end {
        End::Stopped(Stop::Limit(why)) => Fetch::Failed(why),
        End::Stopped(s) => Fetch::Stopped(s),
        End::Exited(s) if s.success() => match produced {
            Some(path) if path.is_file() => Fetch::File(path),
            _ => Fetch::Skipped(skip_reason(job, info, target.window)),
        },
        End::Exited(_) => Fetch::Failed(failure_text(job, &outcome)),
    }
}

fn skip_reason(job: &Job, info: Option<(Option<f64>, bool)>, window: Option<(u64, u64)>) -> String {
    let max = job.ctx.limits.max_duration;
    match info {
        Some((_, true)) => "Live streams are not downloaded".to_owned(),
        Some((Some(d), _)) if d > max.as_secs() as f64 => {
            format!("Longer than the {} limit", humantime::format_duration(max))
        }
        Some((Some(d), _)) if window.is_some_and(|(lo, hi)| d < lo as f64 || d > hi as f64) => {
            format!("Duration {} s does not match", d.round())
        }
        Some(_) => format!(
            "Larger than the {} MiB file size limit",
            job.ctx.limits.max_file_size / (1024 * 1024)
        ),
        None => "yt-dlp did not produce a file".to_owned(),
    }
}

/// Downloads an item and moves it into the library.
fn download_one(job: &mut Job, item: &Item, index: usize, total: usize) -> Result<String, Fail> {
    // Playlist entries can point anywhere; checked here for a clear message,
    // enforced by the proxy.
    netguard::check_url(&item.url).map_err(Fail::Error)?;
    job.check_space().map_err(Fail::Error)?;
    let target = Target {
        url: &item.url,
        output: OUTPUT_TEMPLATE,
        window: None,
    };
    match fetch(job, &target, index, total) {
        Fetch::File(path) => job
            .place(&path)
            .map_err(|e| Fail::Error(format!("Could not move the download into the library: {e}"))),
        Fetch::Skipped(why) | Fetch::Failed(why) => Err(Fail::Error(why)),
        Fetch::Stopped(s) => Err(Fail::Stopped(s)),
    }
}

/// Downloads every item in order, deduplicating by source id, and keeps
/// going past failures.
fn download_all(job: &mut Job, items: &[Item], result: &mut SourceResult) {
    let total = items.len();
    result.total = total;
    job.set_total(total);
    for (i, item) in items.iter().enumerate() {
        if let Some(s) = job.stop_reason() {
            result.stopped = Some(s);
            return;
        }
        if let Some(existing) = item.id.as_deref().and_then(|id| job.existing(id)) {
            log!("[{}] already in library: {existing}", job.req.id);
            job.add_track(existing);
        } else {
            match download_one(job, item, i, total) {
                Ok(rel) => {
                    log!("[{}] downloaded {rel}", job.req.id);
                    job.add_track(rel);
                }
                Err(Fail::Error(e)) => {
                    let name = item.title.clone().unwrap_or_else(|| item.url.clone());
                    job.fail_item(name, e);
                }
                Err(Fail::Stopped(s)) => {
                    result.stopped = Some(s);
                    return;
                }
            }
        }
        job.item_done(i, total);
    }
}

/// What a YouTube or YouTube Music link's `list=` parameter makes it
/// (contract: "YouTube links").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum YouTubeList {
    /// No list, an empty one, or an auto-generated mix (`RD...`): the one
    /// video.
    Single,
    /// An album (`OLAK5uy_...`) or playlist: the whole list, as
    /// `https://<host>/playlist?list=<id>`.
    List(String),
}

/// Applies the contract's rule to `url`; `None` if it is not a YouTube or
/// YouTube Music link.
pub fn youtube_list(url: &str) -> Option<YouTubeList> {
    let (scheme, rest) = url.split_once("://")?;
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return None;
    }
    let rest = rest.split('#').next().unwrap_or(rest);
    let authority = rest.split(['/', '?']).next()?;
    let host = authority
        .rsplit('@')
        .next()?
        .split(':')
        .next()?
        .to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let host = host.strip_prefix("m.").unwrap_or(host);
    let playlist_host = match host {
        "youtube.com" | "youtu.be" => "www.youtube.com",
        "music.youtube.com" => "music.youtube.com",
        _ => return None,
    };
    let list = rest
        .split_once('?')
        .and_then(|(_, q)| q.split('&').find_map(|p| p.strip_prefix("list=")))
        .unwrap_or("");
    Some(
        if list.is_empty() || (list.starts_with("RD") && !list.starts_with("RDCLAK5uy_")) {
            YouTubeList::Single
        } else {
            YouTubeList::List(format!("https://{playlist_host}/playlist?list={list}"))
        },
    )
}

pub fn run_url(job: &mut Job, url: &str) -> SourceResult {
    if let Err(e) = netguard::check_url(url) {
        return SourceResult::fatal(e);
    }
    // A video link inside a playlist or album downloads the whole list.
    let playlist_url;
    let url = match youtube_list(url) {
        Some(YouTubeList::List(u)) => {
            playlist_url = u;
            playlist_url.as_str()
        }
        _ => url,
    };
    let max = job.ctx.limits.max_items;
    // One more than allowed tells whether the source was cut.
    let mut listing = match list(job, url, None, Some(max + 1)) {
        Ok(l) => l,
        Err(Fail::Stopped(s)) => return SourceResult::stopped(s),
        Err(Fail::Error(e)) => return SourceResult::fatal(e),
    };
    job.set_title(listing.title.clone());
    let mut result = SourceResult::default();
    if listing.is_playlist {
        result.source_playlist = listing.title.clone();
        if listing.items.len() > max {
            listing.items.truncate(max);
            let of = listing
                .count
                .filter(|c| *c > max)
                .map(|c| format!(" of {c}"))
                .unwrap_or_default();
            job.warn(format!(
                "Imported the first {}{of}: {}",
                plural(max, "item", "items"),
                limit_note(max)
            ));
        }
    }
    download_all(job, &listing.items, &mut result);
    result
}

/// `1 item`, `2 items`.
pub fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Why a source was cut to `max` items.
pub fn limit_note(max: usize) -> String {
    format!(
        "a request downloads at most {} (--max-items)",
        plural(max, "item", "items")
    )
}

pub fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The YouTube Music "songs" search for `text`.
pub fn music_search_url(text: &str) -> String {
    format!(
        "https://music.youtube.com/search?q={}#songs",
        percent_encode(text)
    )
}

/// Search: the first YouTube Music song result, else the first YouTube result.
pub fn run_search(job: &mut Job, text: &str) -> SourceResult {
    let mut result = SourceResult::default();
    let targets = [music_search_url(text), format!("ytsearch1:{text}")];
    let mut last_error = None;
    for target in &targets {
        match list(job, target, Some("1"), None) {
            Err(Fail::Stopped(s)) => return SourceResult::stopped(s),
            Ok(listing) => {
                if let Some(item) = listing.items.into_iter().next() {
                    log!("[{}] search {text:?} resolved to {}", job.req.id, item.url);
                    job.set_title(item.title.clone());
                    download_all(job, &[item], &mut result);
                    return result;
                }
            }
            Err(Fail::Error(e)) => last_error = Some(e),
        }
    }
    result.fatal = Some(last_error.unwrap_or_else(|| format!("No results for “{text}”")));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_lines() {
        assert_eq!(parse_progress("TMPROG 50 100 NA"), Some(0.5));
        assert_eq!(parse_progress("TMPROG 50 NA 200"), Some(0.25));
        assert_eq!(parse_progress("TMPROG 50 NA NA"), None);
        assert_eq!(parse_progress("other"), None);
        // Found by `fuzz/fuzz_targets/remote_metadata.rs`: a negative or
        // NaN count gave a negative (or NaN) status `progress`.
        assert_eq!(parse_progress("TMPROG -7 8 NA"), None);
        assert_eq!(parse_progress("TMPROG NaN 8 NA"), None);
        assert_eq!(parse_progress("TMPROG inf 8 NA"), Some(1.0));
        assert_eq!(parse_progress("TMPROG 1 inf NA"), Some(0.0));
        assert_eq!(parse_progress("TMPROG inf inf NA"), None);
    }

    #[test]
    fn listing_shapes() {
        let single: Value = serde_json::json!({"_type":"video","id":"a","title":"T","webpage_url":"https://y/a","duration":5.0});
        let l = parse_listing(&single, "in");
        assert!(!l.is_playlist);
        assert_eq!(l.items[0].url, "https://y/a");
        assert_eq!(l.items[0].duration, Some(5.0));
        let pl: Value = serde_json::json!({"_type":"playlist","title":"P","playlist_count":9,"entries":[
            {"id":"a","url":"https://y/a","title":"A"}, null, {"id":"b","ie_key":"Youtube"}]});
        let l = parse_listing(&pl, "in");
        assert!(l.is_playlist);
        assert_eq!(l.title.as_deref(), Some("P"));
        assert_eq!(l.count, Some(9));
        assert_eq!(
            l.items.iter().map(|i| i.url.as_str()).collect::<Vec<_>>(),
            vec!["https://y/a", "https://www.youtube.com/watch?v=b"]
        );
    }

    /// The contract's "YouTube links" rule.
    #[test]
    fn youtube_list_rule() {
        use YouTubeList::*;
        let list = |h: &str, id: &str| List(format!("https://{h}/playlist?list={id}"));
        let w = "www.youtube.com";
        let m = "music.youtube.com";
        for (url, want) in [
            (
                "https://www.youtube.com/watch?v=x&list=PLabc",
                Some(list(w, "PLabc")),
            ),
            ("https://youtu.be/x?list=PLabc&t=3", Some(list(w, "PLabc"))),
            (
                "https://m.youtube.com/watch?list=PLabc&v=x",
                Some(list(w, "PLabc")),
            ),
            (
                "HTTPS://WWW.M.YouTube.com/watch?v=x&list=PLabc",
                Some(list(w, "PLabc")),
            ),
            (
                "https://music.youtube.com/watch?v=x&list=OLAK5uy_abc",
                Some(list(m, "OLAK5uy_abc")),
            ),
            (
                "https://music.youtube.com/playlist?list=RDCLAK5uy_abc",
                Some(list(m, "RDCLAK5uy_abc")),
            ),
            (
                "https://www.youtube.com/watch?v=x&list=RDx&start_radio=1",
                Some(Single),
            ),
            (
                "https://music.youtube.com/watch?v=x&list=RDAMVMx",
                Some(Single),
            ),
            ("https://www.youtube.com/watch?v=x&list=", Some(Single)),
            ("https://www.youtube.com/watch?v=x#list=PLabc", Some(Single)),
            (
                "https://www.youtube.com/watch?v=x&list=PLa&list=PLb",
                Some(list(w, "PLa")),
            ),
            (
                "https://www.youtube.com/watch?v=x&list=pl%20x",
                Some(list(w, "pl%20x")),
            ),
            ("https://www.youtube.com/watch?v=x", Some(Single)),
            ("https://example.com/watch?v=x&list=PLabc", None),
            ("https://notyoutube.com/watch?list=PLabc", None),
        ] {
            assert_eq!(youtube_list(url), want, "{url}");
        }
    }

    #[test]
    fn encodes_search_text() {
        assert_eq!(percent_encode("Sigur Rós & co"), "Sigur+R%C3%B3s+%26+co");
    }
}
