//! GUI tests: the real window on a display (headless sway or Xvfb), driven
//! from inside the process.
//!
//! - `leaks`: every page, panel and dialog is freed once it is closed.
//! - `stalls`: the longest main-loop stall per action on the hostile library
//!   (release build).
//!
//! Both need a display and are `#[ignore]`d; `scripts/gui-checks.sh` starts a
//! headless compositor and runs them. GTK may only be used from one thread,
//! so every test hands its body to [`on_gtk`].

mod leaks;
mod stalls;

use std::cell::Cell;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{LazyLock, Mutex, mpsc};
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::app::App;
use crate::config::Config;

type Job = Box<dyn FnOnce() + Send>;

/// The thread of this test process that uses GTK, fed through a channel.
static JOBS: LazyLock<Mutex<mpsc::Sender<Job>>> = LazyLock::new(|| {
    // As in main(): two malloc arenas (threads made from now on share them).
    // SAFETY: plain libc call.
    unsafe { libc::mallopt(libc::M_ARENA_MAX, 2) };
    let (tx, rx) = mpsc::channel::<Job>();
    std::thread::Builder::new()
        .name("gtk".into())
        .spawn(move || {
            gtk::init().expect("GTK needs a display (see scripts/gui-checks.sh)");
            adw::init().expect("libadwaita");
            gio::resources_register_include!("tunemesh.gresource").expect("register resources");
            crate::app::load_style();
            for job in rx {
                job();
            }
        })
        .expect("spawn GTK thread");
    Mutex::new(tx)
});

/// Runs `f` on the one thread of this test process that uses GTK.
fn on_gtk<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    let (tx, rx) = mpsc::channel();
    JOBS.lock()
        .expect("GTK job queue")
        .send(Box::new(move || {
            let _ = tx.send(panic::catch_unwind(AssertUnwindSafe(f)));
        }))
        .expect("GTK thread ended");
    match rx.recv().expect("GTK thread ended (no display?)") {
        Ok(r) => r,
        Err(p) => panic::resume_unwind(p),
    }
}

/// Runs the main loop for `d`.
fn pump(d: Duration) {
    let ctx = glib::MainContext::default();
    let done = Rc::new(Cell::new(false));
    let flag = done.clone();
    glib::timeout_add_local_once(d, move || flag.set(true));
    while !done.get() {
        ctx.iteration(true);
    }
}

/// A scratch folder that is removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        // /var/tmp: the hostile library is too large for a tmpfs /tmp.
        let base = Path::new("/var/tmp");
        let base = if base.is_dir() {
            base.to_owned()
        } else {
            std::env::temp_dir()
        };
        let dir = base.join(format!("tunemesh-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The app's window on `library`, presented; the library still loads.
fn open(library: &Path) -> Rc<App> {
    let gtk_app = adw::Application::builder()
        .application_id("io.github.brainage04.TuneMesh.Test")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    gtk_app
        .register(None::<&gio::Cancellable>)
        .expect("register application");
    let config = Config {
        library_path: library.to_owned(),
        downloader: false,
        setup_done: true,
    };
    let app = App::new(&gtk_app, config);
    app.present();
    app
}

/// Lets the library scan, background loads and redraws finish.
fn settle(app: &App) {
    app.store.settle_for(Duration::from_secs(600));
    pump(Duration::from_millis(300));
    app.store.settle_for(Duration::from_secs(600));
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Widget,
    Model,
    Item,
}

/// An object seen in the window, held weakly.
struct Seen {
    what: String,
    obj: glib::WeakRef<glib::Object>,
    kind: Kind,
}

impl Seen {
    fn new(obj: &impl IsA<glib::Object>, kind: Kind) -> Self {
        Self {
            what: obj.type_().name().to_owned(),
            obj: obj.upcast_ref::<glib::Object>().downgrade(),
            kind,
        }
    }
}

/// `root`, every widget under it, the list models of list and grid views
/// and their first items.
fn collect(root: &gtk::Widget, out: &mut Vec<Seen>) {
    out.push(Seen::new(root, Kind::Widget));
    let model: Option<gio::ListModel> = if let Some(v) = root.downcast_ref::<gtk::ListView>() {
        v.model().map(|m| m.upcast())
    } else if let Some(v) = root.downcast_ref::<gtk::GridView>() {
        v.model().map(|m| m.upcast())
    } else if let Some(v) = root.downcast_ref::<gtk::ColumnView>() {
        v.model().map(|m| m.upcast())
    } else if let Some(v) = root.downcast_ref::<gtk::DropDown>() {
        v.model()
    } else {
        None
    };
    if let Some(m) = model {
        collect_model(&m, out);
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        collect(&c, out);
        child = c.next_sibling();
    }
}

fn collect_model(model: &gio::ListModel, out: &mut Vec<Seen>) {
    out.push(Seen::new(model, Kind::Model));
    let inner: Option<gio::ListModel> = if let Some(m) = model.downcast_ref::<gtk::NoSelection>() {
        m.model()
    } else if let Some(m) = model.downcast_ref::<gtk::SingleSelection>() {
        m.model()
    } else if let Some(m) = model.downcast_ref::<gtk::MultiSelection>() {
        m.model()
    } else if let Some(m) = model.downcast_ref::<gtk::FilterListModel>() {
        m.model()
    } else if let Some(m) = model.downcast_ref::<gtk::SortListModel>() {
        m.model()
    } else if let Some(m) = model.downcast_ref::<gtk::SliceListModel>() {
        m.model()
    } else {
        None
    };
    match inner {
        Some(inner) => collect_model(&inner, out),
        None => {
            for i in 0..model.n_items().min(64) {
                if let Some(item) = model.item(i) {
                    out.push(Seen::new(&item, Kind::Item));
                }
            }
        }
    }
}

/// What must be gone.
#[derive(Clone, Copy)]
enum Expect {
    /// Everything: a closed page or dialog.
    AllFreed,
    /// A panel that stays: widgets no longer in a window, and the items its
    /// models showed before they were replaced.
    Replaced,
    /// The whole window: widgets no longer in a window.
    Detached,
}

/// What is still alive that should not be, counted by type.
fn leaked(seen: &[Seen], expect: Expect) -> Vec<(String, usize)> {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for s in seen {
        let Some(obj) = s.obj.upgrade() else { continue };
        let in_window = || {
            obj.downcast_ref::<gtk::Widget>()
                .is_some_and(|w| w.root().is_some())
        };
        let leak = match (expect, s.kind) {
            (Expect::AllFreed, _) => true,
            (Expect::Replaced, Kind::Widget) | (Expect::Detached, Kind::Widget) => !in_window(),
            (Expect::Replaced, Kind::Item) => true,
            (Expect::Replaced, Kind::Model) | (Expect::Detached, _) => false,
        };
        if !leak {
            continue;
        }
        match counts.iter_mut().find(|(w, _)| *w == s.what) {
            Some((_, n)) => *n += 1,
            None => counts.push((s.what.clone(), 1)),
        }
    }
    counts.sort();
    counts
}
