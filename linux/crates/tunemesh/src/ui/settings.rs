//! Settings: library folder, sync and downloads, library status, device
//! info, shortcuts.

use std::rc::Rc;

use gtk::gio;
use gtk::prelude::*;

use crate::app::{App, Page};
use crate::config::{self, expand_home};
use crate::ui::downloads;
use crate::ui::widgets::{label, plural};

fn card(title: &str) -> gtk::Box {
    let c = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .css_classes(["tm-card"])
        .build();
    c.append(&label(title, &["heading"]));
    c
}

fn dim(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .css_classes(["dim"])
        .build()
}

pub fn page(app: &Rc<App>) -> Page {
    let b = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(16)
        .margin_start(24)
        .margin_end(24)
        .margin_top(20)
        .margin_bottom(24)
        .build();
    b.append(&label("Settings", &["tm-page-title"]));

    let lib_card = card("Library folder");
    lib_card.append(&dim(
        "The Syncthing folder that holds your music, playlists and download requests. Every device that shares it sees the same library.",
    ));
    let row = gtk::Box::builder().spacing(12).build();
    let entry = gtk::Entry::builder()
        .text(app.store.root().to_string_lossy())
        .hexpand(true)
        .build();
    let choose = gtk::Button::builder()
        .label("Choose…")
        .css_classes(["tm-pill-outline"])
        .build();
    let apply = gtk::Button::builder()
        .label("Apply")
        .css_classes(["tm-green"])
        .build();
    row.append(&entry);
    row.append(&choose);
    row.append(&apply);
    lib_card.append(&row);
    let (a, e) = (app.clone(), entry.clone());
    choose.connect_clicked(move |_| {
        let dialog = gtk::FileDialog::builder()
            .title("Choose the library folder")
            .modal(true)
            .build();
        dialog.set_initial_folder(Some(&gio::File::for_path(a.store.root())));
        let e = e.clone();
        dialog.select_folder(Some(&a.window), None::<&gio::Cancellable>, move |res| {
            if let Ok(folder) = res
                && let Some(path) = folder.path()
            {
                e.set_text(&path.to_string_lossy());
            }
        });
    });
    // The entry's own handler gets the entry as its argument: a captured
    // reference would keep the entry (and its text field) alive after the
    // page is gone.
    let apply_path = {
        let a = app.clone();
        Rc::new(move |e: &gtk::Entry| {
            let path = expand_home(e.text().trim());
            if path.as_os_str().is_empty() {
                return;
            }
            if !path.is_dir() {
                a.toast(&format!("{} is not a folder", path.display()));
                return;
            }
            if path != a.store.root() {
                a.set_library_path(path);
                a.refresh();
            }
        })
    };
    let (ap, e) = (apply_path.clone(), entry.clone());
    apply.connect_clicked(move |_| ap(&e));
    entry.connect_activate(move |e| apply_path(e));
    b.append(&lib_card);

    let sync = card("Sync");
    sync.append(&dim(
        "Syncthing keeps the library folder the same on all your devices. The setup assistant shares the folder from the Syncthing on this computer and adds your other devices.",
    ));
    let setup = gtk::Button::builder()
        .label("Open setup assistant")
        .css_classes(["tm-pill-outline"])
        .halign(gtk::Align::Start)
        .build();
    setup.set_action_name(Some("app.setup"));
    sync.append(&setup);
    b.append(&sync);

    let dl = card("Downloads");
    let row = gtk::Box::builder().spacing(12).build();
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .hexpand(true)
        .build();
    text.append(&label("This device downloads songs", &[]));
    text.append(&dim(
        "Runs the downloader while TuneMesh is open: it fetches what any device asks for. One device per library downloads. Needs ffmpeg; yt-dlp is fetched automatically.",
    ));
    row.append(&text);
    let switch = gtk::Switch::builder()
        .active(app.config.borrow().downloader)
        .valign(gtk::Align::Center)
        .build();
    let a = app.clone();
    switch.connect_active_notify(move |s| a.set_downloader(s.is_active()));
    row.append(&switch);
    dl.append(&row);
    dl.append(&downloads::status_label(app));
    b.append(&dl);

    let lib = app.store.library();
    let status = card("Library");
    let mut text = format!(
        "{}, {}, {}, {}",
        plural(lib.tracks.len(), "song", "songs"),
        plural(lib.albums.len(), "album", "albums"),
        plural(lib.artists.len(), "artist", "artists"),
        plural(app.store.playlists().len() + 1, "playlist", "playlists"),
    );
    if app.store.is_scanning() {
        text.push_str(" • scanning…");
    }
    status.append(&dim(&text));
    let rescan = gtk::Button::builder()
        .label("Rescan now")
        .css_classes(["tm-pill-outline"])
        .halign(gtk::Align::Start)
        .build();
    let a = app.clone();
    rescan.connect_clicked(move |_| a.store.rescan());
    status.append(&rescan);
    b.append(&status);

    let device = card("This device");
    device.append(&dim(&format!(
        "Device name: {}\nQueue, playback position, volume and these settings are stored on this device only ({}, {}).",
        config::device_name(),
        config::config_file().display(),
        config::state_dir().display(),
    )));
    if let Some(e) = &app.player.init_error {
        let l = dim(&format!("Audio playback is unavailable: {e}"));
        l.add_css_class("tm-error");
        device.append(&l);
    }
    b.append(&device);

    let keys = card("Keyboard shortcuts");
    let grid = gtk::Grid::builder()
        .column_spacing(24)
        .row_spacing(6)
        .build();
    for (i, (k, what)) in [
        ("Space", "Play / pause"),
        ("Ctrl+F", "Search"),
        ("Ctrl+→ / Ctrl+←", "Next / previous track"),
        ("Alt+←", "Go back"),
        ("Ctrl+Alt+Q", "Show or hide the queue"),
        ("Double-click", "Play a track"),
        ("Right-click", "Track options"),
        ("Ctrl+Q", "Quit"),
    ]
    .iter()
    .enumerate()
    {
        grid.attach(&label(k, &["heading"]), 0, i as i32, 1, 1);
        grid.attach(&label(what, &["dim"]), 1, i as i32, 1, 1);
    }
    keys.append(&grid);
    b.append(&keys);

    let about = card("About");
    about.append(&dim(&format!("TuneMesh {}", env!("CARGO_PKG_VERSION"))));
    b.append(&about);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&b)
        .build();
    Page {
        widget: scroller.clone().upcast(),
        scroller: Some(scroller),
    }
}
