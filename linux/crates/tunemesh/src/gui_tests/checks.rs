//! What the user sees after the round-3 review fixes, in the real window:
//!
//! - closing the window or quitting writes playlist edits still queued on
//!   the writer thread (R3-Opus-5);
//! - closing the window while the downloader runs counts the waiting
//!   requests off the main loop, by file name (R3-Opus-4);
//! - a playlist reload that read the files before an edit never takes the
//!   edit's result off the screen (R3-Opus-6);
//! - the Downloads page keeps little of 200 request and 200 status files
//!   at their limits (R3-Opus-3).

use std::path::Path;
use std::time::{Duration, Instant};

use adw::prelude::*;
use tunemesh_core::playlist::Playlist;
use tunemesh_core::{LIKED_SONGS, PLAYLISTS_DIR};

use super::{Scratch, on_gtk, open, pump, settle};
use crate::app::{App, Route};
use crate::store::wait_for;

/// A library with playlist `P` (one song) and a Liked Songs of 200,000
/// entries, whose rewrite takes long enough to be still queued at exit.
fn playlists_library(root: &Path) {
    let dir = root.join(PLAYLISTS_DIR);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("P.m3u8"), "#EXTM3U\n../A.opus\n").unwrap();
    let liked: String = (0..200_000).map(|i| format!("../L/{i}.opus\n")).collect();
    std::fs::write(
        dir.join(format!("{LIKED_SONGS}.m3u8")),
        format!("#EXTM3U\n{liked}"),
    )
    .unwrap();
}

/// Checks that `P` holds the added song, Liked Songs the liked one, and
/// that no temporary file is left in `Playlists/`.
fn edits_written(root: &Path, how: &str) {
    let p = Playlist::read(root, "P").unwrap();
    assert!(
        p.entries.iter().any(|e| e.track() == Some("B.opus")),
        "{how}: the queued add was lost"
    );
    let liked = Playlist::read(root, LIKED_SONGS).unwrap();
    assert!(
        liked.entries.iter().any(|e| e.track() == Some("New.opus")),
        "{how}: the queued like was lost"
    );
    let temps: Vec<String> = std::fs::read_dir(root.join(PLAYLISTS_DIR))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(".~tunemesh-"))
        .collect();
    assert!(temps.is_empty(), "{how}: left behind {temps:?}");
}

fn queue_edits(app: &App) {
    app.store
        .add_to_playlist("P", &["B.opus".into()], |r| r.unwrap());
    app.store.toggle_like("New.opus", |r| r.unwrap());
}

#[test]
#[ignore = "needs a display; run by scripts/gui-checks.sh"]
fn closing_or_quitting_writes_queued_playlist_edits() {
    on_gtk(|| {
        for how in ["closing the window", "quitting"] {
            let dir = Scratch::new("flush-check");
            playlists_library(&dir.0);
            let app = open(&dir.0);
            settle(&app);
            assert_eq!(app.store.liked_count(), 200_000);
            queue_edits(&app);
            if how == "quitting" {
                app.quit();
            } else {
                app.window.close();
            }
            // The main loop does not run again after either.
            edits_written(&dir.0, how);
            drop(app);
        }
    });
}

/// Request files of 1 MiB each (an `input` far over its limit): what a
/// sharing device can put in `requests/`.
fn large_requests(root: &Path, n: usize) {
    let dir = tunemesh_core::request::requests_dir(root);
    std::fs::create_dir_all(&dir).unwrap();
    let input = "x".repeat(1024 * 1024 - 100);
    for i in 0..n {
        let id = format!("{}-{i:08x}", 1_700_000_000_000u64 + i as u64);
        std::fs::write(
            dir.join(format!("{id}.json")),
            format!(r#"{{"version":1,"id":"{id}","input":"{input}"}}"#),
        )
        .unwrap();
    }
}

#[test]
#[ignore = "needs a display; run by scripts/gui-checks.sh"]
fn closing_counts_waiting_requests_off_the_main_loop() {
    on_gtk(|| {
        let dir = Scratch::new("pending-check");
        std::fs::create_dir_all(&dir.0).unwrap();
        large_requests(&dir.0, 400);
        let app = open(&dir.0);
        settle(&app);
        app.downloader.pretend_running();

        let start = Instant::now();
        app.window.close();
        let took = start.elapsed();
        // Reading the 400 MiB of requests took 301 ms on the main loop.
        assert!(
            took < Duration::from_millis(50),
            "the close request held the main loop for {took:?}"
        );
        let start = Instant::now();
        while app.window.visible_dialog().is_none() && start.elapsed() < Duration::from_secs(10) {
            pump(Duration::from_millis(20));
        }
        let dialog = app
            .window
            .visible_dialog()
            .expect("the keep-downloading question");
        let body = dialog
            .downcast_ref::<adw::AlertDialog>()
            .expect("an alert")
            .body();
        assert!(body.starts_with("400 downloads are left"), "{body}");
        assert!(app.window.is_visible(), "the window stays open meanwhile");
        dialog.force_close();
        drop(dialog);
        pump(Duration::from_millis(100));
        drop(app);
    });
}

/// Whether a label under `w` shows exactly `text`.
fn shows(w: &gtk::Widget, text: &str) -> bool {
    if w.downcast_ref::<gtk::Label>()
        .is_some_and(|l| l.label() == text)
    {
        return true;
    }
    let mut child = w.first_child();
    while let Some(c) = child {
        if shows(&c, text) {
            return true;
        }
        child = c.next_sibling();
    }
    false
}

#[test]
#[ignore = "needs a display; run by scripts/gui-checks.sh"]
fn a_reload_older_than_an_edit_keeps_the_edit_shown() {
    on_gtk(|| {
        let dir = Scratch::new("reload-check");
        let lists = dir.0.join(PLAYLISTS_DIR);
        std::fs::create_dir_all(&lists).unwrap();
        std::fs::write(lists.join("P.m3u8"), "#EXTM3U\n../A.opus\n").unwrap();
        let app = open(&dir.0);
        settle(&app);
        app.navigate(Route::Playlists);
        settle(&app);
        // Only this check's reload runs.
        app.store.stop_watching();

        let (read, go) = app.store.start_paused_reload();
        read.recv().unwrap();
        let fresh = wait_for(|done| app.store.create_playlist("Fresh", done)).unwrap();
        pump(Duration::from_millis(200));
        assert!(shows(app.window.upcast_ref(), &fresh), "created, shown");
        go.send(()).unwrap();
        settle(&app);
        pump(Duration::from_millis(200));
        assert!(
            shows(app.window.upcast_ref(), &fresh),
            "the reload that read the folder before \"{fresh}\" was created took it off the screen"
        );
        drop(app);
    });
}

/// Bytes in use on the heap, after returning free pages.
fn heap_in_use() -> usize {
    // SAFETY: plain libc queries.
    unsafe {
        libc::malloc_trim(0);
        libc::mallinfo2().uordblks
    }
}

/// The Downloads page held 397 MiB with 200 crafted requests and 200
/// crafted statuses.
const DOWNLOADS_PAGE_BUDGET: usize = 48 << 20;

#[test]
#[ignore = "needs a display; run by scripts/gui-checks.sh"]
fn the_downloads_page_keeps_little_of_large_files() {
    on_gtk(|| {
        use tunemesh_core::limits::MAX_LISTED_REQUESTS;
        let dir = Scratch::new("downloads-memory-check");
        std::fs::create_dir_all(&dir.0).unwrap();
        tunemesh_core::hostile::max_requests(&dir.0, MAX_LISTED_REQUESTS).unwrap();
        tunemesh_core::hostile::max_statuses(&dir.0, MAX_LISTED_REQUESTS).unwrap();
        let app = open(&dir.0);
        settle(&app);
        app.navigate(Route::Home);
        settle(&app);
        pump(Duration::from_millis(500));
        let before = heap_in_use();

        app.navigate(Route::Downloads);
        settle(&app);
        // Rows are built a few per frame.
        pump(Duration::from_secs(3));
        let page = app.page_widget().expect("page");
        assert!(shows(&page, "Requests"), "the Downloads page is shown");
        drop(page);
        let grew = heap_in_use().saturating_sub(before);
        eprintln!(
            "Downloads page with {MAX_LISTED_REQUESTS} + {MAX_LISTED_REQUESTS} files at their limits: {} KiB more heap",
            grew >> 10
        );
        assert!(
            grew <= DOWNLOADS_PAGE_BUDGET,
            "the Downloads page holds {} MiB",
            grew >> 20
        );
        app.navigate(Route::Home);
        settle(&app);
        drop(app);
    });
}
