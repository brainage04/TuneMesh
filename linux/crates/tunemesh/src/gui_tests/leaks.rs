//! Every page, the queue panel, the now-playing bar and the setup assistant
//! free their widgets, models and items once they are closed or replaced.
//! Before d79cca2 four reference cycles kept every visited page alive; a
//! page that had the keyboard focus, the Downloads and Settings forms and
//! the setup assistant still leaked after it.

use std::time::Duration;

use adw::prelude::*;
use tunemesh_core::LIKED_SONGS;
use tunemesh_core::hostile::{self, Options};

use super::{Expect, Scratch, Seen, collect, leaked, on_gtk, open, pump, settle};
use crate::app::{App, Route};
use crate::store::wait_for;

/// Visits per page.
const ROUNDS: usize = 3;

/// A small library of the hostile kind: one album of 300 tracks, albums of
/// ten, download requests and status files full of warnings.
fn library(root: &std::path::Path) {
    let options = Options {
        album_tracks: 300,
        files: 800,
        statuses: 3,
        requests: 30,
        big_playlists: 0,
        small_playlists: 5,
        huge: false,
    };
    hostile::generate(root, &options).expect("generate library");
}

/// Lets anything a closed widget scheduled (animations, idle callbacks,
/// background loads) finish.
fn quiesce(app: &App) {
    settle(app);
    pump(Duration::from_millis(400));
}

fn report(failures: &mut Vec<String>, what: &str, seen: &[Seen], expect: Expect) {
    let left = leaked(seen, expect);
    if !left.is_empty() {
        let list: Vec<String> = left.iter().map(|(t, n)| format!("{n} {t}")).collect();
        failures.push(format!("{what}: {}", list.join(", ")));
    }
}

fn walk() -> Vec<String> {
    let dir = Scratch::new("leak-test");
    library(&dir.0);
    let app = open(&dir.0);
    settle(&app);

    let lib = app.store.library();
    let album = lib
        .albums
        .iter()
        .find(|a| a.name == "One Album")
        .expect("big album")
        .key();
    let album_paths: Vec<String> = (0..300).map(hostile::album_track).collect();
    wait_for(|done| app.store.set_liked_paths(&album_paths[..20], true, done)).expect("like");
    let mixed = wait_for(|done| app.store.create_playlist("Mixed", done)).expect("playlist");
    wait_for(|done| app.store.add_to_playlist(&mixed, &album_paths, done))
        .expect("add to playlist");
    settle(&app);

    let routes = [
        Route::Home,
        Route::Search("song 1".into()),
        Route::Albums,
        Route::Artists,
        Route::Album(album),
        Route::Artist("Hostile Artist".into()),
        Route::Playlists,
        Route::Playlist(mixed),
        Route::Playlist(LIKED_SONGS.into()),
        Route::Downloads,
        Route::Settings,
    ];
    let away = Route::Search(String::new());

    let mut failures = Vec::new();
    let mut window = Vec::new();
    for round in 1..=ROUNDS {
        collect(app.window.upcast_ref(), &mut window);

        for route in &routes {
            app.navigate(route.clone());
            quiesce(&app);
            let page = app.page_widget().expect("page");
            // As after a click on the page: the keyboard focus is inside it.
            page.child_focus(gtk::DirectionType::TabForward);
            let mut seen = Vec::new();
            collect(&page, &mut seen);
            drop(page);
            app.navigate(away.clone());
            quiesce(&app);
            report(
                &mut failures,
                &format!("round {round}, page {route:?}"),
                &seen,
                Expect::AllFreed,
            );
        }

        // The queue panel: a long queue, then a short one; rows the list
        // let go of and the long queue's items must be freed.
        app.player
            .play_tracks("Leak test", album_paths.clone(), 0, Some(false));
        app.player.pause();
        if !app.queue_shown() {
            app.toggle_queue();
        }
        quiesce(&app);
        let mut seen = Vec::new();
        collect(&app.queue_widget(), &mut seen);
        app.player
            .play_tracks("Leak test 2", album_paths[..2].to_vec(), 0, Some(false));
        app.player.pause();
        quiesce(&app);
        app.toggle_queue();
        quiesce(&app);
        report(
            &mut failures,
            &format!("round {round}, queue panel"),
            &seen,
            Expect::Replaced,
        );

        // The now-playing bar follows the current track.
        for i in [1, 0, 1] {
            app.player.jump_to(i);
            app.player.pause();
            quiesce(&app);
        }

        // The setup assistant.
        crate::ui::setup::open(&app);
        quiesce(&app);
        let dialog = app.window.visible_dialog().expect("setup dialog");
        let mut seen = Vec::new();
        collect(dialog.upcast_ref(), &mut seen);
        dialog.force_close();
        drop(dialog);
        quiesce(&app);
        report(
            &mut failures,
            &format!("round {round}, setup assistant"),
            &seen,
            Expect::AllFreed,
        );
    }
    collect(app.window.upcast_ref(), &mut window);
    app.navigate(away);
    quiesce(&app);
    report(
        &mut failures,
        "whole window (sidebar, queue, now-playing bar)",
        &window,
        Expect::Detached,
    );
    failures
}

#[test]
#[ignore = "needs a display; run by scripts/gui-checks.sh"]
fn closed_pages_are_freed() {
    let failures = on_gtk(walk);
    assert!(
        failures.is_empty(),
        "still alive after closing:\n{}",
        failures.join("\n")
    );
}
