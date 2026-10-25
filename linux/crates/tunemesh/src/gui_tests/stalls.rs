//! The hostile-library walk: every page and action on a library at the
//! contract's limits (`tunemesh_core::hostile::Options::full`), recording
//! the longest main-loop stall each one causes. A release build must stay
//! under the budget (`TUNEMESH_STALL_BUDGET_MS`, 100 ms by default).
//!
//! A timeout at high priority ticks every 2 ms; a stall is the longest gap
//! between two ticks while an action and everything it scheduled run
//! (layout, drawing, background results arriving, the heap trim two seconds
//! after a page change). Nothing the main loop runs can interrupt it, so
//! input waits as long.
//!
//! `TUNEMESH_HOSTILE_LIBRARY=<dir>` reuses a generated library
//! (`cargo run --release -p tunemesh-core --features hostile --example
//! hostile_library -- <dir>`); otherwise one is generated under /var/tmp
//! and removed afterwards. `TUNEMESH_STALL_REPORT=<file>` also writes the
//! table there.

use std::cell::Cell;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::glib;
use tunemesh_core::LIKED_SONGS;
use tunemesh_core::hostile::{self, Options};

use super::{Scratch, on_gtk, open, pump, settle};
use crate::app::{App, Route};

const TICK: Duration = Duration::from_millis(2);
const ROUNDS: usize = 3;

/// The main-loop probe.
struct Probe {
    last: Cell<Instant>,
    worst: Cell<Duration>,
}

impl Probe {
    fn start() -> Rc<Self> {
        let probe = Rc::new(Self {
            last: Cell::new(Instant::now()),
            worst: Cell::new(Duration::ZERO),
        });
        let p = probe.clone();
        glib::timeout_add_local_full(TICK, glib::Priority::HIGH, move || {
            let now = Instant::now();
            let gap = now - p.last.replace(now);
            if gap > p.worst.get() {
                p.worst.set(gap);
            }
            glib::ControlFlow::Continue
        });
        probe
    }

    fn reset(&self) {
        self.last.set(Instant::now());
        self.worst.set(Duration::ZERO);
    }

    /// The longest stall since `reset`, less the tick interval.
    fn stall(&self) -> Duration {
        let open = self.last.get().elapsed();
        self.worst.get().max(open).saturating_sub(TICK)
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

type Action = Box<dyn Fn(&Rc<App>, usize)>;

fn actions(album: String) -> Vec<(&'static str, Action)> {
    let album_paths: Rc<Vec<String>> = Rc::new(
        (0..Options::full().album_tracks)
            .map(hostile::album_track)
            .collect(),
    );
    let go = |r: Route| -> Action { Box::new(move |app, _| app.navigate(r.clone())) };
    let paths = album_paths.clone();
    let paths2 = album_paths.clone();
    vec![
        ("home", go(Route::Home)),
        ("albums (10,001)", go(Route::Albums)),
        (
            "albums, scrolled to the end",
            Box::new(|a, _| scroll_to_end(a)),
        ),
        ("artists (601)", go(Route::Artists)),
        (
            "artists, scrolled to the end",
            Box::new(|a, _| scroll_to_end(a)),
        ),
        ("album of 40,000 tracks", go(Route::Album(album))),
        (
            "album, scrolled to the end",
            Box::new(|a, _| scroll_to_end(a)),
        ),
        (
            "like a track on the album page",
            Box::new(|a, _| {
                a.store.toggle_like(&hostile::album_track(3)).expect("like");
            }),
        ),
        (
            "a synced file arrives (album page)",
            Box::new(|a, round| {
                let lib = a.store.root();
                let file = lib.join(format!("Hostile Artist/One Album/new {round}.mp3"));
                let bytes = hostile::mp3(&hostile::Tags {
                    title: "Arrived",
                    artists: &["Hostile Artist"],
                    album: "One Album",
                    track: Some(1),
                    cover: None,
                });
                std::fs::write(file, bytes).expect("write file");
                // The watcher reports within a second; the rescan follows.
                pump(Duration::from_millis(1500));
            }),
        ),
        (
            "artist with the 40,000-track album",
            go(Route::Artist("Hostile Artist".into())),
        ),
        ("playlists (1,051)", go(Route::Playlists)),
        (
            "playlists, scrolled to the end",
            Box::new(|a, _| scroll_to_end(a)),
        ),
        (
            "playlist of 250,000 entries",
            go(Route::Playlist("Big 00".into())),
        ),
        (
            "playlist, scrolled to the end",
            Box::new(|a, _| scroll_to_end(a)),
        ),
        ("Liked Songs", go(Route::Playlist(LIKED_SONGS.into()))),
        (
            "downloads (600 requests, 500 1 MiB statuses)",
            go(Route::Downloads),
        ),
        ("settings", go(Route::Settings)),
        (
            "search typed: \"song 1\"",
            Box::new(|a, _| {
                let text = "song 1";
                for end in 1..=text.len() {
                    a.search_entry.set_text(&text[..end]);
                    pump(Duration::from_millis(80));
                }
            }),
        ),
        ("search \"a\"", go(Route::Search("a".into()))),
        (
            "play the 40,000-track album",
            Box::new(move |a, _| {
                a.player
                    .play_tracks("One Album", paths.to_vec(), 0, Some(false));
                a.player.pause();
            }),
        ),
        (
            "queue panel opened (40,000 entries)",
            Box::new(|a, _| {
                if !a.queue_shown() {
                    a.toggle_queue();
                }
            }),
        ),
        (
            "next track, queue open",
            Box::new(|a, round| {
                a.player.jump_to(round);
                a.player.pause();
            }),
        ),
        (
            "queue panel closed",
            Box::new(|a, _| {
                if a.queue_shown() {
                    a.toggle_queue();
                }
            }),
        ),
        (
            "new playlist",
            Box::new(|a, round| {
                a.store
                    .create_playlist(&format!("Stall {round}"))
                    .expect("create playlist");
            }),
        ),
        (
            "40,000 tracks added to it",
            Box::new(move |a, round| {
                a.store
                    .add_to_playlist(&format!("Stall {round}"), &paths2)
                    .expect("add to playlist");
            }),
        ),
        (
            "playlist deleted",
            Box::new(|a, round| {
                a.store
                    .delete_playlist(&format!("Stall {round}"))
                    .expect("delete playlist");
            }),
        ),
        (
            "setup assistant opened",
            Box::new(|a, _| crate::ui::setup::open(a)),
        ),
        (
            "setup assistant closed",
            Box::new(|a, _| {
                if let Some(d) = a.window.visible_dialog() {
                    d.force_close();
                }
            }),
        ),
        ("back", Box::new(|a, _| a.back())),
        ("forward", Box::new(|a, _| a.forward())),
    ]
}

struct Walk {
    /// Per action: its stall in each round.
    rows: Vec<(String, Vec<Duration>)>,
    /// Heap in use after each round, in bytes.
    heap: Vec<usize>,
}

fn walk(library: PathBuf) -> Walk {
    let probe = Probe::start();
    let mut rows = Vec::new();

    probe.reset();
    let app = open(&library);
    settle(&app);
    pump(Duration::from_millis(2500));
    rows.push((
        "start-up, scanning 100,000 files".to_owned(),
        vec![probe.stall()],
    ));

    let lib = app.store.library();
    let album = lib
        .albums
        .iter()
        .find(|a| a.name == "One Album")
        .expect("big album")
        .key();
    drop(lib);
    let actions = actions(album);
    for (name, _) in &actions {
        rows.push(((*name).to_owned(), Vec::new()));
    }
    let mut heap = Vec::new();
    for round in 1..=ROUNDS {
        for (i, (name, action)) in actions.iter().enumerate() {
            probe.reset();
            action(&app, round);
            settle(&app);
            // Long enough to include the heap trim after a page change.
            pump(Duration::from_millis(2500));
            let stall = probe.stall();
            eprintln!(
                "round {round}: {name}: {:.0} ms",
                stall.as_secs_f64() * 1000.0
            );
            rows[i + 1].1.push(stall);
        }
        heap.push(heap_in_use());
    }
    Walk { rows, heap }
}

fn table(w: &Walk) -> String {
    let ms = |d: &Duration| format!("{:.0}", d.as_secs_f64() * 1000.0);
    let mut out = String::from("| Action | Worst stall per round (ms) |\n| --- | --- |\n");
    for (name, stalls) in &w.rows {
        let cells: Vec<String> = stalls.iter().map(ms).collect();
        let _ = writeln!(out, "| {name} | {} |", cells.join(" / "));
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

#[test]
#[ignore = "release build with a display, about 10 minutes; run by scripts/gui-checks.sh"]
fn hostile_walk_stays_responsive() {
    if cfg!(debug_assertions) {
        panic!("stall budgets apply to release builds: cargo test --release");
    }
    let budget = Duration::from_millis(
        std::env::var("TUNEMESH_STALL_BUDGET_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100),
    );
    let (library, _scratch) = match std::env::var_os("TUNEMESH_HOSTILE_LIBRARY") {
        Some(dir) => (PathBuf::from(dir), None),
        None => {
            let s = Scratch::new("hostile");
            let start = Instant::now();
            hostile::generate(&s.0, &Options::full()).expect("generate library");
            eprintln!("generated the hostile library in {:.0?}", start.elapsed());
            (s.0.clone(), Some(s))
        }
    };
    let w = on_gtk(move || walk(library));
    let table = table(&w);
    println!("{table}");
    if let Some(path) = std::env::var_os("TUNEMESH_STALL_REPORT") {
        std::fs::write(path, &table).expect("write report");
    }
    let over: Vec<String> = w
        .rows
        .iter()
        .filter(|(_, s)| s.iter().any(|d| *d > budget))
        .map(|(n, _)| n.clone())
        .collect();
    assert!(
        over.is_empty(),
        "main-loop stalls over {budget:?}: {}",
        over.join(", ")
    );
}
