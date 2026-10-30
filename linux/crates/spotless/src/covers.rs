//! Cover art thumbnails: decoded and scaled on worker threads, kept in a
//! small byte-bounded cache of GPU-uploadable textures.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::{gdk, glib, prelude::*};
use spotless_core::fsx::Dir;
use spotless_core::{CoverSource, Library, Track, limits};

/// Upper bound for cached thumbnail pixels.
const CACHE_BYTES: usize = 24 * 1024 * 1024;
const WORKERS: usize = 2;

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Source {
    /// Picture embedded in this root-relative audio file.
    Embedded(String),
    /// Root-relative image file.
    File(String),
    /// Generated placeholder seeded by a name.
    Placeholder(String),
}

impl Source {
    pub fn for_track(t: &Track) -> Self {
        match &t.cover {
            CoverSource::Embedded => Source::Embedded(t.path.clone()),
            CoverSource::File(f) => Source::File(f.clone()),
            CoverSource::None => {
                Source::Placeholder(format!("{}\u{1f}{}", t.listed_artist(), t.album))
            }
        }
    }

    /// The album's first track with a cover, else a placeholder.
    pub fn for_album(lib: &Library, album: usize) -> Self {
        let a = &lib.albums[album];
        a.tracks
            .iter()
            .map(|&i| &lib.tracks[i])
            .find(|t| t.cover != CoverSource::None)
            .map(Self::for_track)
            .unwrap_or_else(|| Source::Placeholder(a.key()))
    }

    pub fn for_paths<'a>(
        lib: &Library,
        paths: impl IntoIterator<Item = &'a str>,
        seed: &str,
    ) -> Self {
        paths
            .into_iter()
            .filter_map(|p| lib.track(p))
            .find(|t| t.cover != CoverSource::None)
            .map(Self::for_track)
            .unwrap_or_else(|| Source::Placeholder(seed.to_owned()))
    }
}

/// The outline drawn into a cover's pixels. With a GPU, covers are square
/// and CSS rounds them; drawn on the CPU, clipping every cover of a grid to
/// a rounded rectangle at each redraw took most of a frame, so the corners
/// are made transparent once, when the cover is decoded.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Shape {
    Square,
    /// Corners of this radius, in pixels.
    Rounded(u32),
    Round,
}

/// What a cached cover is: a source drawn at a size and shape.
type Key = (Source, u32, Shape);

struct Job {
    key: Key,
    root: PathBuf,
}

struct Done {
    key: Key,
    pixels: Vec<u8>,
}

type Callback = Box<dyn FnOnce(&gdk::Texture)>;

pub struct Covers {
    root: RefCell<PathBuf>,
    cache: RefCell<HashMap<Key, (gdk::Texture, u64)>>,
    /// Every texture made that may still be on screen, also after the
    /// cache let go of it: a cover shown again (a page kept, a grid
    /// scrolled back) gets the same texture instead of a second copy, and
    /// the GPU renderer keeps its upload.
    alive: RefCell<HashMap<Key, glib::WeakRef<gdk::Texture>>>,
    cache_bytes: Cell<usize>,
    clock: Cell<u64>,
    pending: RefCell<HashMap<Key, Vec<Callback>>>,
    jobs: async_channel::Sender<Job>,
    /// Drawn on the CPU: covers come with their outline (see `Shape`).
    software: Cell<bool>,
    /// The library the cached covers were read from.
    library: RefCell<Option<std::sync::Arc<Library>>>,
}

/// Bytes per pixel of a cover drawn in `shape`.
fn bytes_per_pixel(shape: Shape) -> usize {
    if shape == Shape::Square { 3 } else { 4 }
}

/// `rgb` (`size` x `size`) with the corners outside `shape` transparent,
/// edges smoothed over one pixel.
fn cut_out(rgb: &[u8], size: u32, shape: Shape) -> Vec<u8> {
    let n = size as f32;
    let r = match shape {
        Shape::Square => return rgb.to_vec(),
        Shape::Rounded(r) => (r as f32).min(n / 2.0),
        Shape::Round => n / 2.0,
    };
    let mut out = Vec::with_capacity(rgb.len() / 3 * 4);
    for y in 0..size {
        for x in 0..size {
            // Distance outside the rounded square, from the pixel's centre.
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let dx = (r - px).max(px - (n - r)).max(0.0);
            let dy = (r - py).max(py - (n - r)).max(0.0);
            let outside = (dx * dx + dy * dy).sqrt() - r;
            let alpha = (0.5 - outside).clamp(0.0, 1.0);
            let i = ((y * size + x) * 3) as usize;
            out.extend_from_slice(&rgb[i..i + 3]);
            out.push((alpha * 255.0).round() as u8);
        }
    }
    out
}

fn hash_of(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - f * s), v * (1.0 - (1.0 - f) * s));
    match i as i32 % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// A gradient square with a stylised record, coloured by `seed`.
fn placeholder(seed: &str, size: u32) -> Vec<u8> {
    let h = hash_of(seed);
    let hue = (h % 360) as f32 / 360.0;
    let a = hsv(hue, 0.55, 0.45);
    let b = hsv((hue + 0.12) % 1.0, 0.65, 0.22);
    let n = size as f32;
    let mut out = Vec::with_capacity((size * size * 3) as usize);
    for y in 0..size {
        for x in 0..size {
            let t = (x as f32 + y as f32) / (2.0 * n);
            let mut c = [
                a[0] + (b[0] - a[0]) * t,
                a[1] + (b[1] - a[1]) * t,
                a[2] + (b[2] - a[2]) * t,
            ];
            let (dx, dy) = (x as f32 - n / 2.0, y as f32 - n / 2.0);
            let r = (dx * dx + dy * dy).sqrt() / n;
            if r < 0.06 {
                c = [0.85, 0.85, 0.85];
            } else if r < 0.30 {
                let groove = if ((r * 120.0) as i32) % 2 == 0 {
                    0.10
                } else {
                    0.07
                };
                c = [groove, groove, groove];
            }
            out.extend(c.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8));
        }
    }
    out
}

/// The picture embedded in, or stored as, a root-relative file. Files are
/// opened below `root` without following symlinks, so a synced symlink
/// cannot show a file from outside the library.
fn read_cover(root: &Path, source: &Source) -> Option<Vec<u8>> {
    let dir = Dir::open(root).ok()?;
    match source {
        Source::Embedded(rel) => {
            let mut file = dir.open_file(rel).ok()?;
            spotless_core::library::read_embedded_cover_from(&mut file)
        }
        Source::File(rel) => dir.read(rel, limits::MAX_COVER_BYTES).ok(),
        Source::Placeholder(_) => None,
    }
}

fn decode(job: &Job) -> Vec<u8> {
    let (source, size, shape) = &job.key;
    cut_out(&decode_rgb(source, *size, &job.root), *size, *shape)
}

fn decode_rgb(source: &Source, size: u32, root: &Path) -> Vec<u8> {
    let bytes = match source {
        Source::Placeholder(seed) => return placeholder(seed, size),
        source => read_cover(root, source),
    };
    // A synced picture is untrusted input: a decoder panic leaves this
    // cover as a placeholder instead of ending the cover thread.
    // Pictures over the cover limits (bytes, or pixels per side as the
    // header states) are never decoded: the placeholder is drawn instead.
    let image = bytes.filter(|b| limits::cover_ok(b)).and_then(|b| {
        spotless_core::library::guarded("a cover picture", || image::load_from_memory(&b).ok())
            .flatten()
    });
    let Some(img) = image else {
        let seed = match source {
            Source::Embedded(s) | Source::File(s) | Source::Placeholder(s) => s,
        };
        return placeholder(seed, size);
    };
    let (w, h) = (img.width(), img.height());
    let side = w.min(h);
    let square = img.crop_imm((w - side) / 2, (h - side) / 2, side, side);
    square.thumbnail_exact(size, size).into_rgb8().into_raw()
}

fn key_bytes(key: &Key) -> usize {
    (key.1 as usize).pow(2) * bytes_per_pixel(key.2)
}

impl Covers {
    pub fn new(root: PathBuf) -> Rc<Self> {
        let (jobs_tx, jobs_rx) = async_channel::unbounded::<Job>();
        let (done_tx, done_rx) = async_channel::unbounded::<Done>();
        for i in 0..WORKERS {
            let rx = jobs_rx.clone();
            let tx = done_tx.clone();
            std::thread::Builder::new()
                .name(format!("covers-{i}"))
                .spawn(move || {
                    while let Ok(job) = rx.recv_blocking() {
                        let pixels = decode(&job);
                        if tx
                            .send_blocking(Done {
                                key: job.key,
                                pixels,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                })
                .expect("spawn cover thread");
        }
        let covers = Rc::new(Self {
            root: RefCell::new(root),
            cache: RefCell::new(HashMap::new()),
            alive: RefCell::new(HashMap::new()),
            cache_bytes: Cell::new(0),
            clock: Cell::new(0),
            pending: RefCell::new(HashMap::new()),
            jobs: jobs_tx,
            software: Cell::new(false),
            library: RefCell::new(None),
        });
        let weak = Rc::downgrade(&covers);
        glib::spawn_future_local(async move {
            while let Ok(done) = done_rx.recv().await {
                let Some(c) = weak.upgrade() else { break };
                c.finish(done);
            }
        });
        covers
    }

    pub fn set_root(&self, root: PathBuf) {
        *self.root.borrow_mut() = root;
        self.library.borrow_mut().take();
        self.cache.borrow_mut().clear();
        self.alive.borrow_mut().clear();
        self.cache_bytes.set(0);
    }

    /// Drops every cached cover (the stall walk's heap accounting).
    #[cfg(test)]
    pub fn forget_all(&self) {
        self.cache.borrow_mut().clear();
        self.alive.borrow_mut().clear();
        self.cache_bytes.set(0);
    }

    /// Covers drawn on the CPU come with their outline (see `Shape`).
    pub fn set_software(&self, on: bool) {
        self.software.set(on);
    }

    /// Drops cached covers of files that changed on disk: a track whose
    /// file is unchanged since the cover was read keeps it, so a synced
    /// file arriving does not decode every cover on screen again.
    pub fn invalidate(&self, lib: &std::sync::Arc<Library>) {
        let old = self.library.replace(Some(lib.clone()));
        let unchanged = |path: &str| {
            let (Some(old), Some(new)) =
                (old.as_ref().and_then(|o| o.track(path)), lib.track(path))
            else {
                return false;
            };
            (old.mtime, old.mtime_nsec, &old.cover) == (new.mtime, new.mtime_nsec, &new.cover)
        };
        let keep = |(s, _, _): &Key| match s {
            Source::Placeholder(_) => true,
            Source::Embedded(p) => unchanged(p),
            Source::File(_) => false,
        };
        self.cache.borrow_mut().retain(|k, _| keep(k));
        self.alive.borrow_mut().retain(|k, _| keep(k));
        let bytes = self.cache.borrow().keys().map(key_bytes).sum();
        self.cache_bytes.set(bytes);
    }

    fn finish(&self, done: Done) {
        let size = done.key.1 as usize;
        let (format, bpp) = match bytes_per_pixel(done.key.2) {
            3 => (gdk::MemoryFormat::R8g8b8, 3),
            _ => (gdk::MemoryFormat::R8g8b8a8, 4),
        };
        let texture: gdk::Texture = gdk::MemoryTexture::new(
            size as i32,
            size as i32,
            format,
            &glib::Bytes::from_owned(done.pixels),
            size * bpp,
        )
        .upcast();
        self.insert(done.key.clone(), texture.clone());
        if let Some(cbs) = self.pending.borrow_mut().remove(&done.key) {
            for cb in cbs {
                cb(&texture);
            }
        }
    }

    fn insert(&self, key: Key, texture: gdk::Texture) {
        let bytes = key_bytes(&key);
        let tick = self.clock.get() + 1;
        self.clock.set(tick);
        {
            let mut alive = self.alive.borrow_mut();
            // Forget textures no longer shown anywhere, now and then.
            if alive.len() > 4096 {
                alive.retain(|_, w| w.upgrade().is_some());
            }
            alive.insert(key.clone(), texture.downgrade());
        }
        let mut cache = self.cache.borrow_mut();
        if cache.insert(key, (texture, tick)).is_none() {
            self.cache_bytes.set(self.cache_bytes.get() + bytes);
        }
        while self.cache_bytes.get() > CACHE_BYTES && cache.len() > 1 {
            let oldest = cache
                .iter()
                .min_by_key(|(_, (_, t))| *t)
                .map(|(k, _)| k.clone());
            let Some(k) = oldest else { break };
            cache.remove(&k);
            self.cache_bytes.set(self.cache_bytes.get() - key_bytes(&k));
        }
    }

    /// Calls `cb` with the cover, immediately when cached.
    fn load(&self, key: Key, cb: impl FnOnce(&gdk::Texture) + 'static) {
        if let Some((tex, used)) = self.cache.borrow_mut().get_mut(&key) {
            let tick = self.clock.get() + 1;
            self.clock.set(tick);
            *used = tick;
            let tex = tex.clone();
            cb(&tex);
            return;
        }
        let shown = self.alive.borrow().get(&key).and_then(|w| w.upgrade());
        if let Some(tex) = shown {
            self.insert(key, tex.clone());
            cb(&tex);
            return;
        }
        let mut pending = self.pending.borrow_mut();
        let first = !pending.contains_key(&key);
        pending.entry(key.clone()).or_default().push(Box::new(cb));
        if first {
            let _ = self.jobs.send_blocking(Job {
                key,
                root: self.root.borrow().clone(),
            });
        }
    }

    /// Whether `picture` shows (or waits for) the cover of `source`.
    pub fn shows(&self, picture: &gtk::Image, source: &Source) -> bool {
        // SAFETY: only ever stored as a String, in `apply`.
        unsafe { picture.data::<String>("tm-cover-source") }
            .is_some_and(|d| *unsafe { d.as_ref() } == format!("{source:?}"))
    }

    /// Sets `picture`'s paintable to the cover once loaded (if the image
    /// still shows the same source by then).
    pub fn apply(&self, picture: &gtk::Image, source: Source, size: u32) {
        let tag = format!("{source:?}");
        // SAFETY: the stored value is only ever read back as a String.
        unsafe { picture.set_data("tm-cover-source", tag.clone()) };
        let weak = picture.downgrade();
        let scale = picture.scale_factor().max(1) as u32;
        let scaled = size * scale;
        let shape = match (self.software.get(), picture.has_css_class("round")) {
            (false, _) => Shape::Square,
            (true, true) => Shape::Round,
            // `.tm-cover`'s border-radius.
            (true, false) => Shape::Rounded(6 * scale),
        };
        self.load((source, scaled, shape), move |tex| {
            if let Some(p) = weak.upgrade() {
                // SAFETY: see above.
                let current = unsafe {
                    p.data::<String>("tm-cover-source")
                        .map(|d| d.as_ref().clone())
                };
                // Setting the same texture again still clears the image and
                // redraws it: every row of every list, at each like.
                if current.as_deref() == Some(tag.as_str())
                    && p.paintable().as_ref() != Some(tex.upcast_ref())
                {
                    p.set_paintable(Some(tex));
                }
            }
        });
    }
}

/// Writes the cover of `track` to a cache file for MPRIS `mpris:artUrl`.
/// The copy is read safely (see `read_cover`); the desktop shell then reads
/// the cache file, never a path inside the library.
pub fn art_file(root: &Path, track: &Track) -> Option<PathBuf> {
    let source = Source::for_track(track);
    match &source {
        Source::Placeholder(_) => None,
        Source::Embedded(rel) | Source::File(rel) => {
            let dir = crate::config::cache_dir().join("art");
            let file = dir.join(format!(
                "{:016x}.img",
                hash_of(&format!(
                    "{}:{}.{:09}:{rel}",
                    track.path, track.mtime, track.mtime_nsec
                ))
            ));
            if !file.exists() {
                let bytes = read_cover(root, &source)?;
                // Only the current track's art is needed; drop older files.
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for e in entries.flatten() {
                        let _ = std::fs::remove_file(e.path());
                    }
                }
                spotless_core::atomic::write(&file, &bytes).ok()?;
            }
            Some(file)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Covers drawn on the CPU have their outline in their pixels.
    #[test]
    fn software_covers_have_transparent_corners() {
        let size = 40;
        let rgb = vec![200u8; (size * size * 3) as usize];
        let alpha = |px: &[u8], x: u32, y: u32| px[((y * size + x) * 4 + 3) as usize];
        assert_eq!(cut_out(&rgb, size, Shape::Square), rgb);
        for shape in [Shape::Rounded(6), Shape::Round] {
            let px = cut_out(&rgb, size, shape);
            assert_eq!(px.len(), (size * size * 4) as usize);
            for (x, y) in [(0, 0), (size - 1, 0), (0, size - 1), (size - 1, size - 1)] {
                assert_eq!(alpha(&px, x, y), 0, "{shape:?} corner {x},{y}");
            }
            assert_eq!(alpha(&px, size / 2, size / 2), 255);
            assert!(alpha(&px, size / 2, 0) > 240, "{shape:?} edge middle");
            assert_eq!(px[0], 200);
        }
        // Rounded keeps the sides straight past its radius; round does not.
        let rounded = cut_out(&rgb, size, Shape::Rounded(6));
        let round = cut_out(&rgb, size, Shape::Round);
        assert_eq!(alpha(&rounded, 0, 10), 255);
        assert_eq!(alpha(&round, 0, 10), 0);
    }

    #[test]
    fn covers_are_never_read_through_a_symlink() {
        let base = std::env::temp_dir().join(format!("spotless-cover-test-{}", std::process::id()));
        let (root, outside) = (base.join("lib"), base.join("outside"));
        std::fs::create_dir_all(root.join("Artist/Album")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.jpg"), b"outside").unwrap();
        std::fs::write(root.join("Artist/Album/folder.jpg"), b"inside").unwrap();
        std::os::unix::fs::symlink(
            outside.join("secret.jpg"),
            root.join("Artist/Album/cover.jpg"),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            outside.join("secret.jpg"),
            root.join("Artist/Album/Song [x].mp3"),
        )
        .unwrap();
        std::os::unix::fs::symlink(&outside, root.join("Linked")).unwrap();

        let read = |s: Source| read_cover(&root, &s);
        assert_eq!(
            read(Source::File("Artist/Album/folder.jpg".into())).as_deref(),
            Some(&b"inside"[..])
        );
        assert_eq!(read(Source::File("Artist/Album/cover.jpg".into())), None);
        assert_eq!(read(Source::File("Linked/secret.jpg".into())), None);
        assert_eq!(
            read(Source::Embedded("Artist/Album/Song [x].mp3".into())),
            None
        );
        std::fs::remove_dir_all(&base).unwrap();
    }
}
