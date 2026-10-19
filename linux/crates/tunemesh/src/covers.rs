//! Cover art thumbnails: decoded and scaled on worker threads, kept in a
//! small byte-bounded cache of GPU-uploadable textures.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::rc::Rc;

use gtk::{gdk, glib, prelude::*};
use tunemesh_core::{CoverSource, Library, Track};

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

struct Job {
    source: Source,
    size: u32,
    root: PathBuf,
}

struct Done {
    source: Source,
    size: u32,
    pixels: Vec<u8>,
}

type Callback = Box<dyn FnOnce(&gdk::Texture)>;

pub struct Covers {
    root: RefCell<PathBuf>,
    cache: RefCell<HashMap<(Source, u32), (gdk::Texture, u64)>>,
    cache_bytes: Cell<usize>,
    clock: Cell<u64>,
    pending: RefCell<HashMap<(Source, u32), Vec<Callback>>>,
    jobs: async_channel::Sender<Job>,
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

fn decode(job: &Job) -> Vec<u8> {
    let bytes = match &job.source {
        Source::Embedded(rel) => tunemesh_core::library::read_embedded_cover(&job.root.join(rel)),
        Source::File(rel) => std::fs::read(job.root.join(rel)).ok(),
        Source::Placeholder(seed) => return placeholder(seed, job.size),
    };
    let image = bytes.and_then(|b| image::load_from_memory(&b).ok());
    let Some(img) = image else {
        let seed = match &job.source {
            Source::Embedded(s) | Source::File(s) | Source::Placeholder(s) => s,
        };
        return placeholder(seed, job.size);
    };
    let (w, h) = (img.width(), img.height());
    let side = w.min(h);
    let square = img.crop_imm((w - side) / 2, (h - side) / 2, side, side);
    square
        .thumbnail_exact(job.size, job.size)
        .into_rgb8()
        .into_raw()
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
                                source: job.source,
                                size: job.size,
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
            cache_bytes: Cell::new(0),
            clock: Cell::new(0),
            pending: RefCell::new(HashMap::new()),
            jobs: jobs_tx,
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
        self.cache.borrow_mut().clear();
        self.cache_bytes.set(0);
    }

    /// Drops cached thumbnails of files that changed on disk.
    pub fn invalidate(&self) {
        self.cache
            .borrow_mut()
            .retain(|(s, _), _| matches!(s, Source::Placeholder(_)));
        let bytes = self
            .cache
            .borrow()
            .keys()
            .map(|(_, size)| (*size as usize).pow(2) * 3)
            .sum();
        self.cache_bytes.set(bytes);
    }

    fn finish(&self, done: Done) {
        let size = done.size as usize;
        let texture: gdk::Texture = gdk::MemoryTexture::new(
            size as i32,
            size as i32,
            gdk::MemoryFormat::R8g8b8,
            &glib::Bytes::from_owned(done.pixels),
            size * 3,
        )
        .upcast();
        let key = (done.source, done.size);
        self.insert(key.clone(), texture.clone());
        if let Some(cbs) = self.pending.borrow_mut().remove(&key) {
            for cb in cbs {
                cb(&texture);
            }
        }
    }

    fn insert(&self, key: (Source, u32), texture: gdk::Texture) {
        let bytes = (key.1 as usize).pow(2) * 3;
        let tick = self.clock.get() + 1;
        self.clock.set(tick);
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
            self.cache_bytes
                .set(self.cache_bytes.get() - (k.1 as usize).pow(2) * 3);
        }
    }

    /// Calls `cb` with the cover at `size` pixels, immediately when cached.
    pub fn load(&self, source: Source, size: u32, cb: impl FnOnce(&gdk::Texture) + 'static) {
        let key = (source, size);
        if let Some((tex, used)) = self.cache.borrow_mut().get_mut(&key) {
            let tick = self.clock.get() + 1;
            self.clock.set(tick);
            *used = tick;
            let tex = tex.clone();
            cb(&tex);
            return;
        }
        let mut pending = self.pending.borrow_mut();
        let first = !pending.contains_key(&key);
        pending.entry(key.clone()).or_default().push(Box::new(cb));
        if first {
            let _ = self.jobs.send_blocking(Job {
                source: key.0,
                size,
                root: self.root.borrow().clone(),
            });
        }
    }

    /// Sets `picture`'s paintable to the cover once loaded (if the image
    /// still shows the same source by then).
    pub fn apply(&self, picture: &gtk::Image, source: Source, size: u32) {
        let tag = format!("{source:?}");
        // SAFETY: the stored value is only ever read back as a String.
        unsafe { picture.set_data("tm-cover-source", tag.clone()) };
        let weak = picture.downgrade();
        let scaled = size * picture.scale_factor().max(1) as u32;
        self.load(source, scaled, move |tex| {
            if let Some(p) = weak.upgrade() {
                // SAFETY: see above.
                let current = unsafe {
                    p.data::<String>("tm-cover-source")
                        .map(|d| d.as_ref().clone())
                };
                if current.as_deref() == Some(tag.as_str()) {
                    p.set_paintable(Some(tex));
                }
            }
        });
    }
}

/// Writes the cover of `track` to a cache file for MPRIS `mpris:artUrl`.
pub fn art_file(root: &std::path::Path, track: &Track) -> Option<PathBuf> {
    match &track.cover {
        CoverSource::File(rel) => Some(root.join(rel)),
        CoverSource::Embedded => {
            let dir = crate::config::cache_dir().join("art");
            let file = dir.join(format!(
                "{:016x}.img",
                hash_of(&format!("{}:{}", track.path, track.mtime))
            ));
            if !file.exists() {
                let bytes = tunemesh_core::library::read_embedded_cover(&root.join(&track.path))?;
                // Only the current track's art is needed; drop older files.
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for e in entries.flatten() {
                        let _ = std::fs::remove_file(e.path());
                    }
                }
                tunemesh_core::atomic::write(&file, &bytes).ok()?;
            }
            Some(file)
        }
        CoverSource::None => None,
    }
}
