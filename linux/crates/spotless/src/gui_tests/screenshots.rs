//! The desktop screenshots in `docs/screenshots/`: not a check.
//! `scripts/screenshots.sh` generates the library, its download requests
//! and a throwaway Syncthing, then runs this in a headless 1440x900 sway;
//! each picture is the whole output, saved with grim.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use adw::prelude::*;
use spotless_core::library::album_key;

use super::{on_gtk, open, pump, settle};
use crate::app::Route;

/// The first mapped widget of type `T` under `w` for which `ok` holds.
fn find<T: IsA<gtk::Widget>>(w: &gtk::Widget, ok: &impl Fn(&T) -> bool) -> Option<T> {
    if let Some(t) = w.downcast_ref::<T>()
        && w.is_mapped()
        && ok(t)
    {
        return Some(t.clone());
    }
    let mut child = w.first_child();
    while let Some(c) = child {
        if let Some(t) = find(&c, ok) {
            return Some(t);
        }
        child = c.next_sibling();
    }
    None
}

/// Runs the main loop until a label under `w` starts with `text`.
fn wait_for_label(w: &gtk::Widget, text: &str) {
    let start = Instant::now();
    while find(w, &|l: &gtk::Label| l.label().starts_with(text)).is_none() {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "no label starting with {text:?}"
        );
        pump(Duration::from_millis(100));
    }
}

/// Saves the compositor's output as `<out>/<name>.png`, once redrawn.
fn grim(out: &Path, name: &str) {
    pump(Duration::from_millis(1500));
    let file = out.join(format!("{name}.png"));
    let status = Command::new("grim").arg(&file).status().expect("run grim");
    assert!(status.success(), "grim failed for {}", file.display());
}

fn env_path(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("{name} is not set")))
}

/// Library-relative paths, as `scripts/screenshots.sh` names the tracks.
fn tracks(paths: &[&str]) -> Vec<String> {
    paths.iter().map(|p| (*p).to_owned()).collect()
}

/// Pauses the started track at `secs`, once the player has loaded it.
fn paused_at(app: &crate::app::App, secs: f64) {
    let start = Instant::now();
    while app.player.duration() <= 0.0 && start.elapsed() < Duration::from_secs(10) {
        pump(Duration::from_millis(100));
    }
    app.player.pause();
    app.player.seek(secs);
    pump(Duration::from_millis(500));
}

#[test]
#[ignore = "needs a display, a library and a Syncthing; run by scripts/screenshots.sh"]
fn docs_screenshots() {
    let library = env_path("SPOTLESS_SHOTS_LIBRARY");
    let out = env_path("SPOTLESS_SHOTS_OUT");
    on_gtk(move || {
        let app = open(&library);
        settle(&app);
        let window = app.window.clone().upcast::<gtk::Widget>();

        // Home, with a song of Road Trip paused.
        app.player.play_tracks(
            "Road Trip",
            tracks(&[
                "Velvet Static/Dusk Signals/Night Bus [vs01].m4a",
                "Aurora Lane/Pure Tones/Morning A [pt01].mp3",
                "Juniper Echo/Long Shadows/Lantern [je01].webm",
                "The Oscillators/Square Days/Edge Rider [sq02].opus",
            ]),
            0,
            Some(false),
        );
        paused_at(&app, 63.0);
        app.navigate(Route::Home);
        settle(&app);
        grim(&out, "home");

        // An album playing, with the queue open.
        app.player.play_tracks(
            "Pure Tones",
            tracks(&[
                "Aurora Lane/Pure Tones/Morning A [pt01].mp3",
                "Aurora Lane/Pure Tones/Second Harmonic [pt02].mp3",
                "Aurora Lane/Pure Tones/Low Hum [pt03].mp3",
                "Aurora Lane/Pure Tones/Fifth Above [pt04].mp3",
            ]),
            1,
            Some(false),
        );
        paused_at(&app, 48.0);
        app.navigate(Route::Album(album_key("Aurora Lane", "Pure Tones")));
        app.toggle_queue();
        settle(&app);
        grim(&out, "album");
        app.toggle_queue();

        // Downloads: two links pasted, playlists saved from their sources,
        // the failed songs of the running request listed.
        app.navigate(Route::Downloads);
        settle(&app);
        let input: gtk::TextView = find(&window, &|_| true).expect("the links field");
        input.buffer().set_text(
            "https://open.spotify.com/playlist/0aExamplePlaylist0001\nhttps://music.youtube.com/watch?v=aExampleId1",
        );
        let from_source: gtk::Switch = find(&window, &|_| true).expect("the playlists switch");
        from_source.set_active(true);
        wait_for_label(&window, "Downloads are handled by");
        let failures: gtk::Expander = find(&window, &|_| true).expect("the failed songs");
        failures.set_expanded(true);
        GtkWindowExt::set_focus(&app.window, None::<&gtk::Widget>);
        grim(&out, "downloads");

        // The setup assistant's Syncthing step over Home, with the library
        // already shared.
        app.navigate(Route::Home);
        settle(&app);
        crate::ui::setup::open(&app);
        pump(Duration::from_millis(500));
        let dialog = app
            .window
            .visible_dialog()
            .expect("setup dialog")
            .upcast::<gtk::Widget>();
        let next: gtk::Button = find(&dialog, &|b: &gtk::Button| {
            b.label().is_some_and(|l| l == "Continue")
        })
        .expect("the library step's Continue");
        next.emit_clicked();
        wait_for_label(&dialog, "Shared from");
        grim(&out, "setup-syncthing");
    });
}
