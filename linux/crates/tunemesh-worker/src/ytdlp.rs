//! Downloads through yt-dlp: anything but Spotify, plus searches.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

use crate::child::{self, End, Stream};
use crate::job::{ItemResult, Job, SourceResult};
use crate::log;

/// `<Artist>/<Album or Singles>/<Title> [<id>].<ext>` from YouTube Music
/// fields with uploader/title fallbacks. Fields are capped in bytes so the
/// `[<id>]` suffix always survives; the worker sanitises names again when it
/// moves the file into the library.
const OUTPUT_TEMPLATE: &str =
    "%(artists.0,artist,creator,uploader,channel).80B/%(album|Singles).80B/%(track,title).80B [%(id)s].%(ext)s";

/// Crops the thumbnail to a centred square while converting it to JPEG.
const SQUARE_THUMBNAIL: &str =
    "ThumbnailsConvertor+ffmpeg_o:-c:v mjpeg -qmin 1 -q:v 2 -vf crop=\"'if(gt(ih,iw),iw,ih)':'if(gt(iw,ih),ih,iw)'\"";

/// Same fallbacks as the `<Artist>` directory in `OUTPUT_TEMPLATE`.
const PRIMARY_ARTIST_AS_ALBUM_ARTIST: &str = "%(artists.0,artist,creator,uploader,channel)s:%(meta_album_artist)s";

const PROGRESS_TEMPLATE: &str = "download:TMPROG %(progress.downloaded_bytes)s %(progress.total_bytes)s %(progress.total_bytes_estimate)s";

fn yt_dlp() -> Command {
    let mut c = Command::new("yt-dlp");
    c.args(["--ignore-config", "--no-playlist", "--no-colors"]);
    c
}

pub struct Item {
    pub id: Option<String>,
    pub url: String,
    pub title: Option<String>,
}

struct Listing {
    title: Option<String>,
    is_playlist: bool,
    items: Vec<Item>,
}

fn str_field(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned)
}

fn parse_listing(v: &Value, input: &str) -> Listing {
    let entries = v.get("entries").and_then(Value::as_array);
    match entries {
        Some(entries) => {
            let items = entries
                .iter()
                .filter(|e| !e.is_null())
                .filter_map(|e| {
                    let id = str_field(e, "id");
                    let url = str_field(e, "url").or_else(|| str_field(e, "webpage_url")).or_else(|| {
                        let ie = str_field(e, "ie_key").unwrap_or_default();
                        id.as_ref().filter(|_| ie.starts_with("Youtube")).map(|id| format!("https://www.youtube.com/watch?v={id}"))
                    })?;
                    Some(Item { id, url, title: str_field(e, "title") })
                })
                .collect();
            Listing { title: str_field(v, "title"), is_playlist: true, items }
        }
        None => {
            let url = str_field(v, "webpage_url").or_else(|| str_field(v, "original_url")).unwrap_or_else(|| input.to_owned());
            let title = str_field(v, "track").or_else(|| str_field(v, "title"));
            Listing { title: title.clone(), is_playlist: false, items: vec![Item { id: str_field(v, "id"), url, title }] }
        }
    }
}

/// Lists what `target` refers to without downloading. `Ok(None)` means the
/// request was cancelled meanwhile.
fn list(job: &Job, target: &str, first_only: bool) -> Result<Option<Listing>, String> {
    let mut cmd = yt_dlp();
    cmd.args(["--flat-playlist", "--dump-single-json", "--no-warnings"]);
    if first_only {
        cmd.args(["--playlist-items", "1"]);
    }
    cmd.arg("--").arg(target);
    let mut json = String::new();
    let outcome = child::run(
        &mut cmd,
        |stream, line| match stream {
            Stream::Out => json.push_str(line),
            Stream::Err => log!("[{}] yt-dlp: {line}", job.req.id),
        },
        || job.cancelled(),
    )
    .map_err(|e| format!("Could not run yt-dlp: {e}"))?;
    match outcome.end {
        End::Cancelled => Ok(None),
        End::Exited(s) if s.success() => {
            let v: Value = serde_json::from_str(&json).map_err(|e| format!("Unexpected yt-dlp output: {e}"))?;
            Ok(Some(parse_listing(&v, target)))
        }
        End::Exited(_) => Err(outcome.error_message()),
    }
}

fn parse_progress(line: &str) -> Option<f64> {
    let mut it = line.strip_prefix("TMPROG ")?.split(' ');
    let done: f64 = it.next()?.parse().ok()?;
    let total = it.next().and_then(|t| t.parse::<f64>().ok());
    let estimate = it.next().and_then(|t| t.parse::<f64>().ok());
    let total = total.or(estimate).filter(|t| *t > 0.0)?;
    Some((done / total).min(1.0))
}

/// Downloads one item into the job's scratch directory and moves it into
/// the library.
fn download(job: &mut Job, item: &Item, index: usize, total: usize) -> ItemResult {
    let mut cmd = yt_dlp();
    cmd.args([
        "--newline",
        "--progress",
        "--progress-template",
        PROGRESS_TEMPLATE,
        "--format",
        "bestaudio/best",
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
        "--trim-filenames",
        "110",
        "--no-mtime",
        "--no-part",
        "--print",
        "after_move:TMFILE %(filepath)s",
        "--no-simulate",
        "--paths",
    ])
    .arg(&job.dir)
    .args(["--output", OUTPUT_TEMPLATE, "--"])
    .arg(&item.url);

    let mut produced: Option<PathBuf> = None;
    let id = job.req.id.clone();
    let outcome = {
        let job_ref = &mut *job;
        let cancel_root = job_ref.root.to_path_buf();
        child::run(
            &mut cmd,
            |stream, line| match stream {
                Stream::Out => {
                    if let Some(frac) = parse_progress(line) {
                        // Leave headroom for post-processing.
                        job_ref.set_progress((index as f64 + frac * 0.9) / total as f64);
                    } else if let Some(path) = line.strip_prefix("TMFILE ") {
                        produced = Some(PathBuf::from(path));
                    } else {
                        log!("[{id}] yt-dlp: {line}");
                    }
                }
                Stream::Err => log!("[{id}] yt-dlp: {line}"),
            },
            || tunemesh_core::request::is_cancelled(&cancel_root, &id),
        )
    };
    let outcome = match outcome {
        Ok(o) => o,
        Err(e) => return ItemResult::Failed(format!("Could not run yt-dlp: {e}")),
    };
    match outcome.end {
        End::Cancelled => ItemResult::Cancelled,
        End::Exited(s) if s.success() => match produced {
            Some(path) if path.is_file() => match job.place(&path) {
                Ok(rel) => ItemResult::Track(rel),
                Err(e) => ItemResult::Failed(format!("Could not move the download into the library: {e}")),
            },
            _ => ItemResult::Failed("yt-dlp did not produce a file".to_owned()),
        },
        End::Exited(_) => ItemResult::Failed(outcome.error_message()),
    }
}

/// Downloads every item in order, deduplicating by source id.
fn download_all(job: &mut Job, items: &[Item], result: &mut SourceResult) {
    let total = items.len();
    result.total = total;
    for (i, item) in items.iter().enumerate() {
        if job.cancelled() {
            result.cancelled = true;
            return;
        }
        if let Some(existing) = item.id.as_deref().and_then(|id| job.existing(id)) {
            log!("[{}] already in library: {existing}", job.req.id);
            job.add_track(existing);
        } else {
            match download(job, item, i, total) {
                ItemResult::Track(rel) => {
                    log!("[{}] downloaded {rel}", job.req.id);
                    job.add_track(rel);
                }
                ItemResult::Failed(e) => {
                    log!("[{}] {}: {e}", job.req.id, item.url);
                    result.failures.push(e);
                }
                ItemResult::Cancelled => {
                    result.cancelled = true;
                    return;
                }
            }
        }
        job.set_progress((i + 1) as f64 / total as f64);
    }
}

pub fn run_url(job: &mut Job, url: &str) -> SourceResult {
    let mut result = SourceResult::default();
    let listing = match list(job, url, false) {
        Ok(Some(l)) => l,
        Ok(None) => return SourceResult { cancelled: true, ..result },
        Err(e) => return SourceResult { fatal: Some(e), ..result },
    };
    job.set_title(listing.title.clone());
    if listing.is_playlist {
        result.source_playlist = listing.title.clone();
    }
    download_all(job, &listing.items, &mut result);
    result
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Search: the first YouTube Music song result, else the first YouTube result.
pub fn run_search(job: &mut Job, text: &str) -> SourceResult {
    let mut result = SourceResult::default();
    let targets = [
        format!("https://music.youtube.com/search?q={}#songs", percent_encode(text)),
        format!("ytsearch1:{text}"),
    ];
    let mut last_error = None;
    for target in &targets {
        match list(job, target, true) {
            Ok(None) => return SourceResult { cancelled: true, ..result },
            Ok(Some(listing)) => {
                if let Some(item) = listing.items.into_iter().next() {
                    log!("[{}] search {text:?} resolved to {}", job.req.id, item.url);
                    job.set_title(item.title.clone());
                    download_all(job, &[item], &mut result);
                    return result;
                }
            }
            Err(e) => last_error = Some(e),
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
    }

    #[test]
    fn listing_shapes() {
        let single: Value = serde_json::json!({"_type":"video","id":"a","title":"T","webpage_url":"https://y/a"});
        let l = parse_listing(&single, "in");
        assert!(!l.is_playlist);
        assert_eq!(l.items[0].url, "https://y/a");
        let pl: Value = serde_json::json!({"_type":"playlist","title":"P","entries":[
            {"id":"a","url":"https://y/a","title":"A"}, null, {"id":"b","ie_key":"Youtube"}]});
        let l = parse_listing(&pl, "in");
        assert!(l.is_playlist);
        assert_eq!(l.title.as_deref(), Some("P"));
        assert_eq!(l.items.iter().map(|i| i.url.as_str()).collect::<Vec<_>>(), vec!["https://y/a", "https://www.youtube.com/watch?v=b"]);
    }

    #[test]
    fn encodes_search_text() {
        assert_eq!(percent_encode("Sigur Rós & co"), "Sigur+R%C3%B3s+%26+co");
    }
}
