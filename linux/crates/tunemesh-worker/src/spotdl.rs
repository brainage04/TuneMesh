//! Spotify URLs through spotDL: Spotify supplies metadata, audio comes from
//! YouTube Music.

use std::fs;
use std::process::Command;

use serde::Deserialize;

use crate::child::{self, End};
use crate::job::{Job, SourceResult, find_download};
use crate::log;

/// spotDL output template; `{track-id}` is the Spotify id used for
/// deduplication.
const OUTPUT_TEMPLATE: &str = "{artist}/{album}/{title} [{track-id}].{output-ext}";

#[derive(Debug, Deserialize)]
struct Song {
    name: String,
    song_id: String,
    url: String,
    #[serde(default)]
    album_name: Option<String>,
    #[serde(default)]
    list_name: Option<String>,
}

fn spotdl() -> Command {
    Command::new("spotdl")
}

/// `--lyrics` with no providers, last on the command line so it consumes
/// nothing after it: TuneMesh shows no lyrics, and the default providers
/// (Genius, AZLyrics, Musixmatch) cost about 30 s per spotDL run, most of it
/// AZLyrics timing out.
const NO_LYRICS: &str = "--lyrics";

/// Fetches the song list for a Spotify URL into `<dir>/songs.spotdl`.
fn save(job: &Job, url: &str) -> Result<Option<Vec<Song>>, String> {
    let file = job.dir.join("songs.spotdl");
    let mut cmd = spotdl();
    cmd.args(["save", url, "--log-level", "ERROR", "--save-file"]).arg(&file).arg(NO_LYRICS).current_dir(&job.dir);
    let id = job.req.id.clone();
    let outcome = child::run(&mut cmd, |_, line| log!("[{id}] spotdl: {line}"), || job.cancelled())
        .map_err(|e| format!("Could not run spotdl: {e}"))?;
    match outcome.end {
        End::Cancelled => Ok(None),
        End::Exited(s) if s.success() => {
            let bytes = fs::read(&file).map_err(|e| format!("spotdl did not save the song list: {e}"))?;
            serde_json::from_slice(&bytes).map(Some).map_err(|e| format!("Unexpected spotdl output: {e}"))
        }
        End::Exited(_) => Err(outcome.error_message()),
    }
}

pub fn run(job: &mut Job, url: &str) -> SourceResult {
    let mut result = SourceResult::default();
    let songs = match save(job, url) {
        Ok(Some(s)) => s,
        Ok(None) => return SourceResult { cancelled: true, ..result },
        Err(e) => return SourceResult { fatal: Some(e), ..result },
    };
    let is_track = url.contains("/track/");
    let is_album = url.contains("/album/");
    let first = songs.first();
    let source_title = if is_album {
        first.and_then(|s| s.album_name.clone().or_else(|| s.list_name.clone()))
    } else {
        first.and_then(|s| s.list_name.clone())
    };
    job.set_title(if is_track { first.map(|s| s.name.clone()) } else { source_title.clone() });
    if !is_track {
        result.source_playlist = source_title;
    }

    let total = songs.len();
    result.total = total;
    let out_dir = job.dir.join("spotdl");
    for (i, song) in songs.iter().enumerate() {
        if job.cancelled() {
            result.cancelled = true;
            return result;
        }
        if let Some(existing) = job.existing(&song.song_id) {
            log!("[{}] already in library: {existing}", job.req.id);
            job.add_track(existing);
            job.set_progress((i + 1) as f64 / total as f64);
            continue;
        }
        let _ = fs::create_dir_all(&out_dir);
        let mut cmd = spotdl();
        cmd.args([
            "download",
            &song.url,
            "--output",
            OUTPUT_TEMPLATE,
            // Keep YouTube's Opus stream: with `disable`, spotDL copies the
            // audio instead of re-encoding when the source is WebM/Opus.
            "--format",
            "opus",
            "--bitrate",
            "disable",
            "--overwrite",
            "skip",
            "--simple-tui",
            "--log-level",
            "INFO",
            "--print-errors",
            NO_LYRICS,
        ])
        .current_dir(&out_dir);
        let id = job.req.id.clone();
        let root = job.root.to_path_buf();
        let outcome = child::run(
            &mut cmd,
            |_, line| log!("[{id}] spotdl: {line}"),
            || tunemesh_core::request::is_cancelled(&root, &id),
        );
        let outcome = match outcome {
            Ok(o) => o,
            Err(e) => {
                result.failures.push(format!("Could not run spotdl: {e}"));
                continue;
            }
        };
        if let End::Cancelled = outcome.end {
            result.cancelled = true;
            return result;
        }
        match find_download(&out_dir, &song.song_id) {
            Some(path) => match job.place(&path) {
                Ok(rel) => {
                    log!("[{}] downloaded {rel}", job.req.id);
                    job.add_track(rel);
                }
                Err(e) => result.failures.push(format!("Could not move the download into the library: {e}")),
            },
            None => {
                let msg = outcome.error_message();
                log!("[{}] {}: {msg}", job.req.id, song.url);
                result.failures.push(format!("{}: {msg}", song.name));
            }
        }
        job.set_progress((i + 1) as f64 / total as f64);
    }
    result
}
