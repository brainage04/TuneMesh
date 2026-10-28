//! Processing one request from start to final status.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tunemesh_core::fsx::{Dir, Kind};
use tunemesh_core::library::{self, source_id_from_file_name};
use tunemesh_core::merge::Bases;
use tunemesh_core::playlist::{self, LIKED_SONGS, Playlist, TrackInfo};
use tunemesh_core::request::{self, Request, RequestState, Status};
use tunemesh_core::sanitize;

use crate::child::Stop;
use crate::config::Limits;
use crate::http::Http;
use crate::proxy::Proxy;
use crate::rundir::RunDir;
use crate::tools::{Resolved, Tools};
use crate::{leader, log, signals, spotify, ytdlp};

/// How often progress-only status updates are written (each write syncs to
/// every device).
const STATUS_INTERVAL: Duration = Duration::from_secs(1);

/// Source ids (`[<id>]` in file names) mapped to the library path carrying
/// them, for skipping downloads that are already there. Built on first use
/// only (a request that fails early never walks the library) and kept
/// between requests for `SourceIds::MAX_AGE`; the worker's own placements
/// update it, and a path is checked to still exist before it is used, so a
/// kept index misses at most files another device added since.
#[derive(Default)]
pub struct SourceIds {
    index: HashMap<String, String>,
    built: Option<Instant>,
    /// Library walks so far.
    pub walks: usize,
}

impl SourceIds {
    pub const MAX_AGE: Duration = Duration::from_secs(10 * 60);

    /// The index, walking the library if there is none or it is too old.
    fn get(&mut self, root: &Path) -> &mut HashMap<String, String> {
        if self.built.is_none_or(|t| t.elapsed() >= Self::MAX_AGE) {
            self.index = library::source_id_index(root);
            self.built = Some(Instant::now());
            self.walks += 1;
        }
        &mut self.index
    }

    pub fn is_built(&self) -> bool {
        self.built.is_some()
    }
}

/// What every job shares.
pub struct Ctx {
    pub root: PathBuf,
    pub work_dir: PathBuf,
    /// This process's own part of the work directory.
    pub run: RunDir,
    pub limits: Limits,
    pub proxy: Proxy,
    pub http: Http,
    pub tools: Tools,
    pub source_ids: SourceIds,
    /// This device's playlist bases (contract: "Playlist conflict copies").
    pub bases: Bases,
    /// A final status that could not be written: its request file is kept,
    /// and no other request is taken until it is written
    /// (`retry_unwritten`).
    pub unwritten: Option<Status>,
}

pub struct Job<'a> {
    pub root: &'a Path,
    pub ctx: &'a Ctx,
    pub tools: Resolved,
    /// Per-request scratch directory outside the library.
    pub dir: PathBuf,
    pub req: Request,
    pub status: Status,
    /// Source id → library path, for deduplication.
    pub ids: SourceIds,
    /// Items that failed, including those beyond the listed `failures`.
    pub failed: usize,
    /// Every track the request produced or matched (the status lists at
    /// most `MAX_STATUS_TRACKS`), for the playlist.
    pub tracks: Vec<String>,
    last_write: Instant,
}

/// What a source did for the request.
#[derive(Default)]
pub struct SourceResult {
    /// Title of the source playlist/album, for `playlistFromSource`.
    pub source_playlist: Option<String>,
    /// Number of items the request covered.
    pub total: usize,
    pub stopped: Option<Stop>,
    /// The source could not be resolved at all.
    pub fatal: Option<String>,
}

impl SourceResult {
    pub fn fatal(e: String) -> Self {
        Self {
            fatal: Some(e),
            ..Self::default()
        }
    }

    pub fn stopped(s: Stop) -> Self {
        Self {
            stopped: Some(s),
            ..Self::default()
        }
    }
}

/// Bytes below `dir` (regular files only, symlinks not followed).
fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(e.path()),
                Ok(t) if t.is_file() => total += e.metadata().map_or(0, |m| m.len()),
                _ => {}
            }
        }
    }
    total
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i + 1 < UNITS.len() {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

impl Job<'_> {
    /// Why the current work should stop, if it should.
    pub fn stop_reason(&self) -> Option<Stop> {
        if signals::stop_requested() || leader::lost() {
            return Some(Stop::Interrupted);
        }
        if request::is_cancelled(self.root, &self.req.id) {
            return Some(Stop::Cancelled);
        }
        None
    }

    /// `stop_reason`, plus the scratch directory size limit while a
    /// downloader runs (yt-dlp cannot enforce `--max-filesize` when the size
    /// is not announced in advance).
    pub fn child_check(&self) -> Option<Stop> {
        self.stop_reason().or_else(|| {
            let limit = self
                .ctx
                .limits
                .max_file_size
                .saturating_mul(3)
                .saturating_add(64 << 20);
            (dir_size(&self.dir) > limit).then(|| {
                Stop::Limit(format!(
                    "The download is larger than the {} limit",
                    human_bytes(self.ctx.limits.max_file_size)
                ))
            })
        })
    }

    /// Writes the status now if `force` or enough time has passed.
    pub fn flush(&mut self, force: bool) {
        if (force || self.last_write.elapsed() >= STATUS_INTERVAL)
            && let Err(e) = self.write_status()
        {
            log!("[{}] writing status: {e}", self.req.id);
        }
    }

    fn write_status(&mut self) -> io::Result<()> {
        self.status.updated_at = tunemesh_core::now_rfc3339().into();
        self.last_write = Instant::now();
        self.status.write(self.root)
    }

    pub fn set_title(&mut self, title: Option<String>) {
        let title = title.map(request::Text::from_string);
        if title.is_some() && self.status.title != title {
            self.status.title = title;
            self.flush(true);
        }
    }

    pub fn set_total(&mut self, total: usize) {
        self.status.items_total = Some(total as u32);
        self.status.items_done = Some(0);
        self.flush(true);
    }

    /// Marks item `index` (0-based) of `total` finished and writes the
    /// status.
    pub fn item_done(&mut self, index: usize, total: usize) {
        self.status.items_done = Some((index + 1) as u32);
        self.status.progress = Some(((index + 1) as f64 / total.max(1) as f64).clamp(0.0, 1.0));
        self.flush(true);
    }

    pub fn warn(&mut self, warning: String) {
        log!("[{}] {warning}", self.req.id);
        self.status.add_warning(warning);
        self.flush(true);
    }

    /// Records a failed item; written with the next `item_done`.
    pub fn fail_item(&mut self, item: String, error: String) {
        log!("[{}] {item}: {error}", self.req.id);
        self.failed += 1;
        self.status.add_failure(item, error);
    }

    fn root_dir(&self) -> io::Result<Dir> {
        Dir::open(self.root)
    }

    /// The library path of an already present track with this source id.
    pub fn existing(&mut self, id: &str) -> Option<String> {
        let root = self.root_dir().ok()?;
        self.ids
            .get(self.root)
            .get(id)
            .filter(|rel| root.is_file(rel))
            .cloned()
    }

    /// Records a track; written with the next `item_done`.
    pub fn add_track(&mut self, rel: String) {
        self.status.tracks.push(&rel);
        self.tracks.push(rel);
    }

    /// Free space check before a download starts.
    pub fn check_space(&self) -> Result<(), String> {
        let min = self.ctx.limits.min_free_space;
        for (what, dir) in [
            ("library", self.root),
            ("work directory", self.ctx.work_dir.as_path()),
        ] {
            let free = Dir::open(dir)
                .and_then(|d| d.free_space())
                .map_err(|e| format!("{what}: {e}"))?;
            if free < min {
                return Err(format!(
                    "Not enough free space in the {what}: {} left, {} must stay free",
                    human_bytes(free),
                    human_bytes(min)
                ));
            }
        }
        Ok(())
    }

    /// Moves a finished download from the scratch directory into the library
    /// as `<Artist>/<Album>/<Title> [<id>].<ext>`, deduplicating by id.
    ///
    /// `produced` must be a regular file inside this job's scratch directory.
    /// Every directory on the way into the library is opened without
    /// following symlinks, so a synced symlink in place of an artist or album
    /// directory is refused rather than written through.
    pub fn place(&mut self, produced: &Path) -> io::Result<String> {
        let job_dir = self.dir.canonicalize()?;
        let canonical = produced.canonicalize()?;
        let rel = match canonical.strip_prefix(&job_dir) {
            Ok(rel) if !rel.as_os_str().is_empty() => rel.to_path_buf(),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "refusing {}: not inside the job directory",
                        produced.display()
                    ),
                ));
            }
        };
        let meta = fs::symlink_metadata(&canonical)?;
        if !meta.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{} is not a regular file", produced.display()),
            ));
        }
        let parts: Vec<&str> = rel.iter().filter_map(|c| c.to_str()).collect();
        let name = *parts
            .last()
            .ok_or_else(|| io::Error::other("empty file name"))?;
        let album = parts.len().checked_sub(2).map_or("Singles", |i| parts[i]);
        let artist = parts
            .len()
            .checked_sub(3)
            .map_or(library::UNKNOWN_ARTIST, |i| parts[i]);
        let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
        let id = source_id_from_file_name(name).map(str::to_owned);
        let (title, suffix) = match &id {
            Some(id) => (&stem[..stem.len() - id.len() - 3], format!(" [{id}].{ext}")),
            None => (stem, format!(".{ext}")),
        };
        if let Some(existing) = id.as_deref().and_then(|id| self.existing(id)) {
            fs::remove_file(&canonical)?;
            return Ok(existing);
        }
        // Android's shared storage is case-insensitive: "XI" and "xi" are one
        // directory there and Syncthing refuses to sync the second, so an
        // artist or album directory that exists in another casing is reused.
        let root = self.root_dir()?;
        let artist_name = existing_casing(&root, &sanitize::component(artist));
        let artist_dir = root.sub(&artist_name, true)?;
        let album_name = existing_casing(&artist_dir, &sanitize::component(album));
        let album_dir = artist_dir.sub(&album_name, true)?;
        let file_name = sanitize::file_name(title, &suffix);
        let target_rel = format!("{artist_name}/{album_name}/{file_name}");

        let free = album_dir.free_space()?;
        let min = self.ctx.limits.min_free_space;
        if free < meta.len().saturating_add(min) {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                format!(
                    "not enough free space in the library ({} left, {} must stay free)",
                    human_bytes(free),
                    human_bytes(min)
                ),
            ));
        }
        match album_dir.move_in(&canonical, &file_name) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                if album_dir.kind(&file_name)? != Kind::File {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("refusing {target_rel}: not a regular file"),
                    ));
                }
                fs::remove_file(&canonical)?;
            }
            Err(e) => return Err(e),
        }
        if let Some(id) = id {
            self.ids.get(self.root).insert(id, target_rel.clone());
        }
        Ok(target_rel)
    }
}

/// `name`, or the name of an existing directory in `parent` that differs from
/// it only in letter case (symlinks never match).
fn existing_casing(parent: &Dir, name: &str) -> String {
    let folded = name.to_lowercase();
    parent
        .entries()
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, kind)| *kind == Kind::Dir)
        .map(|(n, _)| n)
        .find(|existing| existing.to_lowercase() == folded)
        .unwrap_or_else(|| name.to_owned())
}

fuzz_pub! {
enum Input {
    Spotify(String),
    Url(String),
    Search(String),
}
}

fn host(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?.split(':').next()?;
    Some(host.to_ascii_lowercase())
}

fuzz_pub! {
/// Classifies request input: Spotify URL, other URL, or search text. Bare
/// `host.tld/path` input without a scheme counts as an https URL.
fn classify(input: &str) -> Input {
    let input = input.trim();
    let lower = input.to_ascii_lowercase();
    let url = if lower.starts_with("http://") || lower.starts_with("https://") {
        Some(input.to_owned())
    } else if !input.contains(char::is_whitespace)
        && input.split('/').next().is_some_and(|h| {
            h.contains('.')
                && h.rsplit('.').next().is_some_and(|tld| {
                    tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic())
                })
        })
        && input.contains('/')
    {
        Some(format!("https://{input}"))
    } else {
        None
    };
    match url {
        Some(u) if host(&u).as_deref() == Some("open.spotify.com") => Input::Spotify(u),
        Some(u) => Input::Url(u),
        None => Input::Search(input.to_owned()),
    }
}
}

/// Appends `tracks` to the playlist `name` (Liked Songs: inserted at the
/// top), creating it if needed. A playlist whose name differs only in letter
/// case is the same playlist. Returns the name of the playlist written.
pub fn add_to_playlist(
    root: &Path,
    bases: &Bases,
    name: &str,
    tracks: &[String],
) -> io::Result<String> {
    let name = playlist::sanitize_name(name);
    let name = playlist::existing_name(root, &name).unwrap_or(name);
    let dir = Dir::open(root)?;
    let infos: HashMap<&str, (Option<u64>, String)> = tracks
        .iter()
        .map(|rel| {
            let tags = dir
                .open_file(rel)
                .map(|mut f| library::read_tags_from(&mut f))
                .unwrap_or_default();
            let t = library::make_track(rel, tags, None, (0, 0), 0);
            (
                rel.as_str(),
                (t.duration_secs(), format!("{} - {}", t.artist, t.title)),
            )
        })
        .collect();
    let info = |path: &str| {
        infos.get(path).map(|(d, l)| TrackInfo {
            duration_secs: *d,
            label: l.clone(),
        })
    };
    // Under the Playlists/ lock, so an app on this machine editing the same
    // playlist does not lose its edit or ours.
    let add = |p: &mut Playlist| {
        if p.name == LIKED_SONGS {
            p.like_all(tracks.iter().map(String::as_str));
        } else {
            p.append(tracks.iter().map(String::as_str));
        }
        Ok(())
    };
    match Playlist::edit(root, bases, &name, &info, add) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            match Playlist::create(root, &name) {
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
            Playlist::edit(root, bases, &name, &info, add)?;
        }
        r => {
            r?;
        }
    }
    Ok(name)
}

/// Processes one request to a final state, or back to `queued` when the
/// worker is interrupted. A final status that cannot be written goes to
/// `ctx.unwritten`, and the request file stays.
pub fn process(ctx: &mut Ctx, req: Request) {
    let ids = std::mem::take(&mut ctx.source_ids);
    (ctx.source_ids, ctx.unwritten) = run(ctx, req, ids);
}

/// Writes the final status kept in `ctx.unwritten`, if any, then removes
/// its request file. Returns whether the worker may take another request:
/// nothing was waiting, it is written now, or its request was cancelled
/// meanwhile (the sweep then sees the status it has).
pub fn retry_unwritten(ctx: &mut Ctx) -> bool {
    let Some(mut status) = ctx.unwritten.take() else {
        return true;
    };
    let id = status.id.to_string();
    if request::is_cancelled(&ctx.root, &id) {
        return true;
    }
    status.updated_at = tunemesh_core::now_rfc3339().into();
    if status.write(&ctx.root).is_err() {
        ctx.unwritten = Some(status);
        return false;
    }
    log!("[{id}] final status written");
    if let Err(e) = request::cancel(&ctx.root, &id) {
        log!("[{id}] removing request file: {e}");
    }
    true
}

/// `process`; returns the source-id index for the next request and the
/// final status, if it could not be written.
fn run(ctx: &mut Ctx, req: Request, ids: SourceIds) -> (SourceIds, Option<Status>) {
    let id = req.id.clone();
    log!("[{id}] processing {:?}", req.input);
    let dir = ctx.run.jobs().join(&*id);
    let _ = fs::remove_dir_all(&dir);
    if let Err(e) = fs::create_dir_all(&dir) {
        log!("[{id}] creating {}: {e}", dir.display());
    }
    // A request whose input was cut on reading fails at once, without
    // resolving tools.
    let too_long = req.input.was_cut();
    let tools = if too_long {
        Err("The request is too long (more than 8 KiB)".to_owned())
    } else {
        ctx.tools.resolve(&ctx.http)
    };
    let ctx = &*ctx;
    let mut status = Status::new(&req, RequestState::Downloading);
    status.progress = Some(0.0);
    let mut job = Job {
        root: &ctx.root,
        ctx,
        tools: Resolved {
            yt_dlp: PathBuf::new(),
            deno: None,
            ffmpeg: PathBuf::new(),
            ffprobe: PathBuf::new(),
        },
        dir: dir.clone(),
        req,
        status,
        ids,
        failed: 0,
        tracks: Vec::new(),
        last_write: Instant::now(),
    };
    job.flush(true);

    let result = match tools {
        Err(e) => SourceResult::fatal(e),
        Ok(tools) => {
            job.tools = tools;
            match job.check_space() {
                Err(e) => SourceResult::fatal(e),
                Ok(()) => match classify(&job.req.input) {
                    Input::Spotify(url) => spotify::run(&mut job, &url),
                    Input::Url(url) => ytdlp::run_url(&mut job, &url),
                    Input::Search(text) => ytdlp::run_search(&mut job, &text),
                },
            }
        }
    };
    let unwritten = finish(&mut job, result);
    let _ = fs::remove_dir_all(&dir);
    (job.ids, unwritten)
}

/// Sets and writes the final status, then removes the request file. When
/// the final status cannot be written, the request file stays and the
/// status is returned, so the request is neither lost nor shown unfinished
/// for good (`retry_unwritten`).
fn finish(job: &mut Job, r: SourceResult) -> Option<Status> {
    let id = job.req.id.clone();
    if r.stopped == Some(Stop::Interrupted) {
        // Resumed by whichever worker runs next.
        let s = &mut job.status;
        s.state = RequestState::Queued;
        s.progress = None;
        s.message = Some("Interrupted; continues when a downloader is running".into());
        log!("[{id}] interrupted");
        job.flush(true);
        return None;
    }
    let failed = job.failed;
    let s = &mut job.status;
    if r.stopped.is_some() {
        s.state = RequestState::Cancelled;
        s.progress = None;
    } else if let Some(fatal) = r.fatal {
        s.state = RequestState::Failed;
        s.progress = None;
        s.message = Some(fatal.into());
    } else if job.tracks.is_empty() {
        s.state = RequestState::Failed;
        s.progress = None;
        let first = s.failures.first().map(|f| f.error.to_string());
        s.message = Some(
            match (r.total, first) {
                (0, _) => "Nothing to download".to_owned(),
                (1, Some(e)) => e,
                (n, Some(e)) => format!("All {n} tracks unavailable: {e}"),
                (_, None) => "Nothing was downloaded".to_owned(),
            }
            .into(),
        );
    } else {
        s.state = RequestState::Done;
        s.progress = Some(1.0);
        let mut notes = Vec::new();
        if failed > 0 {
            notes.push(format!("{failed} of {} tracks unavailable", r.total));
        }
        notes.extend(s.warnings.iter().map(|w| w.to_string()));
        let target = job
            .req
            .playlist
            .as_deref()
            .filter(|p| !p.trim().is_empty())
            .map(str::to_owned)
            .or_else(|| {
                if job.req.playlist_from_source {
                    r.source_playlist.clone()
                } else {
                    None
                }
            });
        if let Some(name) = target {
            match add_to_playlist(job.root, &job.ctx.bases, &name, &job.tracks) {
                Ok(name) => log!(
                    "[{id}] added {} track(s) to playlist {name:?}",
                    job.tracks.len()
                ),
                Err(e) => {
                    // Too large (contract: "Playlists"): the playlist is
                    // left unchanged and the warning says so.
                    let warning = format!("Could not update playlist “{name}”: {e}");
                    if playlist::is_too_large(&e) {
                        s.add_warning(warning.clone());
                    }
                    notes.push(warning);
                }
            }
        } else if job.req.playlist_from_source && job.req.playlist.is_none() {
            notes.push(
                "The source is not a playlist or album, so no playlist was created".to_owned(),
            );
        }
        if !notes.is_empty() {
            s.message = Some(notes.join(". ").into());
        }
    }
    log!(
        "[{id}] {:?}{}",
        s.state,
        s.message
            .as_deref()
            .map(|m| format!(": {m}"))
            .unwrap_or_default()
    );
    if let Err(e) = job.write_status() {
        log!("[{id}] writing the final status: {e}; the request stays until it is written");
        return Some(job.status.clone());
    }
    if let Err(e) = request::cancel(job.root, &id) {
        log!("[{id}] removing request file: {e}");
    }
    None
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    pub fn test_ctx(root: &Path, work: &Path) -> Ctx {
        let proxy = Proxy::start().unwrap();
        let http = Http::new(&proxy.url()).unwrap();
        Ctx {
            root: root.to_path_buf(),
            work_dir: work.to_path_buf(),
            run: RunDir::claim(work).unwrap(),
            limits: Limits {
                min_free_space: 0,
                ..Limits::default()
            },
            proxy,
            http,
            tools: Tools::new(None),
            source_ids: SourceIds::default(),
            bases: Bases::none(),
            unwritten: None,
        }
    }

    pub fn test_job<'a>(ctx: &'a Ctx, dir: &Path) -> Job<'a> {
        let req = Request::new("x", None, false, "t");
        let status = Status::new(&req, RequestState::Downloading);
        Job {
            root: &ctx.root,
            ctx,
            tools: Resolved {
                yt_dlp: PathBuf::new(),
                deno: None,
                ffmpeg: PathBuf::new(),
                ffprobe: PathBuf::new(),
            },
            dir: dir.to_path_buf(),
            req,
            status,
            ids: SourceIds::default(),
            failed: 0,
            tracks: Vec::new(),
            last_write: Instant::now(),
        }
    }

    fn produce(work: &Path, rel: &str) -> PathBuf {
        let p = work.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, b"x").unwrap();
        p
    }

    /// R3Deepseek-3: every request walked the whole library first, even one
    /// that failed at once (39 ms per tiny request on a 100k-file library).
    #[test]
    fn early_failures_do_not_walk_the_library() {
        let lib = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(lib.path(), work.path());
        let req = Request::new(&"q".repeat(9 * 1024), None, false, "t");
        let id = req.id.to_string();
        process(&mut ctx, req);
        assert!(!ctx.source_ids.is_built());
        let s = request::read_status(lib.path(), &id).unwrap();
        assert_eq!(s.state, RequestState::Failed);
        assert!(s.message.unwrap().contains("too long"));
        // Built on first use, then kept.
        let mut job = test_job(&ctx, work.path());
        assert_eq!(job.existing("x"), None);
        assert_eq!(job.existing("y"), None);
        assert_eq!(job.ids.walks, 1);
    }

    /// R4D-1: a final status that cannot be written (the status directory
    /// replaced by a symlink, a full disk) keeps the request file instead of
    /// deleting it with no final status; the worker takes no other request
    /// until the status is written, then removes the request.
    #[test]
    fn an_unwritten_final_status_keeps_the_request() {
        let lib = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(lib.path(), work.path());
        let req = Request::new(&"q".repeat(9 * 1024), None, false, "t");
        let id = req.id.to_string();
        let requests = request::requests_dir(lib.path());
        fs::create_dir_all(&requests).unwrap();
        let request_file = requests.join(format!("{id}.json"));
        fs::write(&request_file, b"{}").unwrap();
        let status_dir = request::status_dir(lib.path());
        symlink(elsewhere.path(), &status_dir).unwrap();
        process(&mut ctx, req);
        assert!(request_file.exists(), "the request was deleted");
        assert!(!retry_unwritten(&mut ctx), "written into a symlink");
        assert!(request_file.exists());
        assert!(fs::read_dir(elsewhere.path()).unwrap().next().is_none());
        fs::remove_file(&status_dir).unwrap();
        assert!(retry_unwritten(&mut ctx));
        assert!(!request_file.exists());
        assert!(ctx.unwritten.is_none());
        let s = request::read_status(lib.path(), &id).unwrap();
        assert_eq!(s.state, RequestState::Failed);
        assert!(s.message.unwrap().contains("too long"));
        assert!(retry_unwritten(&mut ctx), "nothing left to write");
    }

    #[test]
    fn classifies_inputs() {
        assert!(matches!(
            classify("https://open.spotify.com/track/abc"),
            Input::Spotify(_)
        ));
        assert!(matches!(
            classify("HTTPS://www.youtube.com/watch?v=x"),
            Input::Url(_)
        ));
        assert!(
            matches!(classify("youtu.be/jNQXAC9IVRw"), Input::Url(u) if u == "https://youtu.be/jNQXAC9IVRw")
        );
        assert!(matches!(
            classify("open.spotify.com/album/x"),
            Input::Spotify(_)
        ));
        assert!(matches!(
            classify("never gonna give you up"),
            Input::Search(_)
        ));
        assert!(matches!(classify("AC/DC"), Input::Search(_)));
        assert!(matches!(classify("v1.2/remix"), Input::Search(_)));
    }

    #[test]
    fn place_sanitises_and_dedupes() {
        let lib = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let ctx = test_ctx(lib.path(), work.path());
        let mut job = test_job(&ctx, work.path());
        let produced = produce(work.path(), "A：B/Singles/What？ [id1].opus");
        assert_eq!(
            job.place(&produced).unwrap(),
            "A：B/Singles/What？ [id1].opus"
        );

        let produced = produce(work.path(), "Other/Album./T: x [id1].opus");
        assert_eq!(
            job.place(&produced).unwrap(),
            "A：B/Singles/What？ [id1].opus"
        );
        assert!(!produced.exists());

        let produced = produce(work.path(), "Other/Album./T: x [id2].m4a");
        assert_eq!(job.place(&produced).unwrap(), "Other/Album_/T_ x [id2].m4a");
    }

    #[test]
    fn place_reuses_directories_that_differ_only_in_case() {
        let lib = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        fs::create_dir_all(lib.path().join("XI/Parousia")).unwrap();
        let ctx = test_ctx(lib.path(), work.path());
        let mut job = test_job(&ctx, work.path());
        let produced = produce(work.path(), "xi/parousia/Other [id3].opus");
        assert_eq!(
            job.place(&produced).unwrap(),
            "XI/Parousia/Other [id3].opus"
        );
        let produced = produce(work.path(), "xi/New Album/Song [id4].opus");
        assert_eq!(
            job.place(&produced).unwrap(),
            "XI/New Album/Song [id4].opus"
        );
        let dirs: Vec<_> = fs::read_dir(lib.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(dirs, vec!["XI"]);
    }

    /// Audit finding 2: a synced symlink in place of an artist directory must
    /// not make the worker write outside the library.
    #[test]
    fn place_refuses_symlinked_artist_and_album_dirs() {
        let lib = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), lib.path().join("Artist")).unwrap();
        fs::create_dir(lib.path().join("Other")).unwrap();
        symlink(outside.path(), lib.path().join("Other/Album")).unwrap();
        let ctx = test_ctx(lib.path(), work.path());
        let mut job = test_job(&ctx, work.path());
        let produced = produce(work.path(), "Artist/Album/Song [id5].opus");
        assert!(job.place(&produced).is_err());
        let produced2 = produce(work.path(), "Other/Album/Song [id6].opus");
        assert!(job.place(&produced2).is_err());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
        assert!(produced.exists() && produced2.exists());
    }

    /// Audit finding 5: only files inside the job directory are moved.
    #[test]
    fn place_refuses_files_outside_the_job_dir() {
        let lib = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let ctx = test_ctx(lib.path(), work.path());
        let job_dir = work.path().join("jobs/1");
        fs::create_dir_all(&job_dir).unwrap();
        let mut job = test_job(&ctx, &job_dir);
        let secret = produce(elsewhere.path(), "A/B/secret [id7].opus");
        let err = job.place(&secret).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        // Also through `..` and through a symlink inside the job directory.
        let sneaky = job_dir
            .join("../../")
            .join(secret.strip_prefix("/").unwrap());
        assert!(job.place(&sneaky).is_err());
        symlink(&secret, job_dir.join("link [id8].opus")).unwrap();
        assert!(job.place(&job_dir.join("link [id8].opus")).is_err());
        assert!(secret.exists());
        assert_eq!(fs::read_dir(lib.path()).unwrap().count(), 0);
    }

    #[test]
    fn place_keeps_the_free_space_reserve() {
        let lib = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(lib.path(), work.path());
        ctx.limits.min_free_space = u64::MAX / 2;
        let mut job = test_job(&ctx, work.path());
        let produced = produce(work.path(), "A/B/c [id9].opus");
        assert_eq!(
            job.place(&produced).unwrap_err().kind(),
            io::ErrorKind::StorageFull
        );
        assert!(job.check_space().is_err());
    }

    #[test]
    fn playlist_append_creates_and_liked_inserts_at_top() {
        let lib = tempfile::tempdir().unwrap();
        let root = lib.path();
        let tracks = vec!["A/B/one [1].opus".to_owned(), "A/B/two [2].opus".to_owned()];
        assert_eq!(
            add_to_playlist(root, &Bases::none(), "Mix: 1", &tracks).unwrap(),
            "Mix_ 1"
        );
        add_to_playlist(root, &Bases::none(), "Mix: 1", &tracks[..1]).unwrap();
        let p = Playlist::read(root, "Mix_ 1").unwrap();
        assert_eq!(p.entries.len(), 3);

        fs::create_dir_all(root.join("Playlists")).unwrap();
        fs::write(
            playlist::playlist_path(root, LIKED_SONGS),
            "#EXTM3U\n../old.mp3\n",
        )
        .unwrap();
        add_to_playlist(root, &Bases::none(), LIKED_SONGS, &tracks).unwrap();
        let liked = Playlist::read(root, LIKED_SONGS).unwrap();
        let order: Vec<_> = liked
            .entries
            .iter()
            .map(|e| e.track().unwrap().to_owned())
            .collect();
        assert_eq!(
            order,
            vec!["A/B/one [1].opus", "A/B/two [2].opus", "old.mp3"]
        );
    }

    #[test]
    fn playlist_names_match_case_insensitively() {
        let lib = tempfile::tempdir().unwrap();
        let root = lib.path();
        let tracks = vec!["A/B/one [1].opus".to_owned()];
        Playlist::create(root, "Road Trip").unwrap();
        assert_eq!(
            add_to_playlist(root, &Bases::none(), "road trip", &tracks).unwrap(),
            "Road Trip"
        );
        // "liked songs" typed into a request is Liked Songs, with its insert-at-top rule.
        assert_eq!(
            add_to_playlist(root, &Bases::none(), "liked songs", &tracks).unwrap(),
            LIKED_SONGS
        );
        assert_eq!(playlist::list_names(root), vec!["Road Trip"]);
        assert_eq!(Playlist::read(root, "Road Trip").unwrap().entries.len(), 1);
        assert!(
            Playlist::read(root, LIKED_SONGS)
                .unwrap()
                .contains("A/B/one [1].opus")
        );
        let files: Vec<_> = fs::read_dir(playlist::playlists_dir(root))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(files.len(), 2);
    }

    /// Audit finding 1 from the worker's side: a request naming a playlist
    /// that is a symlink must not read the link's target into the library.
    #[test]
    fn symlinked_request_playlist_is_not_read() {
        let lib = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("key");
        fs::write(&secret, "ssh-ed25519 AAAA\n").unwrap();
        let root = lib.path();
        fs::create_dir(root.join("Playlists")).unwrap();
        symlink(&secret, playlist::playlist_path(root, "x")).unwrap();
        let tracks = vec!["A/B/one [1].opus".to_owned()];
        assert!(add_to_playlist(root, &Bases::none(), "x", &tracks).is_err());
        assert_eq!(fs::read_to_string(&secret).unwrap(), "ssh-ed25519 AAAA\n");
        assert!(
            fs::symlink_metadata(playlist::playlist_path(root, "x"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    /// A playlist over the contract's limits is not appended to, and the
    /// error says why.
    #[test]
    fn too_large_playlists_are_left_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("Playlists")).unwrap();
        let text = format!(
            "#EXTM3U\n{}",
            "../a.mp3\n".repeat(tunemesh_core::limits::MAX_PLAYLIST_ENTRIES + 1)
        );
        fs::write(root.join("Playlists/Big.m3u8"), &text).unwrap();
        let err = add_to_playlist(root, &Bases::none(), "Big", &["b.mp3".to_owned()]).unwrap_err();
        assert!(playlist::is_too_large(&err));
        assert!(
            err.to_string()
                .contains("too large (more than 250,000 entries)")
        );
        assert_eq!(
            fs::read_to_string(root.join("Playlists/Big.m3u8")).unwrap(),
            text
        );
    }
}
