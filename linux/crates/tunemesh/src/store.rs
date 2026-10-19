//! The in-memory library and playlists, kept in sync with the folder:
//! cached scan on startup, background rescans, debounced file watching.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Duration;

use gtk::glib;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use tunemesh_core::fsx::Dir;
use tunemesh_core::playlist::{self, LIKED_SONGS, TrackInfo};
use tunemesh_core::{Library, PLAYLISTS_DIR, Playlist, TUNEMESH_DIR, Track, library};

use crate::config;

/// How long after the first file change a burst of changes is handled.
const DEBOUNCE: Duration = Duration::from_millis(700);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreChange {
    /// Tracks, albums or artists changed.
    Library,
    /// A playlist (not Liked Songs) changed, was created, renamed or deleted.
    Playlists,
    Liked,
    /// Something below `.tunemesh/` changed.
    Downloads,
    Scanning,
}

#[derive(Serialize, Deserialize)]
struct Cache {
    root: PathBuf,
    tracks: Vec<Track>,
}

fn cache_file() -> PathBuf {
    config::cache_dir().join("library.json")
}

type Listener = Rc<dyn Fn(StoreChange)>;

pub struct Store {
    me: Weak<Store>,
    root: RefCell<PathBuf>,
    lib: RefCell<Rc<Library>>,
    playlists: RefCell<Vec<Playlist>>,
    liked: RefCell<Playlist>,
    liked_set: RefCell<HashSet<String>>,
    listeners: RefCell<Vec<Listener>>,
    watcher: RefCell<Option<RecommendedWatcher>>,
    scanning: Cell<bool>,
    rescan_again: Cell<bool>,
    generation: Cell<u64>,
}

impl Store {
    pub fn new(root: PathBuf) -> Rc<Self> {
        let store = Rc::new_cyclic(|me| Self {
            me: me.clone(),
            root: RefCell::new(root),
            lib: RefCell::new(Rc::new(Library::default())),
            playlists: RefCell::new(Vec::new()),
            liked: RefCell::new(Playlist {
                name: LIKED_SONGS.into(),
                entries: Vec::new(),
            }),
            liked_set: RefCell::new(HashSet::new()),
            listeners: RefCell::new(Vec::new()),
            watcher: RefCell::new(None),
            scanning: Cell::new(false),
            rescan_again: Cell::new(false),
            generation: Cell::new(0),
        });
        store.open();
        store
    }

    /// Loads the cache and playlists for the current root, then watches and rescans.
    fn open(&self) {
        let root = self.root();
        let cached = fs::read(cache_file())
            .ok()
            .and_then(|b| serde_json::from_slice::<Cache>(&b).ok())
            .filter(|c| c.root == root)
            .map(|c| c.tracks)
            .unwrap_or_default();
        *self.lib.borrow_mut() = Rc::new(Library::new(cached));
        self.load_playlists();
        self.watch();
        self.rescan();
    }

    pub fn set_root(&self, root: PathBuf) {
        self.generation.set(self.generation.get() + 1);
        self.scanning.set(false);
        self.rescan_again.set(false);
        *self.root.borrow_mut() = root;
        self.watcher.borrow_mut().take();
        self.open();
        self.emit(StoreChange::Library);
        self.emit(StoreChange::Playlists);
        self.emit(StoreChange::Liked);
        self.emit(StoreChange::Downloads);
    }

    pub fn root(&self) -> PathBuf {
        self.root.borrow().clone()
    }

    pub fn connect(&self, f: impl Fn(StoreChange) + 'static) {
        self.listeners.borrow_mut().push(Rc::new(f));
    }

    fn emit(&self, change: StoreChange) {
        let listeners: Vec<Listener> = self.listeners.borrow().clone();
        for l in listeners {
            l(change);
        }
    }

    pub fn library(&self) -> Rc<Library> {
        self.lib.borrow().clone()
    }

    pub fn is_scanning(&self) -> bool {
        self.scanning.get()
    }

    // ---- scanning and watching ----------------------------------------

    pub fn rescan(&self) {
        if self.scanning.get() {
            self.rescan_again.set(true);
            return;
        }
        self.scanning.set(true);
        self.emit(StoreChange::Scanning);
        let root = self.root();
        let previous: HashMap<String, Track> = self
            .lib
            .borrow()
            .tracks
            .iter()
            .map(|t| (t.path.clone(), t.clone()))
            .collect();
        let generation = self.generation.get();
        let (tx, rx) = async_channel::bounded::<Vec<Track>>(1);
        std::thread::Builder::new()
            .name("library-scan".into())
            .spawn(move || {
                let tracks = library::scan(&root, &previous);
                let cache = Cache { root, tracks };
                if let Ok(json) = serde_json::to_vec(&cache) {
                    let _ = tunemesh_core::atomic::write(&cache_file(), &json);
                }
                let _ = tx.send_blocking(cache.tracks);
            })
            .expect("spawn scan thread");
        let weak = self.me.clone();
        glib::spawn_future_local(async move {
            let Ok(tracks) = rx.recv().await else { return };
            let Some(store) = weak.upgrade() else { return };
            if store.generation.get() != generation {
                return;
            }
            store.scanning.set(false);
            if tracks != store.lib.borrow().tracks {
                *store.lib.borrow_mut() = Rc::new(Library::new(tracks));
                store.emit(StoreChange::Library);
            }
            store.emit(StoreChange::Scanning);
            if store.rescan_again.replace(false) {
                store.rescan();
            }
        });
    }

    fn watch(&self) {
        let root = self.root();
        if !root.is_dir() {
            return;
        }
        let (tx, rx) = async_channel::unbounded::<Vec<PathBuf>>();
        // Reads (open/close-nowrite) are ignored: the app's own rescans and
        // playlist reads would otherwise trigger the next rescan, forever.
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res
                && !matches!(ev.kind, EventKind::Access(_))
            {
                let _ = tx.send_blocking(ev.paths);
            }
        });
        let mut watcher = match watcher {
            Ok(w) => w,
            Err(e) => {
                eprintln!("tunemesh: cannot watch the library: {e}");
                return;
            }
        };
        if let Err(e) = watcher.watch(&root, RecursiveMode::Recursive) {
            eprintln!("tunemesh: cannot watch {}: {e}", root.display());
            return;
        }
        *self.watcher.borrow_mut() = Some(watcher);
        let weak = self.me.clone();
        let generation = self.generation.get();
        glib::spawn_future_local(async move {
            // Changes arrive in bursts (a sync, a download): handle each burst
            // once, DEBOUNCE after its first event.
            while let Ok(first) = rx.recv().await {
                glib::timeout_future(DEBOUNCE).await;
                let mut paths: HashSet<PathBuf> = first.into_iter().collect();
                while let Ok(more) = rx.try_recv() {
                    paths.extend(more);
                }
                let Some(store) = weak.upgrade() else { break };
                if store.generation.get() != generation {
                    break;
                }
                store.on_fs_events(&root, &paths.into_iter().collect::<Vec<_>>());
            }
        });
    }

    fn on_fs_events(&self, root: &Path, paths: &[PathBuf]) {
        let (mut tracks, mut playlists, mut downloads) = (false, false, false);
        for p in paths {
            let Ok(rel) = p.strip_prefix(root) else {
                continue;
            };
            let mut comps = rel.iter().filter_map(|c| c.to_str());
            match comps.next() {
                Some(PLAYLISTS_DIR) => playlists = true,
                Some(TUNEMESH_DIR) => downloads = true,
                Some(first) => {
                    let hidden = first.starts_with('.')
                        || rel.iter().any(|c| c.to_string_lossy().starts_with('.'));
                    if !hidden {
                        tracks = true;
                    }
                }
                None => tracks = true,
            }
        }
        if playlists {
            self.reload_playlists();
        }
        if downloads {
            self.emit(StoreChange::Downloads);
        }
        if tracks {
            self.rescan();
        }
    }

    // ---- playlists ---------------------------------------------------

    fn read_all_playlists(&self) -> (Vec<Playlist>, Playlist) {
        let root = self.root();
        let lists = playlist::list_names(&root)
            .into_iter()
            .filter_map(|name| Playlist::read(&root, &name).ok())
            .collect();
        let liked = Playlist::read(&root, LIKED_SONGS).unwrap_or(Playlist {
            name: LIKED_SONGS.into(),
            entries: Vec::new(),
        });
        (lists, liked)
    }

    fn load_playlists(&self) {
        let (lists, liked) = self.read_all_playlists();
        *self.playlists.borrow_mut() = lists;
        self.set_liked(liked);
    }

    fn set_liked(&self, liked: Playlist) {
        *self.liked_set.borrow_mut() = liked
            .entries
            .iter()
            .filter_map(|e| e.track.clone())
            .collect();
        *self.liked.borrow_mut() = liked;
    }

    fn reload_playlists(&self) {
        let (lists, liked) = self.read_all_playlists();
        if lists != *self.playlists.borrow() {
            *self.playlists.borrow_mut() = lists;
            self.emit(StoreChange::Playlists);
        }
        if liked != *self.liked.borrow() {
            self.set_liked(liked);
            self.emit(StoreChange::Liked);
        }
    }

    /// All playlists except Liked Songs, sorted by name.
    pub fn playlists(&self) -> Vec<Playlist> {
        self.playlists.borrow().clone()
    }

    pub fn playlist_names(&self) -> Vec<String> {
        self.playlists
            .borrow()
            .iter()
            .map(|p| p.name.clone())
            .collect()
    }

    pub fn liked(&self) -> Playlist {
        self.liked.borrow().clone()
    }

    pub fn liked_count(&self) -> usize {
        self.liked.borrow().entries.len()
    }

    pub fn playlist(&self, name: &str) -> Option<Playlist> {
        if name == LIKED_SONGS {
            return Some(self.liked());
        }
        self.playlists
            .borrow()
            .iter()
            .find(|p| p.name == name)
            .cloned()
    }

    pub fn is_liked(&self, path: &str) -> bool {
        self.liked_set.borrow().contains(path)
    }

    fn track_info(&self) -> impl Fn(&str) -> Option<TrackInfo> + use<> {
        let lib = self.library();
        move |path: &str| {
            lib.track(path).map(|t| TrackInfo {
                duration_secs: t.duration_secs(),
                label: format!("{} - {}", t.artist, t.title),
            })
        }
    }

    fn write_playlist(&self, p: &Playlist) -> io::Result<()> {
        p.write(&self.root(), &self.track_info())
    }

    pub fn set_liked_paths(&self, paths: &[String], like: bool) -> io::Result<()> {
        // Re-read first so likes made on other devices are not overwritten.
        let mut liked = Playlist::read(&self.root(), LIKED_SONGS)?;
        if like {
            for p in paths.iter().rev() {
                liked.like(p);
            }
        } else {
            for p in paths {
                liked.unlike(p);
            }
        }
        self.write_playlist(&liked)?;
        self.set_liked(liked);
        self.emit(StoreChange::Liked);
        Ok(())
    }

    pub fn toggle_like(&self, path: &str) -> io::Result<bool> {
        let like = !self.is_liked(path);
        self.set_liked_paths(&[path.to_owned()], like)?;
        Ok(like)
    }

    /// Applies `f` to a freshly read playlist and writes it back.
    fn modify_playlist(&self, name: &str, f: impl FnOnce(&mut Playlist)) -> io::Result<()> {
        let mut p = Playlist::read(&self.root(), name)?;
        f(&mut p);
        self.write_playlist(&p)?;
        if name == LIKED_SONGS {
            self.set_liked(p);
            self.emit(StoreChange::Liked);
        } else {
            let mut lists = self.playlists.borrow_mut();
            match lists.iter_mut().find(|x| x.name == name) {
                Some(x) => *x = p,
                None => lists.push(p),
            }
            drop(lists);
            self.emit(StoreChange::Playlists);
        }
        Ok(())
    }

    pub fn create_playlist(&self, name: &str) -> io::Result<String> {
        let p = Playlist::create(&self.root(), name)?;
        let name = p.name.clone();
        let mut lists = self.playlists.borrow_mut();
        lists.push(p);
        lists.sort_by_cached_key(|p| tunemesh_core::search::fold(&p.name));
        drop(lists);
        self.emit(StoreChange::Playlists);
        Ok(name)
    }

    pub fn rename_playlist(&self, old: &str, new: &str) -> io::Result<String> {
        let new = Playlist::rename(&self.root(), old, new)?;
        let mut lists = self.playlists.borrow_mut();
        if let Some(p) = lists.iter_mut().find(|p| p.name == old) {
            p.name = new.clone();
        }
        lists.sort_by_cached_key(|p| tunemesh_core::search::fold(&p.name));
        drop(lists);
        self.emit(StoreChange::Playlists);
        Ok(new)
    }

    pub fn delete_playlist(&self, name: &str) -> io::Result<()> {
        Playlist::delete(&self.root(), name)?;
        self.playlists.borrow_mut().retain(|p| p.name != name);
        self.emit(StoreChange::Playlists);
        Ok(())
    }

    pub fn add_to_playlist(&self, name: &str, paths: &[String]) -> io::Result<()> {
        if name == LIKED_SONGS {
            return self.set_liked_paths(paths, true);
        }
        self.modify_playlist(name, |p| p.append(paths.iter().map(String::as_str)))
    }

    pub fn remove_from_playlist(&self, name: &str, positions: &[usize]) -> io::Result<()> {
        self.modify_playlist(name, |p| p.remove_positions(positions))
    }

    pub fn move_in_playlist(&self, name: &str, from: usize, to: usize) -> io::Result<()> {
        self.modify_playlist(name, |p| p.move_entry(from, to))
    }

    // ---- tracks ------------------------------------------------------

    /// Deletes track files (this syncs to every device) and empty parent
    /// directories, then drops them from the in-memory library.
    pub fn delete_tracks(&self, paths: &[String]) -> io::Result<()> {
        let result = delete_files(&self.root(), paths);
        let remaining: Vec<Track> = self
            .lib
            .borrow()
            .tracks
            .iter()
            .filter(|t| !paths.contains(&t.path))
            .cloned()
            .collect();
        *self.lib.borrow_mut() = Rc::new(Library::new(remaining));
        self.emit(StoreChange::Library);
        result
    }
}

/// Deletes library files and their emptied parent directories. Paths are
/// opened below `root` without following symlinks, so a synced symlinked
/// folder cannot redirect a delete outside the library.
fn delete_files(root: &Path, paths: &[String]) -> io::Result<()> {
    let top = Dir::open(root)?;
    let mut result = Ok(());
    for rel in paths {
        let (parent, name) = rel.rsplit_once('/').unwrap_or(("", rel.as_str()));
        let removed = if parent.is_empty() {
            top.try_clone()
        } else {
            top.sub(parent, false)
        }
        .and_then(|d| d.remove_file(name));
        if let Err(e) = removed
            && e.kind() != io::ErrorKind::NotFound
        {
            result = Err(e);
            continue;
        }
        // Emptied parent folders go too (never through a symlink).
        let mut dir = parent;
        while !dir.is_empty() {
            let (up, name) = dir.rsplit_once('/').unwrap_or(("", dir));
            let removed = if up.is_empty() {
                top.try_clone()
            } else {
                top.sub(up, false)
            }
            .and_then(|d| d.remove_dir(name));
            if removed.is_err() {
                break;
            }
            dir = up;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deleting_never_follows_a_symlinked_folder() {
        let base =
            std::env::temp_dir().join(format!("tunemesh-delete-test-{}", std::process::id()));
        let (root, outside) = (base.join("lib"), base.join("outside"));
        fs::create_dir_all(root.join("Artist/Album")).unwrap();
        fs::create_dir_all(outside.join("Album")).unwrap();
        fs::write(root.join("Artist/Album/Song [a].opus"), b"x").unwrap();
        fs::write(outside.join("Album/Keep [b].opus"), b"x").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("Linked")).unwrap();

        let r = delete_files(
            &root,
            &[
                "Linked/Album/Keep [b].opus".into(),
                "Artist/Album/Song [a].opus".into(),
            ],
        );
        assert!(r.is_err(), "the symlinked path must be refused");
        assert!(outside.join("Album/Keep [b].opus").exists());
        assert!(
            !root.join("Artist").exists(),
            "the real track and its empty folders are gone"
        );
        fs::remove_dir_all(&base).unwrap();
    }
}
