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
use tunemesh_core::playlist::{self, LIKED_SONGS, Summary, TrackInfo};
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
    /// Playlists other than Liked Songs, as summaries: a long playlist is
    /// only held entry by entry while its page is open.
    playlists: RefCell<Vec<Summary>>,
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
        // Syncthing must never sync TuneMesh's temporary files; add the
        // contract's rules to the library's .stignore (never through a
        // symlink, keeping the user's own rules). A missing folder is left
        // alone.
        if root.is_dir()
            && let Err(e) = tunemesh_core::stignore::ensure(&root)
        {
            eprintln!("tunemesh: cannot update {}/.stignore: {e}", root.display());
        }
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
        let watcher = match watch_library(&root, move |paths| {
            let _ = tx.send_blocking(paths);
        }) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("tunemesh: cannot watch {}: {e}", root.display());
                return;
            }
        };
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

    fn read_all_playlists(&self) -> (Vec<Summary>, Playlist) {
        let root = self.root();
        let lists = playlist::list_names(&root)
            .into_iter()
            .filter_map(|name| Playlist::summary(&root, &name, SUMMARY_TRACKS).ok())
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
            .filter_map(|e| e.track().map(str::to_owned))
            .collect();
        *self.liked.borrow_mut() = liked;
    }

    fn reload_playlists(&self) {
        let (lists, liked) = self.read_all_playlists();
        // A summary does not show every change (a reorder further down), so
        // any change below Playlists/ refreshes what shows playlists.
        *self.playlists.borrow_mut() = lists;
        self.emit(StoreChange::Playlists);
        if liked != *self.liked.borrow() {
            self.set_liked(liked);
            self.emit(StoreChange::Liked);
        }
    }

    /// All playlists except Liked Songs, sorted by name.
    pub fn playlists(&self) -> Vec<Summary> {
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

    pub fn liked_summary(&self) -> Summary {
        summarize(&self.liked.borrow())
    }

    pub fn liked_count(&self) -> usize {
        self.liked.borrow().entries.len()
    }

    /// A playlist with all its entries, read from its file now.
    pub fn playlist(&self, name: &str) -> Option<Playlist> {
        if name == LIKED_SONGS {
            return Some(self.liked());
        }
        if !self.playlists.borrow().iter().any(|p| p.name == name) {
            return None;
        }
        Playlist::read(&self.root(), name)
            .inspect_err(|e| eprintln!("tunemesh: cannot read playlist {name:?}: {e}"))
            .ok()
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
    fn modify_playlist(
        &self,
        name: &str,
        f: impl FnOnce(&mut Playlist) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut p = Playlist::read(&self.root(), name)?;
        f(&mut p)?;
        self.write_playlist(&p)?;
        if name == LIKED_SONGS {
            self.set_liked(p);
            self.emit(StoreChange::Liked);
        } else {
            let mut lists = self.playlists.borrow_mut();
            match lists.iter_mut().find(|x| x.name == name) {
                Some(x) => *x = summarize(&p),
                None => lists.push(summarize(&p)),
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
        lists.push(summarize(&p));
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
        self.modify_playlist(name, |p| {
            p.append(paths.iter().map(String::as_str));
            Ok(())
        })
    }

    /// Removes the entries the user saw at these positions. The file is read
    /// again first, so entries another device moved are found by their path.
    pub fn remove_from_playlist(&self, name: &str, refs: &[EntryRef]) -> io::Result<()> {
        self.modify_playlist(name, |p| {
            let positions = locate(p, refs)?;
            p.remove_positions(&positions);
            Ok(())
        })
    }

    /// Moves the entry the user dragged to where the drop target is now.
    pub fn move_in_playlist(&self, name: &str, from: &EntryRef, to: &EntryRef) -> io::Result<()> {
        self.modify_playlist(name, |p| {
            let found = locate(p, &[from.clone(), to.clone()])?;
            p.move_entry(found[0], found[1]);
            Ok(())
        })
    }

    // ---- tracks ------------------------------------------------------

    /// Deletes track files (this syncs to every device) and empty parent
    /// directories, then drops them from the in-memory library.
    pub fn delete_tracks(&self, paths: &[String]) -> io::Result<()> {
        let (gone, result) = delete_files(&self.root(), paths);
        // Only tracks whose file is really gone leave the library; one that
        // could not be deleted stays listed.
        let remaining: Vec<Track> = self
            .lib
            .borrow()
            .tracks
            .iter()
            .filter(|t| !gone.contains(&t.path))
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
fn delete_files(root: &Path, paths: &[String]) -> (Vec<String>, io::Result<()>) {
    let top = match Dir::open(root) {
        Ok(d) => d,
        Err(e) => return (Vec::new(), Err(e)),
    };
    let mut gone = Vec::new();
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
        gone.push(rel.clone());
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
    (gone, result)
}

/// Tracks a playlist summary keeps for its cover.
const SUMMARY_TRACKS: usize = 8;

fn summarize(p: &Playlist) -> Summary {
    Summary {
        name: p.name.clone(),
        count: p.entries.len(),
        first_tracks: p
            .entries
            .iter()
            .filter_map(|e| e.track().map(str::to_owned))
            .take(SUMMARY_TRACKS)
            .collect(),
    }
}

/// A playlist entry as shown: its position and its identity (the track path,
/// or the line as written when it points outside the library).
pub type EntryRef = (usize, String);

pub fn entry_key(e: &tunemesh_core::playlist::PlaylistEntry) -> &str {
    e.key()
}

/// Finds the shown entries in a freshly read playlist: at their position if
/// it still holds the same entry, else the first unclaimed entry with that
/// identity. Fails when one is gone, so nothing wrong is changed.
fn locate(p: &Playlist, refs: &[EntryRef]) -> io::Result<Vec<usize>> {
    let mut found: Vec<usize> = Vec::with_capacity(refs.len());
    for (pos, key) in refs {
        let same = |i: usize| p.entries.get(i).is_some_and(|e| entry_key(e) == key);
        let at = if same(*pos) && !found.contains(pos) {
            Some(*pos)
        } else {
            (0..p.entries.len()).find(|&i| !found.contains(&i) && same(i))
        };
        match at {
            Some(i) => found.push(i),
            None => {
                return Err(io::Error::other(
                    "The playlist changed on another device. Nothing was changed; try again.",
                ));
            }
        }
    }
    Ok(found)
}

/// Watches `root` recursively and passes the paths of every change that is
/// not a read to `on_change`. Symlinks are never followed: a synced link
/// must not make the app watch (and walk, on the main thread) folders
/// outside the library, or loop through a "symlink bomb".
fn watch_library(
    root: &Path,
    on_change: impl Fn(Vec<PathBuf>) + Send + 'static,
) -> notify::Result<RecommendedWatcher> {
    let mut watcher = RecommendedWatcher::new(
        move |res: notify::Result<notify::Event>| {
            // Reads (open/close-nowrite) are ignored: the app's own rescans
            // and playlist reads would otherwise trigger the next rescan,
            // forever.
            if let Ok(ev) = res
                && !matches!(ev.kind, EventKind::Access(_))
            {
                on_change(ev.paths);
            }
        },
        notify::Config::default().with_follow_symlinks(false),
    )?;
    watcher.watch(root, RecursiveMode::Recursive)?;
    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests that create a `Store` own the default main context in turn.
    static MAIN_CONTEXT: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

        let (gone, r) = delete_files(
            &root,
            &[
                "Linked/Album/Keep [b].opus".into(),
                "Artist/Album/Song [a].opus".into(),
            ],
        );
        assert!(r.is_err(), "the symlinked path must be refused");
        assert_eq!(
            gone,
            ["Artist/Album/Song [a].opus"],
            "only the real track is gone"
        );
        assert!(outside.join("Album/Keep [b].opus").exists());
        assert!(
            !root.join("Artist").exists(),
            "the real track and its empty folders are gone"
        );
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn opening_a_library_adds_missing_stignore_rules() {
        let _serial = MAIN_CONTEXT.lock().unwrap_or_else(|e| e.into_inner());
        let ctx = glib::MainContext::default();
        let _owner = ctx.acquire().expect("main context");
        let base =
            std::env::temp_dir().join(format!("tunemesh-stignore-test-{}", std::process::id()));
        let (lib, other) = (base.join("lib"), base.join("other"));
        fs::create_dir_all(&lib).unwrap();
        fs::create_dir_all(&other).unwrap();
        fs::write(lib.join(".stignore"), "// mine\n*.tmp\n").unwrap();
        let store = Store::new(lib.clone());
        let text = fs::read_to_string(lib.join(".stignore")).unwrap();
        assert!(
            text.starts_with("// mine\n*.tmp\n"),
            "user rules kept: {text:?}"
        );
        assert!(
            text.lines().any(|l| l.trim() == "(?d).~tunemesh-*"),
            "{text:?}"
        );

        // A symlinked .stignore is neither followed nor replaced.
        fs::write(other.join("target"), "keep\n").unwrap();
        std::os::unix::fs::symlink(other.join("target"), other.join(".stignore")).unwrap();
        store.set_root(other.clone());
        assert_eq!(fs::read_to_string(other.join("target")).unwrap(), "keep\n");
        assert!(
            fs::symlink_metadata(other.join(".stignore"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        drop(store);
        fs::remove_dir_all(&base).unwrap();
    }

    /// This process's inotify file descriptors.
    fn inotify_fds() -> Vec<String> {
        fs::read_dir("/proc/self/fd")
            .unwrap()
            .flatten()
            .filter(|e| {
                fs::read_link(e.path()).is_ok_and(|l| l.to_string_lossy().contains("inotify"))
            })
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect()
    }

    fn watches(fd: &str) -> usize {
        fs::read_to_string(format!("/proc/self/fdinfo/{fd}"))
            .map(|s| s.lines().filter(|l| l.starts_with("inotify wd:")).count())
            .unwrap_or(0)
    }

    #[test]
    fn the_watcher_never_follows_symlinks() {
        let base = std::env::temp_dir().join(format!("tunemesh-watch-test-{}", std::process::id()));
        let (root, outside) = (base.join("lib"), base.join("outside"));
        fs::create_dir_all(root.join("Artist/Album")).unwrap();
        for i in 0..300 {
            fs::create_dir_all(outside.join(format!("d{i}"))).unwrap();
        }
        std::os::unix::fs::symlink(&outside, root.join("Linked")).unwrap();
        // A symlink bomb: 7 levels, each holding 60 links to the next.
        for level in 0..7 {
            let dir = root.join(format!(".b/B{level}"));
            fs::create_dir_all(&dir).unwrap();
            if level < 6 {
                for n in 0..60 {
                    std::os::unix::fs::symlink(
                        format!("../B{}", level + 1),
                        dir.join(format!("l{n}")),
                    )
                    .unwrap();
                }
            }
        }

        let before = inotify_fds();
        let (tx, rx) = std::sync::mpsc::channel();
        let started = std::time::Instant::now();
        let watcher = watch_library(&root, move |p| {
            let _ = tx.send(p);
        })
        .unwrap();
        let took = started.elapsed();
        let count: usize = inotify_fds()
            .iter()
            .filter(|fd| !before.contains(fd))
            .map(|fd| watches(fd))
            .sum();

        // lib, Artist, Album, .b and its 7 levels.
        assert!(
            count <= 11,
            "{count} inotify watches: symlinks were followed"
        );
        assert!(took < Duration::from_secs(1), "watching took {took:?}");

        // A change outside the library is not seen; one inside is.
        fs::write(outside.join("d1/new"), b"x").unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(
            rx.try_recv().is_err(),
            "a change outside the library was reported"
        );
        fs::write(root.join("Artist/Album/new"), b"x").unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(2)).is_ok());
        drop(watcher);
        fs::remove_dir_all(&base).unwrap();
    }

    fn playlist_lib(tag: &str, lines: &[&str]) -> (PathBuf, Rc<Store>, Vec<EntryRef>) {
        let base = std::env::temp_dir().join(format!("tunemesh-pl-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join(PLAYLISTS_DIR)).unwrap();
        write_list(&base, lines);
        let store = Store::new(base.clone());
        let shown = Playlist::read(&base, "P").unwrap();
        let refs = shown
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| (i, entry_key(e).to_owned()))
            .collect();
        (base, store, refs)
    }

    fn write_list(base: &Path, lines: &[&str]) {
        let body: String = lines.iter().map(|l| format!("../{l}.opus\n")).collect();
        fs::write(
            base.join(PLAYLISTS_DIR).join("P.m3u8"),
            format!("#EXTM3U\n{body}"),
        )
        .unwrap();
    }

    fn names(base: &Path) -> Vec<String> {
        Playlist::read(base, "P")
            .unwrap()
            .entries
            .iter()
            .map(|e| entry_key(e).trim_end_matches(".opus").to_owned())
            .collect()
    }

    #[test]
    fn removing_finds_an_entry_another_device_moved() {
        let _serial = MAIN_CONTEXT.lock().unwrap_or_else(|e| e.into_inner());
        let ctx = glib::MainContext::default();
        let _owner = ctx.acquire().expect("main context");
        let (base, store, refs) = playlist_lib("rm", &["A", "B", "C"]);
        // Another device moves C to the top; the user removes C as shown (position 2).
        write_list(&base, &["C", "A", "B"]);
        store.remove_from_playlist("P", &[refs[2].clone()]).unwrap();
        assert_eq!(names(&base), ["A", "B"]);

        // An entry that is gone: nothing changes, the user is told.
        write_list(&base, &["A", "B"]);
        let e = store
            .remove_from_playlist("P", &[(2, "C.opus".into())])
            .unwrap_err();
        assert!(e.to_string().contains("changed"), "{e}");
        assert_eq!(names(&base), ["A", "B"]);
        drop(store);
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn moving_finds_entries_after_another_device_removed_one() {
        let _serial = MAIN_CONTEXT.lock().unwrap_or_else(|e| e.into_inner());
        let ctx = glib::MainContext::default();
        let _owner = ctx.acquire().expect("main context");
        let (base, store, refs) = playlist_lib("mv", &["A", "B", "C"]);
        // Another device removes A; the user drags C (shown at 2) onto A (shown at 0).
        write_list(&base, &["B", "C"]);
        let e = store.move_in_playlist("P", &refs[2], &refs[0]).unwrap_err();
        assert!(e.to_string().contains("changed"), "{e}");
        assert_eq!(names(&base), ["B", "C"]);
        // Dragging C onto B (shown at 1, now at 0) moves it there.
        store.move_in_playlist("P", &refs[2], &refs[1]).unwrap();
        assert_eq!(names(&base), ["C", "B"]);
        drop(store);
        fs::remove_dir_all(&base).unwrap();
    }
}
