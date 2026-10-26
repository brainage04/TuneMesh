//! The stall walks: every page and action, three times, on a typical
//! library (`typical`) and on one at the contract's limits
//! (`tunemesh_core::hostile::Options::full`), recording the longest
//! main-loop stall each causes. A release build must stay under the budget
//! (`TUNEMESH_STALL_BUDGET_MS`, 16 ms by default: one frame at 60 Hz) in
//! all but one round of each action.
//!
//! A timeout at high priority ticks every 2 ms; a stall is the longest gap
//! between two ticks while an action and everything it scheduled run
//! (layout, drawing, background results arriving, the heap trim two seconds
//! after a page change). Nothing the main loop runs can interrupt it, so
//! input waits as long. Each stall is shown with the GTK thread's CPU time
//! in it: the rest is time the thread waited (for the disk, the GPU driver
//! or a CPU).
//!
//! Start-up until the first frame is drawn is reported apart and not held
//! to the budget: creating the window loads the GPU driver (EGL or Vulkan
//! vendors) and the first frame compiles its shaders, both in the
//! platform's code. A single late round of an action is likewise allowed:
//! the first time a page draws a new kind of node the driver compiles a
//! shader for it, and another process can take the CPU.
//!
//! `TUNEMESH_TYPICAL_LIBRARY=<dir>` and `TUNEMESH_HOSTILE_LIBRARY=<dir>`
//! reuse a generated library (the hostile one: `cargo run --release -p
//! tunemesh-core --features hostile --example hostile_library -- <dir>`);
//! otherwise one is generated under /var/tmp and removed afterwards.
//! `TUNEMESH_STALL_ROUNDS` changes the number of rounds,
//! `TUNEMESH_STALL_REPORT=<file>.md` also writes the tables to
//! `<file>-typical.md` and `<file>-hostile.md`, and `TUNEMESH_STALL_PROFILE`
//! with `TUNEMESH_PERF_CTL` records chosen actions with perf (see `perf`).

use std::cell::Cell;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::glib;
use tunemesh_core::LIKED_SONGS;
use tunemesh_core::hostile::{self, Options};

use super::{Scratch, on_gtk, open, pump, settle, typical};
use crate::app::{App, Route};
use crate::store::wait_for;

const TICK: Duration = Duration::from_millis(2);
const ROUNDS: usize = 3;

/// One walk at a time: each owns the machine while it measures.
static WALK: Mutex<()> = Mutex::new(());

/// CPU time the calling thread has used.
fn thread_cpu() -> Duration {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: valid out-pointer.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
}

/// A stall: how long the main loop did not run, and the GTK thread's CPU
/// time in that gap.
#[derive(Clone, Copy, Default)]
struct Stall {
    wall: Duration,
    cpu: Duration,
}

/// The main-loop probe.
struct Probe {
    last: Cell<(Instant, Duration)>,
    worst: Cell<Stall>,
}

impl Probe {
    fn start() -> Rc<Self> {
        let probe = Rc::new(Self {
            last: Cell::new((Instant::now(), thread_cpu())),
            worst: Cell::new(Stall::default()),
        });
        let p = probe.clone();
        glib::timeout_add_local_full(TICK, glib::Priority::HIGH, move || {
            p.tick();
            glib::ControlFlow::Continue
        });
        probe
    }

    fn tick(&self) {
        let now = (Instant::now(), thread_cpu());
        let (t, c) = self.last.replace(now);
        let gap = Stall {
            wall: now.0 - t,
            cpu: now.1.saturating_sub(c),
        };
        if gap.wall > self.worst.get().wall {
            self.worst.set(gap);
        }
    }

    fn reset(&self) {
        self.last.set((Instant::now(), thread_cpu()));
        self.worst.set(Stall::default());
    }

    /// The longest stall since `reset`, less the tick interval.
    fn stall(&self) -> Stall {
        self.tick();
        let s = self.worst.get();
        Stall {
            wall: s.wall.saturating_sub(TICK),
            cpu: s.cpu,
        }
    }
}

/// Bytes in use on the heap, after returning free pages.
fn heap_in_use() -> usize {
    // SAFETY: plain libc queries.
    unsafe {
        libc::malloc_trim(0);
        libc::mallinfo2().uordblks
    }
}

/// The first scrolled window in the page, scrolled to its end.
fn scroll_to_end(app: &App) {
    fn find(w: &gtk::Widget) -> Option<gtk::ScrolledWindow> {
        if let Some(s) = w.downcast_ref::<gtk::ScrolledWindow>() {
            return Some(s.clone());
        }
        let mut child = w.first_child();
        while let Some(c) = child {
            if let Some(s) = find(&c) {
                return Some(s);
            }
            child = c.next_sibling();
        }
        None
    }
    let page = app.page_widget().expect("page");
    let s = find(&page).expect("scrolled page");
    let adj = s.vadjustment();
    adj.set_value(adj.upper() - adj.page_size());
}

/// The library a walk runs on.
struct Shape {
    name: &'static str,
    /// The album page visited: its artist, name and tracks.
    album_artist: String,
    album_name: String,
    album_tracks: Vec<String>,
    /// The playlist page visited.
    playlist: String,
    playlist_len: usize,
    /// What "play" queues.
    play: Vec<String>,
}

type Action = Box<dyn Fn(&Rc<App>, usize)>;

fn count(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The actions of a walk; `album` is the album page's key.
fn actions(s: &Shape, album: String) -> Vec<(String, Action)> {
    let go = |r: Route| -> Action { Box::new(move |app, _| app.navigate(r.clone())) };
    let scroll = || -> Action { Box::new(|a, _| scroll_to_end(a)) };
    let play = Rc::new(s.play.clone());
    let (liked_track, play2) = (s.album_tracks[3].clone(), play.clone());
    let (artist, album_name) = (s.album_artist.clone(), s.album_name.clone());
    vec![
        ("home".into(), go(Route::Home)),
        ("albums".into(), go(Route::Albums)),
        ("albums, scrolled to the end".into(), scroll()),
        ("artists".into(), go(Route::Artists)),
        ("artists, scrolled to the end".into(), scroll()),
        (
            format!("album of {} tracks", count(s.album_tracks.len())),
            go(Route::Album(album)),
        ),
        ("album, scrolled to the end".into(), scroll()),
        (
            "like a track on the album page".into(),
            Box::new(move |a, _| {
                wait_for(|done| a.store.toggle_like(&liked_track, done)).expect("like");
            }),
        ),
        (
            "a synced file arrives (album page)".into(),
            Box::new(move |a, round| {
                let lib = a.store.root();
                let file = lib.join(format!("{artist}/{album_name}/new {round}.mp3"));
                let bytes = hostile::mp3(&hostile::Tags {
                    title: "Arrived",
                    artists: &[&artist],
                    album: &album_name,
                    track: Some(1),
                    cover: None,
                });
                std::fs::write(file, bytes).expect("write file");
                // The watcher reports within a second; the rescan follows.
                pump(Duration::from_millis(1500));
            }),
        ),
        (
            "artist page of that album".into(),
            go(Route::Artist(s.album_artist.clone())),
        ),
        ("playlists".into(), go(Route::Playlists)),
        ("playlists, scrolled to the end".into(), scroll()),
        (
            format!("playlist of {} entries", count(s.playlist_len)),
            go(Route::Playlist(s.playlist.clone())),
        ),
        ("playlist, scrolled to the end".into(), scroll()),
        (
            "Liked Songs".into(),
            go(Route::Playlist(LIKED_SONGS.into())),
        ),
        ("downloads".into(), go(Route::Downloads)),
        ("settings".into(), go(Route::Settings)),
        (
            "search typed: \"song 1\"".into(),
            Box::new(|a, _| {
                let text = "song 1";
                for end in 1..=text.len() {
                    a.search_entry.set_text(&text[..end]);
                    pump(Duration::from_millis(80));
                }
            }),
        ),
        ("search \"a\"".into(), go(Route::Search("a".into()))),
        (
            format!("play {} tracks", count(s.play.len())),
            Box::new(move |a, _| {
                a.player
                    .play_tracks("Stall walk", play.to_vec(), 0, Some(false));
                a.player.pause();
            }),
        ),
        (
            format!("queue panel opened ({} entries)", count(s.play.len())),
            Box::new(|a, _| {
                if !a.queue_shown() {
                    a.toggle_queue();
                }
            }),
        ),
        (
            "next track, queue open".into(),
            Box::new(|a, round| {
                a.player.jump_to(round);
                a.player.pause();
            }),
        ),
        (
            "queue panel closed".into(),
            Box::new(|a, _| {
                if a.queue_shown() {
                    a.toggle_queue();
                }
            }),
        ),
        (
            "new playlist".into(),
            Box::new(|a, round| {
                wait_for(|done| a.store.create_playlist(&format!("Stall {round}"), done))
                    .expect("create playlist");
            }),
        ),
        (
            format!("{} tracks added to it", count(play2.len())),
            Box::new(move |a, round| {
                wait_for(|done| {
                    a.store
                        .add_to_playlist(&format!("Stall {round}"), &play2, done)
                })
                .expect("add to playlist");
            }),
        ),
        (
            "playlist deleted".into(),
            Box::new(|a, round| {
                wait_for(|done| a.store.delete_playlist(&format!("Stall {round}"), done))
                    .expect("delete playlist");
            }),
        ),
        (
            "setup assistant opened".into(),
            Box::new(|a, _| crate::ui::setup::open(a)),
        ),
        (
            "setup assistant closed".into(),
            Box::new(|a, _| {
                if let Some(d) = a.window.visible_dialog() {
                    d.force_close();
                }
            }),
        ),
        ("back".into(), Box::new(|a, _| a.back())),
        ("forward".into(), Box::new(|a, _| a.forward())),
    ]
}

/// Whether a row counts against the budget.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Start-up until the first frame: the GPU driver and shader compiles.
    Platform,
    App,
}

struct Walk {
    /// Per action: its stall in each round.
    rows: Vec<(String, Kind, Vec<Stall>)>,
    /// Heap in use after each round, in bytes.
    heap: Vec<usize>,
}

/// Writes `cmd` to perf's control FIFO (`perf record --control
/// fifo:<ctl>,<ack> --delay=-1`), if `TUNEMESH_PERF_CTL=<ctl>,<ack>` is set,
/// and waits for perf to acknowledge.
fn perf(cmd: &str) {
    let Some(spec) = std::env::var_os("TUNEMESH_PERF_CTL") else {
        return;
    };
    let spec = spec.to_string_lossy().into_owned();
    let (ctl, ack) = spec.split_once(',').expect("TUNEMESH_PERF_CTL=<ctl>,<ack>");
    std::fs::write(ctl, format!("{cmd}\n")).expect("perf control FIFO");
    // perf answers "ack" and keeps its end open: read once, not to the end.
    let mut buf = [0u8; 16];
    let _ = std::io::Read::read(
        &mut std::fs::File::open(ack).expect("perf ack FIFO"),
        &mut buf,
    );
}

/// Runs the main loop until the window has drawn a frame.
fn first_frame(app: &App) {
    let drawn = Rc::new(Cell::new(false));
    let d = drawn.clone();
    let clock = app.window.frame_clock().expect("a realized window");
    let id = clock.connect_after_paint(move |_| d.set(true));
    let ctx = glib::MainContext::default();
    let start = Instant::now();
    while !drawn.get() && start.elapsed() < Duration::from_secs(60) {
        ctx.iteration(true);
    }
    clock.disconnect(id);
}

fn walk(library: PathBuf, shape: Shape) -> Walk {
    let rounds: usize = std::env::var("TUNEMESH_STALL_ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(ROUNDS);
    // Actions to profile (substrings of their names, comma-separated).
    let profile: Vec<String> = std::env::var("TUNEMESH_STALL_PROFILE")
        .map(|v| v.split(',').map(str::to_owned).collect())
        .unwrap_or_default();
    let probe = Probe::start();
    let mut rows = Vec::new();

    let profiled = profile.iter().any(|p| "start-up".contains(p.as_str()));
    if profiled {
        perf("enable");
    }
    // Creating the window, before the main loop runs again.
    let start = Instant::now();
    let cpu = thread_cpu();
    let app = open(&library);
    let created = Stall {
        wall: start.elapsed(),
        cpu: thread_cpu() - cpu,
    };
    probe.reset();
    first_frame(&app);
    let first = probe.stall();
    probe.reset();
    settle(&app);
    pump(Duration::from_millis(2500));
    if profiled {
        perf("disable");
    }
    let lib = app.store.library();
    rows.push((
        "start-up: creating the window".to_owned(),
        Kind::Platform,
        vec![created],
    ));
    rows.push((
        "start-up: the first frame".to_owned(),
        Kind::Platform,
        vec![first],
    ));
    rows.push((
        format!(
            "start-up: loading {} tracks, {} albums, {} artists, {} playlists",
            count(lib.tracks.len()),
            count(lib.albums.len()),
            count(lib.artists.len()),
            count(app.store.playlists().len())
        ),
        Kind::App,
        vec![probe.stall()],
    ));
    let album = lib
        .albums
        .iter()
        .find(|a| a.name == shape.album_name && a.artist == shape.album_artist)
        .expect("the walk's album")
        .key();
    drop(lib);

    let actions = actions(&shape, album);
    let first = rows.len();
    for (name, _) in &actions {
        rows.push((name.clone(), Kind::App, Vec::new()));
    }
    let mut heap = Vec::new();
    for round in 1..=rounds {
        for (i, (name, action)) in actions.iter().enumerate() {
            let profiled = profile.iter().any(|p| name.contains(p.as_str()));
            if profiled {
                perf("enable");
            }
            probe.reset();
            action(&app, round);
            settle(&app);
            // Long enough to include the heap trim after a page change.
            pump(Duration::from_millis(2500));
            let stall = probe.stall();
            if profiled {
                perf("disable");
            }
            eprintln!(
                "{}, round {round}: {name}: {:.0} ms ({:.0} ms on CPU)",
                shape.name,
                stall.wall.as_secs_f64() * 1000.0,
                stall.cpu.as_secs_f64() * 1000.0
            );
            rows[first + i].2.push(stall);
        }
        heap.push(heap_in_use());
    }
    drop(app);
    pump(Duration::from_millis(500));
    Walk { rows, heap }
}

fn table(w: &Walk, budget: Duration) -> String {
    let ms = |d: Duration| format!("{:.0}", d.as_secs_f64() * 1000.0);
    let cell = |s: &Stall| format!("{} ({})", ms(s.wall), ms(s.cpu));
    let mut out = String::from("| Action | Worst stall per round, ms (on CPU) |\n| --- | --- |\n");
    for (name, kind, stalls) in &w.rows {
        let cells: Vec<String> = stalls.iter().map(cell).collect();
        let mark = if *kind == Kind::Platform {
            " (platform, not budgeted)"
        } else if stalls.iter().any(|s| s.wall > budget) {
            " **over**"
        } else {
            ""
        };
        let _ = writeln!(out, "| {name}{mark} | {} |", cells.join(" / "));
    }
    let heap: Vec<String> = w
        .heap
        .iter()
        .map(|b| format!("{:.0}", *b as f64 / 1_048_576.0))
        .collect();
    let _ = writeln!(
        out,
        "\nHeap in use after each round (MiB): {}",
        heap.join(" / ")
    );
    out
}

fn run(name: &'static str, library: PathBuf, shape: impl FnOnce(&Path) -> Shape + Send + 'static) {
    if cfg!(debug_assertions) {
        panic!("stall budgets apply to release builds: cargo test --release");
    }
    let _walk = WALK.lock().unwrap_or_else(|e| e.into_inner());
    let budget = Duration::from_millis(
        std::env::var("TUNEMESH_STALL_BUDGET_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(16),
    );
    let w = on_gtk(move || {
        let shape = shape(&library);
        walk(library, shape)
    });
    let table = table(&w, budget);
    println!("{name}:\n\n{table}");
    if let Some(path) = std::env::var_os("TUNEMESH_STALL_REPORT") {
        let path = PathBuf::from(path);
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let file = path.with_file_name(format!("{stem}-{name}.md"));
        std::fs::write(file, &table).expect("write report");
    }
    // An action fails when it went over the budget in more than one round
    // (or in its only one): a single late round is the machine (another
    // process, an efficiency core) or a first shader compile, a repeated
    // one is the app.
    let over: Vec<String> = w
        .rows
        .iter()
        .filter(|(_, kind, s)| {
            let n = s.iter().filter(|d| d.wall > budget).count();
            *kind == Kind::App && (n > 1 || (n == 1 && s.len() == 1))
        })
        .map(|(n, _, _)| n.clone())
        .collect();
    assert!(
        over.is_empty(),
        "{name}: main-loop stalls over {budget:?}: {}",
        over.join(", ")
    );
}

/// The library `var` names (generated there first if it does not exist), or
/// one `generate` writes under /var/tmp and removes afterwards.
fn library(
    var: &str,
    name: &str,
    generate: impl FnOnce(&Path) -> std::io::Result<()>,
) -> (PathBuf, Option<Scratch>) {
    let (dir, scratch) = match std::env::var_os(var) {
        Some(dir) => (PathBuf::from(dir), None),
        None => {
            let s = Scratch::new(name);
            (s.0.clone(), Some(s))
        }
    };
    if !dir.exists() {
        let start = Instant::now();
        generate(&dir).expect("generate library");
        eprintln!("generated the {name} library in {:.0?}", start.elapsed());
    }
    (dir, scratch)
}

#[test]
#[ignore = "release build with a display, about 5 minutes; run by scripts/gui-checks.sh"]
fn typical_walk_stays_responsive() {
    let (library, _scratch) = library("TUNEMESH_TYPICAL_LIBRARY", "typical", typical::generate);
    run("typical", library, |_| {
        let album = 0;
        let album_tracks: Vec<String> = (1..=typical::album_len(album))
            .map(|t| typical::track_path(album, t))
            .collect();
        let artist = typical::artist(album % typical::ARTISTS);
        let all = typical::all_tracks();
        Shape {
            name: "typical",
            album_artist: artist,
            album_name: typical::album(album),
            album_tracks,
            playlist: typical::playlist_name(typical::PLAYLISTS - 1),
            playlist_len: typical::BIG_PLAYLIST_ENTRIES,
            play: all[..typical::BIG_PLAYLIST_ENTRIES].to_vec(),
        }
    });
}

#[test]
#[ignore = "release build with a display, about 10 minutes; run by scripts/gui-checks.sh"]
fn hostile_walk_stays_responsive() {
    let (library, _scratch) = library("TUNEMESH_HOSTILE_LIBRARY", "hostile", |root| {
        hostile::generate(root, &Options::full())
    });
    run("hostile", library, |_| {
        let album_tracks: Vec<String> = (0..Options::full().album_tracks)
            .map(hostile::album_track)
            .collect();
        Shape {
            name: "hostile",
            album_artist: "Hostile Artist".into(),
            album_name: "One Album".into(),
            play: album_tracks.clone(),
            album_tracks,
            playlist: "Big 00".into(),
            playlist_len: tunemesh_core::limits::MAX_PLAYLIST_ENTRIES,
        }
    });
}
