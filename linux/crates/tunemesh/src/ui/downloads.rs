//! Downloads: write request files for the worker and show their status.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use tunemesh_core::limits;
use tunemesh_core::request::{self, Request, RequestState, StatusRow};

use crate::app::{App, Page, Route};
use crate::config;
use crate::downloader;
use crate::ui::widgets::{self, label, plural, section_title};

thread_local! {
    /// The request list of the Downloads page currently shown, if any.
    static LIST: RefCell<Weak<ListState>> = const { RefCell::new(Weak::new()) };
    /// Downloader status lines on screen (Downloads page, Settings).
    static STATUS: RefCell<Vec<(Weak<App>, glib::WeakRef<gtk::Label>)>> = const { RefCell::new(Vec::new()) };
    static STATUS_TIMER: Cell<bool> = const { Cell::new(false) };
}

struct ListState {
    app: Weak<App>,
    /// Weak: the container keeps this state, not the other way round.
    container: glib::WeakRef<gtk::Box>,
    /// Request and status files are read on a thread, one load at a time.
    loading: Cell<bool>,
    again: Cell<bool>,
    /// The rows on screen with their widgets, newest first.
    shown: RefCell<Vec<(Row, gtk::Widget)>>,
    /// Bumped by every `show`; a fill still running for an older one stops.
    fill: Cell<u64>,
}

/// Re-renders the request list if the Downloads page is open (called when
/// files below `.tunemesh/` change); the form keeps its input.
pub fn refresh_if_shown() {
    if let Some(state) = LIST.with(|l| l.borrow().upgrade()) {
        state.render();
    }
    refresh_status();
}

// ---- who downloads ---------------------------------------------------------

fn apply_status(app: &App, l: &gtk::Label) {
    let (text, error) = downloader::summary(&app.store.root(), &app.downloader.state());
    l.set_text(&text);
    if error {
        l.add_css_class("tm-error")
    } else {
        l.remove_css_class("tm-error")
    }
}

/// A line saying which device downloads and how this device's downloader is
/// doing; it updates itself while shown.
pub fn status_label(app: &Rc<App>) -> gtk::Label {
    let l = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .css_classes(["dim", "tm-downloader-status"])
        .build();
    apply_status(app, &l);
    STATUS.with(|s| s.borrow_mut().push((Rc::downgrade(app), l.downgrade())));
    // Heartbeats age and restarts count down without any file changing.
    if !STATUS_TIMER.with(|t| t.replace(true)) {
        glib::timeout_add_seconds_local(5, || {
            refresh_status();
            let alive = STATUS.with(|s| !s.borrow().is_empty());
            if !alive {
                STATUS_TIMER.with(|t| t.set(false));
            }
            glib::ControlFlow::from(alive)
        });
    }
    l
}

/// Updates every downloader status line on screen.
pub fn refresh_status() {
    let live: Vec<(Rc<App>, gtk::Label)> = STATUS.with(|s| {
        let mut v = s.borrow_mut();
        v.retain(|(a, l)| a.strong_count() > 0 && l.upgrade().is_some_and(|l| l.root().is_some()));
        v.iter()
            .filter_map(|(a, l)| Some((a.upgrade()?, l.upgrade()?)))
            .collect()
    });
    for (app, l) in live {
        apply_status(&app, &l);
    }
}

// ---- pasted input ------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A playlist: saved as a TuneMesh playlist by default.
    Playlist,
    /// An album: it shows up as an album anyway.
    Album,
    /// One track, or search text.
    Single,
}

fn query_param<'a>(url: &'a str, name: &str) -> Option<&'a str> {
    let query = url.split_once('?')?.1.split('#').next()?;
    query
        .split('&')
        .find_map(|kv| kv.strip_prefix(name)?.strip_prefix('='))
}

/// What a pasted line is (Spotify and YouTube / YouTube Music URLs; other
/// input counts as a single item). Same rule as the worker and the Android
/// app (contract: "YouTube links"): any `list=` makes a YouTube link a
/// playlist, even with a video in it, except auto-generated mixes (`RD…`).
pub fn classify(line: &str) -> Kind {
    let lower = line.trim().to_ascii_lowercase();
    let Some(rest) = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
    else {
        return Kind::Single;
    };
    let (host, path) = match rest.find(['/', '?', '#']) {
        Some(i) => (&rest[..i], rest[i..].trim_start_matches('/')),
        None => (rest, ""),
    };
    let host = host.strip_prefix("www.").unwrap_or(host);
    let host = host.strip_prefix("m.").unwrap_or(host);
    match host {
        "open.spotify.com" => {
            // `/intl-de/playlist/…` and `/playlist/…`
            let mut parts = path.split(['/', '?']).filter(|p| !p.is_empty());
            let first = parts.next().unwrap_or("");
            let kind = if first.starts_with("intl-") {
                parts.next().unwrap_or("")
            } else {
                first
            };
            match kind {
                "playlist" => Kind::Playlist,
                "album" => Kind::Album,
                _ => Kind::Single,
            }
        }
        "youtube.com" | "music.youtube.com" | "youtu.be" => {
            if path.starts_with("browse/mpreb") {
                return Kind::Album;
            }
            match query_param(line.trim(), "list") {
                // YouTube Music albums are playlists with this prefix.
                Some(id) if id.starts_with("OLAK5uy_") => Kind::Album,
                // YouTube Music's editorial playlists are fixed lists.
                Some(id) if id.starts_with("RDCLAK5uy_") => Kind::Playlist,
                // Other RD lists are endless mixes generated for one video.
                Some(id) if id.starts_with("RD") => Kind::Single,
                Some(id) if !id.is_empty() => Kind::Playlist,
                _ => Kind::Single,
            }
        }
        _ => Kind::Single,
    }
}

/// Request ids for one paste, in pasted order. Ids sort by their
/// millisecond prefix (contract: "Download requests"), so each id gets a
/// later millisecond than the one before instead of waiting for the clock.
struct OrderedIds {
    last: u128,
}

impl OrderedIds {
    fn new() -> Self {
        Self { last: 0 }
    }

    fn next(&mut self) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        self.last = now.max(self.last + 1);
        format!("{}-{:08x}", self.last, fastrand::u32(..))
    }
}

/// The non-empty, trimmed lines of a paste, without duplicates.
pub fn input_lines(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if !out.iter().any(|o| o == l) {
            out.push(l.to_owned());
        }
    }
    out
}

// ---- request list ----------------------------------------------------------------

fn fmt_time(rfc3339: &str) -> String {
    glib::DateTime::from_iso8601(rfc3339, None)
        .ok()
        .and_then(|d| d.to_local().ok())
        .and_then(|d| d.format("%b %-d, %H:%M").ok())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

/// One download as listed: core's cut projection of its request and
/// status, which keeps counts rather than the status's lists. Opening a
/// list (warnings, failures) or playing the songs reads that one status.
#[derive(Clone, PartialEq)]
struct Row {
    id: String,
    request: Option<Request>,
    status: Option<StatusRow>,
}

/// The newest requests with their status (at most `MAX_LISTED_REQUESTS`,
/// cut by core to what a list shows), and how many there are in all.
fn load_rows(root: &std::path::Path) -> (Vec<Row>, usize) {
    let (recent, total) = request::recent(root);
    let rows = recent
        .into_iter()
        .map(|r| Row {
            id: r.id,
            request: r.request,
            status: r.status,
        })
        .collect();
    (rows, total)
}

impl ListState {
    fn render(self: &Rc<Self>) {
        if self.loading.replace(true) {
            self.again.set(true);
            return;
        }
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let root = app.store.root();
        let (tx, rx) = async_channel::bounded(1);
        std::thread::Builder::new()
            .name("downloads".into())
            .spawn(move || {
                let _ = tx.send_blocking(load_rows(&root));
            })
            .expect("spawn downloads thread");
        let state = self.clone();
        glib::spawn_future_local(async move {
            let loaded = rx.recv().await;
            state.loading.set(false);
            if let (Ok((rows, total)), Some(app)) = (loaded, state.app.upgrade()) {
                state.show(&app, rows, total);
            }
            if state.again.replace(false) {
                state.render();
            }
        });
    }

    /// Shows `rows`: a row whose request and status are unchanged keeps its
    /// widget, and new rows are built `ROWS_PER_PASS` at a time, so neither
    /// opening the page nor a status update holds up the window.
    fn show(self: &Rc<Self>, app: &Rc<App>, rows: Vec<Row>, total: usize) {
        let Some(container) = self.container.upgrade() else {
            return;
        };
        let fill = self.fill.get() + 1;
        self.fill.set(fill);
        if rows.is_empty() {
            while let Some(c) = container.first_child() {
                container.remove(&c);
            }
            self.shown.borrow_mut().clear();
            container.append(&widgets::empty(
                "No downloads yet. Paste links or type a song above.",
            ));
            return;
        }
        let old = self
            .shown
            .take()
            .into_iter()
            .map(|(r, w)| (r.id.clone(), (r, w)))
            .collect();
        let f = Rc::new(Fill {
            rows,
            total,
            old: RefCell::new(old),
            placed: RefCell::new(Vec::new()),
        });
        self.fill_rows(app.clone(), f, 0, fill);
    }

    fn fill_rows(self: &Rc<Self>, app: Rc<App>, f: Rc<Fill>, start: usize, fill: u64) {
        if self.fill.get() != fill {
            return;
        }
        let Some(container) = self.container.upgrade() else {
            return;
        };
        let mut prev = f.placed.borrow().last().map(|(_, w)| w.clone());
        let (mut i, mut built) = (start, 0);
        while i < f.rows.len() && built < ROWS_PER_PASS {
            let row = &f.rows[i];
            let w = match f.old.borrow_mut().remove(&row.id) {
                Some((old, w)) if old == *row => w,
                stale => {
                    if let Some((_, w)) = stale {
                        container.remove(&w);
                    }
                    built += 1;
                    request_row(&app, row.clone())
                }
            };
            if w.parent().is_some() {
                container.reorder_child_after(&w, prev.as_ref());
            } else {
                container.insert_child_after(&w, prev.as_ref());
            }
            prev = Some(w.clone());
            f.placed.borrow_mut().push((row.clone(), w));
            i += 1;
        }
        if i < f.rows.len() {
            // The next pass waits for the next frame: idle passes ran several
            // times between frames, and the frame then measured all the rows
            // they had added (over 100 ms on the first visit, shaping text).
            let me = self.clone();
            let next = Cell::new(Some((app, f)));
            container.add_tick_callback(move |_, _| {
                if let Some((app, f)) = next.take() {
                    me.fill_rows(app, f, i, fill);
                }
                glib::ControlFlow::Break
            });
            return;
        }
        // Done: drop what is no longer listed (and the old footer).
        let placed: HashSet<gtk::Widget> =
            f.placed.borrow().iter().map(|(_, w)| w.clone()).collect();
        let mut child = container.first_child();
        while let Some(c) = child {
            child = c.next_sibling();
            if !placed.contains(&c) {
                container.remove(&c);
            }
        }
        let shown = f.rows.len();
        if f.total > shown {
            container.append(&label(
                &format!(
                    "…and {} older",
                    plural(f.total - shown, "request", "requests")
                ),
                &["dim", "tm-small"],
            ));
        }
        *self.shown.borrow_mut() = f.placed.take();
    }
}

/// Request rows built per frame; a row with long warnings takes a
/// millisecond or two to build and lay out.
const ROWS_PER_PASS: usize = 12;

/// A `show` in progress.
struct Fill {
    rows: Vec<Row>,
    total: usize,
    /// Widgets shown before, by request id, until reused or dropped.
    old: RefCell<HashMap<String, (Row, gtk::Widget)>>,
    /// Rows placed so far, in order.
    placed: RefCell<Vec<(Row, gtk::Widget)>>,
}

/// "3 of 12 songs" once the worker knows how many items a request has.
fn count_text(s: &StatusRow) -> Option<String> {
    let total = s.items_total.filter(|t| *t > 1)?;
    Some(format!(
        "{} of {total} songs",
        s.items_done.unwrap_or(0).min(total)
    ))
}

/// Overall progress: finished items plus the current item's progress.
fn fraction(s: &StatusRow) -> Option<f64> {
    match (s.items_total.filter(|t| *t > 1), s.progress) {
        (Some(total), p) => Some(
            ((s.items_done.unwrap_or(0) as f64 + p.unwrap_or(0.0)) / total as f64).clamp(0.0, 1.0),
        ),
        (None, p) => p,
    }
}

fn state_text(row: &Row, pending: bool) -> (String, &'static str) {
    // The worker gives a request its status when it scans the queue.
    let Some(s) = &row.status else {
        return ("Waiting".to_owned(), "waiting");
    };
    if !s.state.is_final() && !pending {
        return ("Cancelling…".to_owned(), "waiting");
    }
    let failed = s.failure_count.get();
    match s.state {
        RequestState::Queued => ("Queued".to_owned(), "queued"),
        RequestState::Downloading => {
            let mut t = match (count_text(s), s.progress) {
                (Some(c), _) => format!("Downloading {c}"),
                (None, Some(p)) => format!("Downloading {:.0}%", p * 100.0),
                (None, None) => "Downloading".to_owned(),
            };
            if failed > 0 {
                t.push_str(&format!(" • {failed} failed"));
            }
            (t, "downloading")
        }
        RequestState::Done => {
            let mut t = format!("Done • {}", plural(s.track_count.get(), "song", "songs"));
            if failed > 0 {
                t.push_str(&format!(" • {failed} failed"));
            }
            (t, if failed > 0 { "partial" } else { "done" })
        }
        RequestState::Failed => ("Failed".to_owned(), "failed"),
        RequestState::Cancelled => ("Cancelled".to_owned(), "cancelled"),
    }
}

/// Lines read from one status file when the user opens its list: the
/// list keeps only counts.
fn status_lines(
    root: &std::path::Path,
    id: &str,
    lines: impl FnOnce(&request::Status) -> Vec<String>,
) -> Vec<String> {
    match request::read_status(root, id) {
        Some(s) => lines(&s),
        None => vec!["This download's status is no longer available.".to_owned()],
    }
}

/// Up to `MAX_SHOWN_ROWS` lines, then "…and N more".
fn shown_lines(items: impl ExactSizeIterator<Item = String>) -> Vec<String> {
    let n = items.len();
    let mut lines: Vec<String> = items.take(limits::MAX_SHOWN_ROWS).collect();
    if n > limits::MAX_SHOWN_ROWS {
        lines.push(format!("…and {} more", n - limits::MAX_SHOWN_ROWS));
    }
    lines
}

fn request_row(app: &Rc<App>, row: Row) -> gtk::Widget {
    let pending = row.request.is_some();
    let input: String = row
        .request
        .as_ref()
        .map(|r| r.input.as_str())
        .or_else(|| row.status.as_ref().map(|s| s.input.as_str()))
        .unwrap_or_default()
        .to_owned();
    let title: String = row
        .status
        .as_ref()
        .and_then(|s| s.title.as_deref())
        .map_or_else(|| input.clone(), str::to_owned);
    let (state_text, state_class) = state_text(&row, pending);

    let b = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .css_classes(["tm-request-row"])
        .build();
    let top = gtk::Box::builder().spacing(12).build();
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .build();
    let t = label(&limits::clip_line(&title), &["title"]);
    t.add_css_class("heading");
    text.append(&t);
    let mut sub = Vec::new();
    if title != input {
        sub.push(limits::clip_line(&input).into_owned());
    }
    if let Some(r) = &row.request {
        if !r.requested_by.is_empty() {
            sub.push(format!("from {}", limits::clip_line(&r.requested_by)));
        }
        if let Some(p) = &r.playlist {
            sub.push(format!("→ {}", limits::clip_line(p)));
        } else if r.playlist_from_source {
            sub.push("saved as a playlist".to_owned());
        }
    }
    let when = row
        .request
        .as_ref()
        .map(|r| r.requested_at.as_str())
        .or_else(|| row.status.as_ref().map(|s| s.updated_at.as_str()))
        .map(fmt_time)
        .unwrap_or_default();
    if !when.is_empty() {
        sub.push(when);
    }
    text.append(&label(&sub.join(" • "), &["dim", "tm-small"]));
    top.append(&text);
    let state = gtk::Label::builder()
        .label(&state_text)
        .valign(gtk::Align::Center)
        .css_classes(["tm-state", state_class])
        .build();
    top.append(&state);

    let final_state = row.status.as_ref().is_some_and(|s| s.state.is_final());
    if final_state {
        // The list keeps only the number of songs: they are read when
        // the user plays them.
        if row.status.as_ref().is_some_and(|s| s.track_count.get() > 0) {
            let play = gtk::Button::builder()
                .icon_name("media-playback-start-symbolic")
                .tooltip_text("Play")
                .css_classes(["tm-icon-btn"])
                .valign(gtk::Align::Center)
                .build();
            let (a, name, id) = (app.clone(), title.clone(), row.id.clone());
            play.connect_clicked(move |_| {
                let lib = a.store.library();
                let available: Vec<String> = request::read_status(&a.store.root(), &id)
                    .map(|s| {
                        s.tracks
                            .iter()
                            .filter(|t| lib.track_index(t).is_some())
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                if available.is_empty() {
                    a.toast("These songs are not in the library on this device yet");
                } else {
                    a.player.play_tracks(&name, available, 0, Some(false));
                }
            });
            top.append(&play);
        }
        let remove = widgets::icon_button("window-close-symbolic", "Remove from list");
        remove.set_valign(gtk::Align::Center);
        let (a, id) = (app.clone(), row.id.clone());
        remove.connect_clicked(move |_| {
            let _ = request::cancel(&a.store.root(), &id);
            if let Err(e) = request::remove_status(&a.store.root(), &id) {
                a.error("Could not remove", e);
            }
            refresh_if_shown();
        });
        top.append(&remove);
    } else if pending {
        let cancel = gtk::Button::builder()
            .label("Cancel")
            .css_classes(["tm-pill-outline"])
            .valign(gtk::Align::Center)
            .build();
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
            let bar = gtk::ProgressBar::builder()
                .css_classes(["tm-progress"])
                .build();
            match fraction(s) {
                Some(p) => bar.set_fraction(p),
                None => bar.pulse(),
            }
            b.append(&bar);
        }
        // The final message repeats the warnings, which are listed below.
        if let Some(m) = s
            .message
            .as_ref()
            .filter(|m| !s.warnings.iter().any(|w| w.as_str() == m.as_str()))
        {
            let class = if s.state == RequestState::Failed {
                "tm-error"
            } else {
                "dim"
            };
            b.append(&detail_text(m, class));
            if is_long(m) {
                let full = m.to_string();
                b.append(&lazy_expander(
                    "Full message",
                    move || vec![full.clone()],
                    class,
                ));
            }
        }
        // Warnings and failures can run to hundreds per request: the list
        // keeps the first warnings, shown at once, and counts the rest and
        // the failures, which are read from the status file when their
        // expander is opened.
        for w in s.warnings.iter() {
            b.append(&detail_text(w, "tm-warning"));
        }
        let root = app.store.root();
        let more = s.warning_count().saturating_sub(s.warnings.len());
        if more > 0 {
            let (root, id) = (root.clone(), row.id.clone());
            b.append(&lazy_expander(
                &format!("{} more", plural(more, "warning", "warnings")),
                move || {
                    status_lines(&root, &id, |st| {
                        let rest = st.warnings.iter().skip(limits::MAX_LISTED_WARNINGS);
                        shown_lines(rest.map(|w| w.to_string()))
                    })
                },
                "tm-warning",
            ));
        }
        let failed = s.failure_count.get();
        if failed > 0 {
            let id = row.id.clone();
            b.append(&lazy_expander(
                &format!(
                    "{} could not be downloaded",
                    plural(failed, "song", "songs")
                ),
                move || {
                    status_lines(&root, &id, |st| {
                        shown_lines(
                            st.failures
                                .iter()
                                .map(|f| format!("{}: {}", limits::clip_line(&f.item), f.error)),
                        )
                    })
                },
                "dim",
            ));
        }
    }
    b.upcast()
}

/// A wrapped, selectable line of request detail.
fn small_text(text: &str, class: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(limits::clip_text(text))
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .selectable(true)
        .css_classes(["tm-small", class])
        .build()
}

/// Whether `text` is more than one short line. Laying out a 2,000
/// character paragraph costs about 0.1 s, so with up to 200 requests on the
/// page only short text wraps in the list; long text shows its first line.
fn is_long(text: &str) -> bool {
    text.contains('\n') || text.chars().nth(limits::MAX_LINE_CHARS).is_some()
}

/// Request detail in the list: wrapped when short, else one cut line.
fn detail_text(text: &str, class: &str) -> gtk::Label {
    if !is_long(text) {
        return small_text(text, class);
    }
    gtk::Label::builder()
        .label(limits::clip_line(text))
        .xalign(0.0)
        .single_line_mode(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .selectable(true)
        .css_classes(["tm-small", class])
        .build()
}

/// An expander whose lines are made the first time it is opened.
fn lazy_expander(
    title: &str,
    lines: impl Fn() -> Vec<String> + 'static,
    class: &str,
) -> gtk::Expander {
    let exp = gtk::Expander::builder()
        .label(title)
        .css_classes(["tm-small"])
        .build();
    let class = class.to_owned();
    exp.connect_expanded_notify(move |e| {
        if !e.is_expanded() || e.child().is_some() {
            return;
        }
        let list = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .margin_start(18)
            .build();
        for l in lines() {
            list.append(&small_text(&l, &class));
        }
        e.set_child(Some(&list));
    });
    exp
}

// ---- the page -------------------------------------------------------------------

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

    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .css_classes(["tm-card"])
        .build();
    card.append(&label("Add music", &["heading"]));
    let hint = gtk::Label::builder()
        .label("Paste YouTube, YouTube Music, SoundCloud, Bandcamp or Spotify links (songs, albums or playlists), one per line, or type what to search for. The downloader fetches them and the songs sync to all your devices.")
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .css_classes(["dim"])
        .build();
    card.append(&hint);
    let status = status_label(app);
    card.append(&status);

    let view = gtk::TextView::builder()
        .wrap_mode(gtk::WrapMode::WordChar)
        .accepts_tab(false)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(10)
        .right_margin(10)
        .css_classes(["tm-input"])
        .build();
    let placeholder = gtk::Label::builder()
        .label("Links or search text, one per line (Ctrl+Enter to download)")
        .xalign(0.0)
        .yalign(0.0)
        .margin_top(8)
        .margin_start(12)
        .can_target(false)
        .css_classes(["dim"])
        .build();
    let overlay = gtk::Overlay::builder().child(&view).build();
    overlay.add_overlay(&placeholder);
    let input_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(64)
        .max_content_height(180)
        .propagate_natural_height(true)
        .child(&overlay)
        .css_classes(["tm-input-frame"])
        .build();
    card.append(&input_scroll);

    let options = gtk::Box::builder().spacing(12).build();
    options.append(&label("Add to playlist", &["dim"]));
    let names = app.store.playlist_names();
    let mut choices: Vec<String> = vec!["None".to_owned(), tunemesh_core::LIKED_SONGS.to_owned()];
    choices.extend(names);
    let shown: Vec<String> = choices
        .iter()
        .map(|c| limits::clip_line(c).into_owned())
        .collect();
    let refs: Vec<&str> = shown.iter().map(String::as_str).collect();
    let dropdown = gtk::DropDown::from_strings(&refs);
    dropdown.set_valign(gtk::Align::Center);
    options.append(&dropdown);
    let spacer = gtk::Box::builder().width_request(24).build();
    options.append(&spacer);
    let switch = gtk::Switch::builder().valign(gtk::Align::Center).build();
    options.append(&switch);
    options.append(&label(
        "Save playlists and albums as TuneMesh playlists",
        &[],
    ));
    let fill = gtk::Box::builder().hexpand(true).build();
    options.append(&fill);
    let go = gtk::Button::builder()
        .label("Download")
        .css_classes(["tm-green"])
        .sensitive(false)
        .build();
    options.append(&go);
    card.append(&options);
    b.append(&card);

    // The switch follows the input (on when a playlist link is pasted) until
    // the user flips it.
    let user_set = Rc::new(Cell::new(false));
    let auto = Rc::new(Cell::new(false));
    let (u, au) = (user_set.clone(), auto.clone());
    switch.connect_active_notify(move |_| {
        if !au.get() {
            u.set(true);
        }
    });
    let s = switch.clone();
    dropdown.connect_selected_notify(move |d| {
        let none = d.selected() == 0;
        s.set_sensitive(none);
        if !none {
            s.set_active(false);
        }
    });
    let (g, s, ph, dd) = (
        go.clone(),
        switch.clone(),
        placeholder.clone(),
        dropdown.clone(),
    );
    view.buffer().connect_changed(move |buf| {
        let text = buf.text(&buf.start_iter(), &buf.end_iter(), false);
        let lines = input_lines(&text);
        ph.set_visible(text.is_empty());
        g.set_sensitive(!lines.is_empty());
        g.set_label(&match lines.len() {
            0 | 1 => "Download".to_owned(),
            n => format!("Download {n}"),
        });
        if text.trim().is_empty() {
            user_set.set(false);
        }
        if !user_set.get() && dd.selected() == 0 {
            auto.set(true);
            s.set_active(lines.iter().any(|l| classify(l) == Kind::Playlist));
            auto.set(false);
        }
    });

    // Weak: the text view's own key controller runs this, so a strong
    // reference would keep the view and the whole form alive after the page
    // is gone.
    let submit = {
        let (a, view, dropdown, switch, choices) = (
            app.clone(),
            view.downgrade(),
            dropdown.downgrade(),
            switch.downgrade(),
            choices.clone(),
        );
        move || {
            let (Some(view), Some(dropdown), Some(switch)) =
                (view.upgrade(), dropdown.upgrade(), switch.upgrade())
            else {
                return;
            };
            let buf = view.buffer();
            let lines = input_lines(&buf.text(&buf.start_iter(), &buf.end_iter(), false));
            if lines.is_empty() {
                return;
            }
            let sel = dropdown.selected() as usize;
            let playlist = (sel > 0).then(|| choices[sel].clone());
            let as_playlists = switch.is_active() && playlist.is_none();
            let device = config::device_name();
            let root = a.store.root();
            let mut ids = OrderedIds::new();
            let requests: Vec<(String, Request)> = lines
                .into_iter()
                .map(|line| {
                    let from_source = as_playlists && classify(&line) != Kind::Single;
                    let mut req = Request::new(&line, playlist.clone(), from_source, &device);
                    req.id = ids.next().into();
                    (line, req)
                })
                .collect();
            // Each write is synced to disk: a long paste takes a while, so it
            // runs off the GTK thread while the box shows what is going on.
            view.set_sensitive(false);
            let (a, view) = (a.clone(), view.clone());
            glib::spawn_future_local(async move {
                let failed = gio::spawn_blocking(move || {
                    // The lines as typed (a request holds its input cut to
                    // the limit), and the first error.
                    let mut failed: Vec<String> = Vec::new();
                    let mut first_error = None;
                    let total = requests.len();
                    for (line, req) in requests {
                        if let Err(e) = req.write(&root) {
                            eprintln!(
                                "tunemesh: writing the request for {:?}: {e}",
                                limits::clip_line(&line)
                            );
                            first_error.get_or_insert(e);
                            failed.push(line);
                        }
                    }
                    (total, failed, first_error)
                })
                .await;
                view.set_sensitive(true);
                let buf = view.buffer();
                match failed {
                    Ok((total, failed, _)) if failed.is_empty() => {
                        buf.set_text("");
                        a.toast(&format!(
                            "Requested {}",
                            plural(total, "download", "downloads")
                        ));
                    }
                    Ok((_, failed, e)) => {
                        // Keep what failed in the box so it can be retried.
                        buf.set_text(&failed.join("\n"));
                        let why = e.map(|e| e.to_string()).unwrap_or_default();
                        a.toast(&format!(
                            "Could not write {}: {why}",
                            plural(failed.len(), "request", "requests")
                        ));
                    }
                    Err(_) => a.toast("Could not write the requests (see the log)"),
                }
                refresh_if_shown();
            });
        }
    };
    let submit = Rc::new(submit);
    let s2 = submit.clone();
    go.connect_clicked(move |_| s2());
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(move |_, key, _, mods| {
        if matches!(key, gdk::Key::Return | gdk::Key::KP_Enter)
            && mods.contains(gdk::ModifierType::CONTROL_MASK)
        {
            submit();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    view.add_controller(keys);

    let head = gtk::Box::builder().spacing(12).build();
    let title = section_title("Requests");
    title.set_hexpand(true);
    head.append(&title);
    let clear = gtk::Button::builder()
        .label("Clear finished")
        .css_classes(["tm-pill-outline"])
        .valign(gtk::Align::Center)
        .build();
    let a = app.clone();
    clear.connect_clicked(move |button| {
        // Reads every orphan status file (up to 1 MiB each): off the GTK
        // thread, with the button off meanwhile.
        button.set_sensitive(false);
        let (a, button) = (a.clone(), button.downgrade());
        let root = a.store.root();
        glib::spawn_future_local(async move {
            match gio::spawn_blocking(move || request::clear_finished(&root)).await {
                Ok(n) => a.toast(&format!("Cleared {}", plural(n, "request", "requests"))),
                Err(_) => a.toast("Could not clear the finished requests (see the log)"),
            }
            if let Some(button) = button.upgrade() {
                button.set_sensitive(true);
            }
            refresh_if_shown();
        });
    });
    head.append(&clear);
    b.append(&head);

    let container = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .build();
    b.append(&container);
    let state = Rc::new(ListState {
        app: Rc::downgrade(app),
        container: container.downgrade(),
        loading: Cell::new(false),
        again: Cell::new(false),
        shown: RefCell::new(Vec::new()),
        fill: Cell::new(0),
    });
    state.render();
    LIST.with(|l| *l.borrow_mut() = Rc::downgrade(&state));
    // The list state lives as long as its container.
    unsafe { container.set_data("tm-list-state", state) };

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&b)
        .build();
    if app.route() == Route::Downloads {
        glib::idle_add_local_once(move || {
            view.grab_focus();
        });
    }
    Page {
        widget: scroller.clone().upcast(),
        scroller: Some(scroller),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R3-Opus-3: what the Downloads page keeps of 200 requests and 200
    /// statuses with every field at its limit (or far over it) and each
    /// file at the 1 MiB cap stays within what the contract's limits allow
    /// a listed row, however large the files are.
    #[test]
    fn listed_downloads_keep_only_a_bounded_projection() {
        use tunemesh_core::limits::*;
        // About 400 MB of files: disk-backed /var/tmp when it is writable,
        // else the system temporary directory (the Nix build sandbox has a
        // read-only /var/tmp and a disk-backed $TMPDIR).
        let name = format!("tunemesh-downloads-mem-{}", std::process::id());
        let root = [std::path::PathBuf::from("/var/tmp"), std::env::temp_dir()]
            .into_iter()
            .map(|base| base.join(&name))
            .find(|root| {
                let _ = std::fs::remove_dir_all(root);
                std::fs::create_dir_all(root).is_ok()
            })
            .expect("no writable temporary directory");
        tunemesh_core::hostile::max_requests(&root, MAX_LISTED_REQUESTS).unwrap();
        tunemesh_core::hostile::max_statuses(&root, MAX_LISTED_REQUESTS).unwrap();
        let file_bytes: u64 = [request::requests_dir(&root), request::status_dir(&root)]
            .iter()
            .flat_map(|d| std::fs::read_dir(d).unwrap())
            .map(|e| e.unwrap().metadata().unwrap().len())
            .sum();

        let ((rows, total), kept) = crate::alloc_count::retained(|| load_rows(&root));
        assert_eq!(rows.len(), MAX_LISTED_REQUESTS);
        assert!(total >= MAX_LISTED_REQUESTS);
        assert!(
            rows.iter()
                .all(|r| r.request.is_some() && r.status.is_some())
        );
        // The id three times, both inputs, the request's playlist, device
        // and time, the status's title, message, listed warnings and time,
        // and 1 KiB a row for the structures holding them.
        let per_row = 3 * MAX_ID_BYTES
            + 2 * MAX_INPUT_BYTES
            + MAX_REQUEST_PLAYLIST_BYTES
            + MAX_DEVICE_BYTES
            + 2 * MAX_TIMESTAMP_BYTES
            + (1 + MAX_LISTED_WARNINGS) * MAX_TEXT_BYTES
            + MAX_MESSAGE_BYTES
            + 1024;
        eprintln!(
            "Downloads list: {} rows keep {kept} B from {file_bytes} B of files ({:.4} of the file size; cap {} B)",
            rows.len(),
            kept as f64 / file_bytes as f64,
            MAX_LISTED_REQUESTS * per_row
        );
        assert!(kept <= MAX_LISTED_REQUESTS * per_row, "kept {kept} B");
        assert!(
            (kept as u64) * 20 <= file_bytes,
            "kept {kept} B of {file_bytes} B"
        );
        drop(rows);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn pasted_ids_keep_their_order_without_waiting() {
        let mut ids = OrderedIds::new();
        let started = std::time::Instant::now();
        let v: Vec<String> = (0..1000).map(|_| ids.next()).collect();
        assert!(started.elapsed() < std::time::Duration::from_millis(200));
        let mut sorted = v.clone();
        sorted.sort_by(|a, b| request::order_key(a).cmp(&request::order_key(b)));
        assert_eq!(sorted, v);
        assert!(v.iter().all(|id| {
            let (ms, hex) = id.split_once('-').unwrap();
            ms.parse::<u128>().is_ok() && hex.len() == 8
        }));
    }

    #[test]
    fn classifies_links() {
        for (url, kind) in [
            (
                "https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M",
                Kind::Playlist,
            ),
            (
                "https://open.spotify.com/intl-de/playlist/37i9dQZF1DXcBWIGoYBM5M?si=x",
                Kind::Playlist,
            ),
            (
                "https://open.spotify.com/album/4aawyAB9vmqN3uQ7FjRGTy",
                Kind::Album,
            ),
            (
                "https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC",
                Kind::Single,
            ),
            (
                "https://www.youtube.com/playlist?list=PLx0sYbCqOb8TBPRdmBHs5Iftvv9TPboYG",
                Kind::Playlist,
            ),
            (
                "https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=PLx0sYbCqOb8TBPRdmBHs5Iftvv9TPboYG",
                Kind::Playlist,
            ),
            (
                "https://music.youtube.com/playlist?list=OLAK5uy_kXN0a2yT2vbtFF1dLJTn4wzVzNXZdUDXg",
                Kind::Album,
            ),
            ("https://music.youtube.com/browse/MPREb_abc", Kind::Album),
            (
                "https://music.youtube.com/watch?v=dQw4w9WgXcQ",
                Kind::Single,
            ),
            ("https://youtu.be/dQw4w9WgXcQ", Kind::Single),
            ("https://youtu.be/jNQXAC9IVRw?list=PLx", Kind::Playlist),
            ("https://YOUTU.BE/x?list=PLx", Kind::Playlist),
            ("https://youtu.be?list=PLx", Kind::Playlist),
            ("https://m.youtube.com/watch?v=x&list=PLx", Kind::Playlist),
            (
                "https://music.youtube.com/watch?v=x&list=PLx",
                Kind::Playlist,
            ),
            (
                "https://www.youtube.com/watch?v=jNQXAC9IVRw&list=RDjNQXAC9IVRw",
                Kind::Single,
            ),
            (
                "https://music.youtube.com/watch?v=x&list=RDAMVMx",
                Kind::Single,
            ),
            (
                "https://music.youtube.com/playlist?list=RDCLAK5uy_k",
                Kind::Playlist,
            ),
            ("https://www.youtube.com/watch?v=x&list=", Kind::Single),
            ("https://www.youtube.com/watch?v=x#list=PLx", Kind::Single),
            ("some artist some song", Kind::Single),
        ] {
            assert_eq!(classify(url), kind, "{url}");
        }
    }

    #[test]
    fn splits_pasted_lines() {
        assert_eq!(input_lines("  a \n\n b\na\r\n  "), vec!["a", "b"]);
    }
}
