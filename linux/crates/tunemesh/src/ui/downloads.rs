//! Downloads: write request files for the worker and show their status.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use gtk::glib;
use gtk::prelude::*;
use tunemesh_core::request::{self, Request, RequestState, Status};

use crate::app::{App, Page};
use crate::config;
use crate::ui::widgets::{self, label, plural, section_title};

thread_local! {
    /// The request list of the Downloads page currently shown, if any.
    static LIST: RefCell<Weak<ListState>> = const { RefCell::new(Weak::new()) };
}

struct ListState {
    app: Weak<App>,
    container: gtk::Box,
}

/// Re-renders the request list if the Downloads page is open (called when
/// files below `.tunemesh/` change); the form keeps its input.
pub fn refresh_if_shown() {
    if let Some(state) = LIST.with(|l| l.borrow().upgrade()) {
        state.render();
    }
}

fn fmt_time(rfc3339: &str) -> String {
    glib::DateTime::from_iso8601(rfc3339, None)
        .ok()
        .and_then(|d| d.to_local().ok())
        .and_then(|d| d.format("%b %-d, %H:%M").ok())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

struct Row {
    id: String,
    request: Option<Request>,
    status: Option<Status>,
}

impl ListState {
    fn render(&self) {
        let Some(app) = self.app.upgrade() else { return };
        while let Some(c) = self.container.first_child() {
            self.container.remove(&c);
        }
        let root = app.store.root();
        let (requests, _) = request::list_requests(&root);
        let statuses = request::list_statuses(&root);
        let mut rows: Vec<Row> = Vec::new();
        for r in requests {
            rows.push(Row { id: r.id.clone(), request: Some(r), status: None });
        }
        for s in statuses {
            match rows.iter_mut().find(|r| r.id == s.id) {
                Some(r) => r.status = Some(s),
                None => rows.push(Row { id: s.id.clone(), request: None, status: Some(s) }),
            }
        }
        rows.sort_by(|a, b| request::order_key(&b.id).cmp(&request::order_key(&a.id)));
        if rows.is_empty() {
            self.container.append(&widgets::empty("No downloads yet. Paste a link or type a song above."));
            return;
        }
        for row in rows {
            self.container.append(&request_row(&app, row));
        }
    }
}

fn request_row(app: &Rc<App>, row: Row) -> gtk::Widget {
    let pending = row.request.is_some();
    let input = row
        .request
        .as_ref()
        .map(|r| r.input.clone())
        .or_else(|| row.status.as_ref().map(|s| s.input.clone()))
        .unwrap_or_default();
    let title = row.status.as_ref().and_then(|s| s.title.clone()).unwrap_or_else(|| input.clone());
    let (state_text, state_class) = match &row.status {
        None => ("Waiting for the downloader".to_owned(), "waiting"),
        Some(s) if !s.state.is_final() && !pending => ("Cancelling…".to_owned(), "waiting"),
        Some(s) => match s.state {
            RequestState::Queued => ("Queued".to_owned(), "queued"),
            RequestState::Downloading => (
                match s.progress {
                    Some(p) => format!("Downloading {:.0}%", p * 100.0),
                    None => "Downloading".to_owned(),
                },
                "downloading",
            ),
            RequestState::Done => (format!("Done • {}", plural(s.tracks.len(), "song", "songs")), "done"),
            RequestState::Failed => ("Failed".to_owned(), "failed"),
            RequestState::Cancelled => ("Cancelled".to_owned(), "cancelled"),
        },
    };

    let b = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).css_classes(["tm-request-row"]).build();
    let top = gtk::Box::builder().spacing(12).build();
    let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).hexpand(true).build();
    let t = label(&title, &["title"]);
    t.add_css_class("heading");
    text.append(&t);
    let mut sub = Vec::new();
    if title != input {
        sub.push(input.clone());
    }
    if let Some(r) = &row.request {
        if !r.requested_by.is_empty() {
            sub.push(format!("from {}", r.requested_by));
        }
        if let Some(p) = &r.playlist {
            sub.push(format!("→ {p}"));
        }
    }
    let when = row
        .request
        .as_ref()
        .map(|r| r.requested_at.clone())
        .or_else(|| row.status.as_ref().map(|s| s.updated_at.clone()))
        .map(|t| fmt_time(&t))
        .unwrap_or_default();
    if !when.is_empty() {
        sub.push(when);
    }
    text.append(&label(&sub.join(" • "), &["dim", "tm-small"]));
    top.append(&text);
    let state = gtk::Label::builder().label(&state_text).valign(gtk::Align::Center).css_classes(["tm-state", state_class]).build();
    top.append(&state);

    let final_state = row.status.as_ref().is_some_and(|s| s.state.is_final());
    if final_state {
        let tracks: Vec<String> = row.status.as_ref().map(|s| s.tracks.clone()).unwrap_or_default();
        let lib = app.store.library();
        let available: Vec<String> = tracks.into_iter().filter(|t| lib.track_index(t).is_some()).collect();
        if !available.is_empty() {
            let play = gtk::Button::builder()
                .icon_name("media-playback-start-symbolic")
                .tooltip_text("Play")
                .css_classes(["tm-icon-btn"])
                .valign(gtk::Align::Center)
                .build();
            let (a, name) = (app.clone(), title.clone());
            play.connect_clicked(move |_| a.player.play_tracks(&name, available.clone(), 0, Some(false)));
            top.append(&play);
        }
        let remove = widgets::icon_button("window-close-symbolic", "Remove from list");
        remove.set_valign(gtk::Align::Center);
        let (a, id) = (app.clone(), row.id.clone());
        remove.connect_clicked(move |_| {
            let _ = request::cancel(&a.store.root(), &id);
            if let Err(e) = std::fs::remove_file(request::status_path(&a.store.root(), &id)) {
                a.error("Could not remove", e);
            }
            refresh_if_shown();
        });
        top.append(&remove);
    } else if pending {
        let cancel = gtk::Button::builder().label("Cancel").css_classes(["tm-pill-outline"]).valign(gtk::Align::Center).build();
        let (a, id) = (app.clone(), row.id.clone());
        cancel.connect_clicked(move |_| {
            match request::cancel(&a.store.root(), &id) {
                Ok(()) => a.toast("Download cancelled"),
                Err(e) => a.error("Could not cancel", e),
            }
            refresh_if_shown();
        });
        top.append(&cancel);
    }
    b.append(&top);

    if let Some(s) = &row.status {
        if s.state == RequestState::Downloading {
            let bar = gtk::ProgressBar::builder().css_classes(["tm-progress"]).build();
            match s.progress {
                Some(p) => bar.set_fraction(p),
                None => bar.pulse(),
            }
            b.append(&bar);
        }
        if let Some(m) = &s.message {
            let l = gtk::Label::builder().label(m).xalign(0.0).wrap(true).selectable(true).css_classes(["tm-small"]).build();
            l.add_css_class(if s.state == RequestState::Failed { "tm-error" } else { "dim" });
            b.append(&l);
        }
    }
    b.upcast()
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
    b.append(&label("Downloads", &["tm-page-title"]));

    let card = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).css_classes(["tm-card"]).build();
    card.append(&label("Add music", &["heading"]));
    let hint = gtk::Label::builder()
        .label("Paste a YouTube, YouTube Music, SoundCloud, Bandcamp or Spotify link, or type what to search for. The download worker fetches it and the song syncs to all your devices.")
        .xalign(0.0)
        .wrap(true)
        .css_classes(["dim"])
        .build();
    card.append(&hint);
    let input_row = gtk::Box::builder().spacing(12).build();
    let entry = gtk::Entry::builder().placeholder_text("Link or search text").hexpand(true).build();
    let go = gtk::Button::builder().label("Download").css_classes(["tm-green"]).sensitive(false).build();
    input_row.append(&entry);
    input_row.append(&go);
    card.append(&input_row);

    let options = gtk::Box::builder().spacing(12).build();
    options.append(&label("Add to playlist", &["dim"]));
    let names = app.store.playlist_names();
    let mut choices: Vec<String> = vec!["None".to_owned(), tunemesh_core::LIKED_SONGS.to_owned()];
    choices.extend(names);
    let refs: Vec<&str> = choices.iter().map(String::as_str).collect();
    let dropdown = gtk::DropDown::from_strings(&refs);
    options.append(&dropdown);
    let spacer = gtk::Box::builder().width_request(24).build();
    options.append(&spacer);
    let switch = gtk::Switch::builder().valign(gtk::Align::Center).build();
    options.append(&switch);
    options.append(&label("Also create a playlist from this link", &[]));
    card.append(&options);
    b.append(&card);

    let s = switch.clone();
    dropdown.connect_selected_notify(move |d| {
        let none = d.selected() == 0;
        s.set_sensitive(none);
        if !none {
            s.set_active(false);
        }
    });
    let g = go.clone();
    entry.connect_changed(move |e| g.set_sensitive(!e.text().trim().is_empty()));

    let submit = {
        let (a, entry, dropdown, switch, choices) = (app.clone(), entry.clone(), dropdown.clone(), switch.clone(), choices.clone());
        move || {
            let text = entry.text().trim().to_owned();
            if text.is_empty() {
                return;
            }
            let sel = dropdown.selected() as usize;
            let playlist = (sel > 0).then(|| choices[sel].clone());
            let req = Request::new(&text, playlist, switch.is_active(), &config::device_name());
            match req.write(&a.store.root()) {
                Ok(()) => {
                    entry.set_text("");
                    a.toast("Download requested");
                    refresh_if_shown();
                }
                Err(e) => a.error("Could not write the request", e),
            }
        }
    };
    let submit = Rc::new(submit);
    let s2 = submit.clone();
    go.connect_clicked(move |_| s2());
    entry.connect_activate(move |_| submit());

    let head = gtk::Box::builder().spacing(12).build();
    let title = section_title("Requests");
    title.set_hexpand(true);
    head.append(&title);
    let clear = gtk::Button::builder().label("Clear finished").css_classes(["tm-pill-outline"]).valign(gtk::Align::Center).build();
    let a = app.clone();
    clear.connect_clicked(move |_| {
        let n = request::clear_finished(&a.store.root());
        a.toast(&format!("Cleared {}", plural(n, "request", "requests")));
        refresh_if_shown();
    });
    head.append(&clear);
    b.append(&head);

    let container = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).build();
    b.append(&container);
    let state = Rc::new(ListState { app: Rc::downgrade(app), container: container.clone() });
    state.render();
    LIST.with(|l| *l.borrow_mut() = Rc::downgrade(&state));
    // The list state lives as long as its container.
    unsafe { container.set_data("tm-list-state", state) };

    let scroller = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&b).build();
    Page { widget: scroller.clone().upcast(), scroller: Some(scroller) }
}
