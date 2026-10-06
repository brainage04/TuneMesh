//! Processing one request from start to final status.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tunemesh_core::library::{self, has_audio_extension, source_id_from_file_name};
use tunemesh_core::playlist::{self, LIKED_SONGS, Playlist, TrackInfo};
use tunemesh_core::request::{self, Request, RequestState, Status};
use tunemesh_core::{atomic, sanitize};

use crate::{log, spotdl, ytdlp};

/// How often progress-only status updates are written (each write syncs to
/// every device).
const STATUS_INTERVAL: Duration = Duration::from_secs(1);

pub struct Job<'a> {
    pub root: &'a Path,
    /// Per-request scratch directory outside the library.
    pub dir: PathBuf,
    pub req: Request,
    pub status: Status,
    /// Source id → library path, for deduplication.
    pub ids: HashMap<String, String>,
    last_write: Instant,
}

/// What a source (yt-dlp or spotDL) did for the request.
#[derive(Default)]
pub struct SourceResult {
    /// Title of the source playlist/album, for `playlistFromSource`.
    pub source_playlist: Option<String>,
    /// Number of items the source listed.
    pub total: usize,
    /// Error messages of items that could not be downloaded.
    pub failures: Vec<String>,
    pub cancelled: bool,
    /// The source could not be resolved at all.
    pub fatal: Option<String>,
}

pub enum ItemResult {
    Track(String),
    Failed(String),
    Cancelled,
}

impl Job<'_> {
    pub fn cancelled(&self) -> bool {
        request::is_cancelled(self.root, &self.req.id)
    }

    /// Writes the status now if `force` or enough time has passed.
    pub fn flush(&mut self, force: bool) {
        if force || self.last_write.elapsed() >= STATUS_INTERVAL {
            self.status.updated_at = tunemesh_core::now_rfc3339();
            if let Err(e) = self.status.write(self.root) {
                log!("[{}] writing status: {e}", self.req.id);
            }
            self.last_write = Instant::now();
        }
    }

    pub fn set_progress(&mut self, p: f64) {
        self.status.progress = Some(p.clamp(0.0, 1.0));
        self.flush(false);
    }

    pub fn set_title(&mut self, title: Option<String>) {
        if title.is_some() && self.status.title != title {
            self.status.title = title;
            self.flush(true);
        }
    }

    /// The library path of an already present track with this source id.
    pub fn existing(&self, id: &str) -> Option<String> {
        self.ids.get(id).filter(|rel| self.root.join(rel).is_file()).cloned()
    }

    pub fn add_track(&mut self, rel: String) {
        self.status.tracks.push(rel);
        self.flush(true);
    }

    /// Moves a finished download from the scratch directory into the library
    /// as `<Artist>/<Album>/<Title> [<id>].<ext>`, deduplicating by id.
    pub fn place(&mut self, produced: &Path) -> io::Result<String> {
        let rel = produced.strip_prefix(&self.dir).unwrap_or(produced);
        let parts: Vec<&str> = rel.iter().filter_map(|c| c.to_str()).collect();
        let name = *parts.last().ok_or_else(|| io::Error::other("empty file name"))?;
        let album = parts.len().checked_sub(2).map_or("Singles", |i| parts[i]);
        let artist = parts.len().checked_sub(3).map_or(library::UNKNOWN_ARTIST, |i| parts[i]);
        let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
        let id = source_id_from_file_name(name).map(str::to_owned);
        let (title, suffix) = match &id {
            Some(id) => (&stem[..stem.len() - id.len() - 3], format!(" [{id}].{ext}")),
            None => (stem, format!(".{ext}")),
        };
        let target_rel = format!(
            "{}/{}/{}",
            sanitize::component(artist),
            sanitize::component(album),
            sanitize::file_name(title, &suffix)
        );
        if let Some(existing) = id.as_deref().and_then(|id| self.existing(id)) {
            fs::remove_file(produced)?;
            return Ok(existing);
        }
        let target = self.root.join(&target_rel);
        if target.exists() {
            fs::remove_file(produced)?;
        } else {
            atomic::move_file(produced, &target)?;
        }
        if let Some(id) = id {
            self.ids.insert(id, target_rel.clone());
        }
        Ok(target_rel)
    }
}

/// Finds the downloaded audio file carrying `[id]` below `dir`.
pub fn find_download(dir: &Path, id: &str) -> Option<PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d).ok()?.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                stack.push(path);
            } else if has_audio_extension(&name) && source_id_from_file_name(&name) == Some(id) {
                return Some(path);
            }
        }
    }
    None
}

pub enum Input {
    Spotify(String),
    Url(String),
    Search(String),
}

fn host(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?.split(':').next()?;
    Some(host.to_ascii_lowercase())
}

/// Classifies request input: Spotify URL, other URL, or search text. Bare
/// `host.tld/path` input without a scheme counts as an https URL.
pub fn classify(input: &str) -> Input {
    let input = input.trim();
    let lower = input.to_ascii_lowercase();
    let url = if lower.starts_with("http://") || lower.starts_with("https://") {
        Some(input.to_owned())
    } else if !input.contains(char::is_whitespace)
        && input.split('/').next().is_some_and(|h| {
            h.contains('.') && h.rsplit('.').next().is_some_and(|tld| tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic()))
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

/// Appends `tracks` to the playlist `name` (Liked Songs: inserted at the
/// top), creating it if needed. Returns the playlist's sanitised name.
pub fn add_to_playlist(root: &Path, name: &str, tracks: &[String]) -> io::Result<String> {
    let name = playlist::sanitize_name(name);
    let mut p = match Playlist::read(root, &name) {
        Ok(p) => p,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Playlist { name: name.clone(), entries: Vec::new() },
        Err(e) => return Err(e),
    };
    if name == LIKED_SONGS {
        for t in tracks.iter().rev() {
            p.like(t);
        }
    } else {
        p.append(tracks.iter().map(String::as_str));
    }
    let infos: HashMap<&str, (Option<u64>, String)> = tracks
        .iter()
        .map(|rel| {
            let t = library::make_track(rel, library::read_tags(&root.join(rel)), None, 0, 0);
            (rel.as_str(), (t.duration_secs(), format!("{} - {}", t.artist, t.title)))
        })
        .collect();
    p.write(root, &|path| {
        infos.get(path).map(|(d, l)| TrackInfo { duration_secs: *d, label: l.clone() })
    })?;
    Ok(name)
}

pub fn process(root: &Path, work_dir: &Path, req: Request) {
    let id = req.id.clone();
    log!("[{id}] processing {:?}", req.input);
    let dir = work_dir.join(&id);
    let _ = fs::remove_dir_all(&dir);
    if let Err(e) = fs::create_dir_all(&dir) {
        log!("[{id}] creating {}: {e}", dir.display());
    }
    let mut status = Status::new(&req, RequestState::Downloading);
    status.progress = Some(0.0);
    let mut job = Job {
        root,
        dir: dir.clone(),
        req,
        status,
        ids: library::source_id_index(root),
        last_write: Instant::now(),
    };
    job.flush(true);

    let result = match classify(&job.req.input) {
        Input::Spotify(url) => spotdl::run(&mut job, &url),
        Input::Url(url) => ytdlp::run_url(&mut job, &url),
        Input::Search(text) => ytdlp::run_search(&mut job, &text),
    };
    finish(&mut job, result);
    let _ = fs::remove_dir_all(&dir);
}

fn finish(job: &mut Job, r: SourceResult) {
    let id = job.req.id.clone();
    let s = &mut job.status;
    if r.cancelled {
        s.state = RequestState::Cancelled;
        s.progress = None;
    } else if let Some(fatal) = r.fatal {
        s.state = RequestState::Failed;
        s.progress = None;
        s.message = Some(fatal);
    } else if s.tracks.is_empty() {
        s.state = RequestState::Failed;
        s.progress = None;
        s.message = Some(match (r.total, r.failures.first()) {
            (0, _) => "Nothing to download".to_owned(),
            (1, Some(e)) => e.clone(),
            (n, Some(e)) => format!("All {n} tracks unavailable: {e}"),
            (_, None) => "Nothing was downloaded".to_owned(),
        });
    } else {
        s.state = RequestState::Done;
        s.progress = Some(1.0);
        let mut notes = Vec::new();
        if !r.failures.is_empty() {
            notes.push(format!("{} of {} tracks unavailable", r.failures.len(), r.total));
        }
        let target = job.req.playlist.clone().filter(|p| !p.trim().is_empty()).or_else(|| {
            if job.req.playlist_from_source { r.source_playlist.clone() } else { None }
        });
        if let Some(name) = target {
            match add_to_playlist(job.root, &name, &s.tracks) {
                Ok(name) => log!("[{id}] added {} track(s) to playlist {name:?}", s.tracks.len()),
                Err(e) => notes.push(format!("Could not update playlist “{name}”: {e}")),
            }
        } else if job.req.playlist_from_source && job.req.playlist.is_none() {
            notes.push("The source is not a playlist or album, so no playlist was created".to_owned());
        }
        if !notes.is_empty() {
            s.message = Some(notes.join(". "));
        }
    }
    log!("[{id}] {:?}{}", s.state, s.message.as_deref().map(|m| format!(": {m}")).unwrap_or_default());
    job.flush(true);
    if let Err(e) = request::cancel(job.root, &id) {
        log!("[{id}] removing request file: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_inputs() {
        assert!(matches!(classify("https://open.spotify.com/track/abc"), Input::Spotify(_)));
        assert!(matches!(classify("HTTPS://www.youtube.com/watch?v=x"), Input::Url(_)));
        assert!(matches!(classify("youtu.be/jNQXAC9IVRw"), Input::Url(u) if u == "https://youtu.be/jNQXAC9IVRw"));
        assert!(matches!(classify("open.spotify.com/album/x"), Input::Spotify(_)));
        assert!(matches!(classify("never gonna give you up"), Input::Search(_)));
        assert!(matches!(classify("AC/DC"), Input::Search(_)));
        assert!(matches!(classify("v1.2/remix"), Input::Search(_)));
    }

    #[test]
    fn place_sanitises_and_dedupes() {
        let lib = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let req = Request::new("x", None, false, "t");
        let status = Status::new(&req, RequestState::Downloading);
        let mut job = Job {
            root: lib.path(),
            dir: work.path().to_path_buf(),
            req,
            status,
            ids: HashMap::new(),
            last_write: Instant::now(),
        };
        let produced = work.path().join("A：B/Singles/What？ [id1].opus");
        fs::create_dir_all(produced.parent().unwrap()).unwrap();
        fs::write(&produced, b"x").unwrap();
        assert_eq!(job.place(&produced).unwrap(), "A：B/Singles/What？ [id1].opus");

        let produced = work.path().join("Other/Album./T: x [id1].opus");
        fs::create_dir_all(produced.parent().unwrap()).unwrap();
        fs::write(&produced, b"x").unwrap();
        assert_eq!(job.place(&produced).unwrap(), "A：B/Singles/What？ [id1].opus");
        assert!(!produced.exists());

        let produced = work.path().join("Other/Album./T: x [id2].m4a");
        fs::write(&produced, b"x").unwrap();
        assert_eq!(job.place(&produced).unwrap(), "Other/Album_/T_ x [id2].m4a");
    }

    #[test]
    fn playlist_append_creates_and_liked_inserts_at_top() {
        let lib = tempfile::tempdir().unwrap();
        let root = lib.path();
        let tracks = vec!["A/B/one [1].opus".to_owned(), "A/B/two [2].opus".to_owned()];
        assert_eq!(add_to_playlist(root, "Mix: 1", &tracks).unwrap(), "Mix_ 1");
        add_to_playlist(root, "Mix: 1", &tracks[..1]).unwrap();
        let p = Playlist::read(root, "Mix_ 1").unwrap();
        assert_eq!(p.entries.len(), 3);

        fs::create_dir_all(root.join("Playlists")).unwrap();
        fs::write(playlist::playlist_path(root, LIKED_SONGS), "#EXTM3U\n../old.mp3\n").unwrap();
        add_to_playlist(root, LIKED_SONGS, &tracks).unwrap();
        let liked = Playlist::read(root, LIKED_SONGS).unwrap();
        let order: Vec<_> = liked.entries.iter().map(|e| e.track.clone().unwrap()).collect();
        assert_eq!(order, vec!["A/B/one [1].opus", "A/B/two [2].opus", "old.mp3"]);
    }
}
