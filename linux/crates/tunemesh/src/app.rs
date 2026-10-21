//! The application object: window layout, navigation, shared services.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use tunemesh_core::LIKED_SONGS;

use crate::config::Config;
use crate::covers::Covers;
use crate::downloader::Downloader;
use crate::player::{Change, Player};
use crate::store::{Store, StoreChange};
use crate::ui;

/// Longest search text kept; longer input (typed, pasted or sent over
/// D-Bus) is cut here.
pub const MAX_SEARCH_CHARS: usize = 200;

pub fn cap_search(text: &str) -> String {
    text.chars().take(MAX_SEARCH_CHARS).collect()
}

pub const APP_ID: &str = "io.github.brainage04.TuneMesh";

/// Narrowest window, in pixels: about a tiled half of a 1366-pixel screen.
const MIN_WIDTH: i32 = 600;
/// Lowest window, in pixels: fits a 1280 x 800 screen at 200 % scale
/// (400 logical pixels) under a title bar.
const MIN_HEIGHT: i32 = 360;
/// Up to this width (sp) the queue opens over the page instead of beside it.
const QUEUE_BESIDE: u32 = 1180;
/// Up to this width (sp) the library sidebar folds away as well.
const SIDEBAR_BESIDE: u32 = 900;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Home,
    Search(String),
    Albums,
    Artists,
    Playlists,
    /// Album key (`artist \x1f album`).
    Album(String),
    Artist(String),
    /// Playlist name; Liked Songs included.
    Playlist(String),
    Downloads,
    Settings,
}

impl Route {
    /// Parses the textual form used by the `app.navigate` action.
    pub fn parse(app: &App, s: &str) -> Option<Route> {
        let (kind, arg) = s.split_once(':').unwrap_or((s, ""));
        let lib = app.store.library();
        Some(match kind {
            "home" => Route::Home,
            "search" => Route::Search(cap_search(arg)),
            "albums" => Route::Albums,
            "artists" => Route::Artists,
            "playlists" => Route::Playlists,
            "liked" => Route::Playlist(LIKED_SONGS.to_owned()),
            "downloads" => Route::Downloads,
            "settings" => Route::Settings,
            "album" => Route::Album(
                lib.albums
                    .iter()
                    .find(|a| a.name == arg || a.key() == arg)?
                    .key(),
            ),
            "artist" => Route::Artist(arg.to_owned()),
            "playlist" => Route::Playlist(arg.to_owned()),
            _ => return None,
        })
    }
}

/// A built page: its root widget and the scroller whose position survives
/// rebuilds.
pub struct Page {
    pub widget: gtk::Widget,
    pub scroller: Option<gtk::ScrolledWindow>,
}

pub struct App {
    pub me: Weak<App>,
    pub gtk_app: adw::Application,
    pub window: adw::ApplicationWindow,
    pub config: RefCell<Config>,
    pub store: Rc<Store>,
    pub player: Rc<Player>,
    pub covers: Rc<Covers>,
    pub downloader: Rc<Downloader>,
    /// Held while the window is closed but downloads are still running.
    background: RefCell<Option<gio::ApplicationHoldGuard>>,
    background_timer: RefCell<Option<glib::SourceId>>,
    pub toasts: adw::ToastOverlay,
    content: gtk::Box,
    pub search_entry: gtk::SearchEntry,
    back_btn: gtk::Button,
    forward_btn: gtk::Button,
    history: RefCell<Vec<Route>>,
    hist_pos: Cell<usize>,
    scroller: RefCell<Option<gtk::ScrolledWindow>>,
    sidebar: RefCell<Option<Rc<ui::sidebar::Sidebar>>>,
    /// The queue panel beside (or, in a narrow window, over) the page.
    pub queue_split: adw::OverlaySplitView,
    /// The library sidebar beside (or, in a narrow window, over) the rest.
    library_split: adw::OverlaySplitView,
    queue_panel: RefCell<Option<Rc<ui::queue::QueuePanel>>>,
    /// Live track lists, refreshed in place on like/now-playing changes.
    track_lists: RefCell<Vec<Weak<ui::tracklist::Shared>>>,
    search_timer: RefCell<Option<glib::SourceId>>,
}

impl App {
    pub fn new(gtk_app: &adw::Application, config: Config) -> Rc<Self> {
        let root = config.library_path.clone();
        let store = Store::new(root.clone());
        let player = Player::new(root.clone());
        let covers = Covers::new(root.clone());
        let downloader = Downloader::new(root, config.downloader);

        let window = adw::ApplicationWindow::builder()
            .application(gtk_app)
            .title("TuneMesh")
            .default_width(1280)
            .default_height(820)
            // Narrow windows (a tiled half screen) fold the sidebar and the
            // queue over the page; see the breakpoints in build_window.
            .width_request(MIN_WIDTH)
            .height_request(MIN_HEIGHT)
            .build();
        window.add_css_class("tunemesh");

        let search_entry = gtk::SearchEntry::builder()
            .max_width_chars(40)
            .placeholder_text("Search your library")
            .css_classes(["tm-search"])
            .build();
        let back_btn = gtk::Button::builder()
            .icon_name("go-previous-symbolic")
            .tooltip_text("Go back")
            .css_classes(["tm-nav-circle", "flat"])
            .sensitive(false)
            .build();
        let forward_btn = gtk::Button::builder()
            .icon_name("go-next-symbolic")
            .tooltip_text("Go forward")
            .css_classes(["tm-nav-circle", "flat"])
            .sensitive(false)
            .build();

        let app = Rc::new_cyclic(|me| App {
            me: me.clone(),
            gtk_app: gtk_app.clone(),
            window,
            config: RefCell::new(config),
            store,
            player,
            covers,
            downloader,
            background: RefCell::new(None),
            background_timer: RefCell::new(None),
            toasts: adw::ToastOverlay::new(),
            content: gtk::Box::new(gtk::Orientation::Vertical, 0),
            search_entry,
            back_btn,
            forward_btn,
            history: RefCell::new(vec![Route::Home]),
            hist_pos: Cell::new(0),
            scroller: RefCell::new(None),
            sidebar: RefCell::new(None),
            queue_split: adw::OverlaySplitView::builder()
                .sidebar_position(gtk::PackType::End)
                // The panes' contents take the width the split view gives
                // them; a wider minimum inside would overflow the pane.
                .min_sidebar_width(300.0)
                .max_sidebar_width(340.0)
                .show_sidebar(false)
                .build(),
            library_split: adw::OverlaySplitView::builder()
                .min_sidebar_width(220.0)
                .max_sidebar_width(280.0)
                .build(),
            queue_panel: RefCell::new(None),
            track_lists: RefCell::new(Vec::new()),
            search_timer: RefCell::new(None),
        });
        app.build_window();
        app.connect_signals();
        app.install_actions();
        app.show_current();
        app
    }

    pub fn rc(&self) -> Rc<App> {
        self.me.upgrade().expect("app alive")
    }

    fn build_window(self: &Rc<Self>) {
        let header = adw::HeaderBar::builder().css_classes(["tm-header"]).build();
        let nav = gtk::Box::builder().spacing(8).margin_start(8).build();
        nav.append(&self.back_btn);
        nav.append(&self.forward_btn);
        header.pack_start(&nav);
        let home_btn = gtk::Button::builder()
            .icon_name("go-home-symbolic")
            .tooltip_text("Home")
            .css_classes(["tm-nav-circle", "flat"])
            .build();
        let center = gtk::Box::builder().spacing(8).build();
        center.append(&home_btn);
        center.append(&self.search_entry);
        header.set_title_widget(Some(&center));
        let app = self.clone();
        home_btn.connect_clicked(move |_| app.navigate(Route::Home));

        let sidebar = ui::sidebar::Sidebar::new(self);
        *self.sidebar.borrow_mut() = Some(sidebar.clone());

        self.content.set_hexpand(true);
        self.content.set_vexpand(true);
        self.content.add_css_class("tm-panel");
        self.content.set_overflow(gtk::Overflow::Hidden);
        self.toasts.set_child(Some(&self.content));
        self.toasts.set_hexpand(true);
        self.toasts.set_vexpand(true);

        let queue = ui::queue::QueuePanel::new(self);
        queue.widget.set_margin_start(8);
        self.queue_split
            .set_sidebar(Some(&shrinkable(&queue.widget)));
        self.queue_split.set_content(Some(&self.toasts));
        *self.queue_panel.borrow_mut() = Some(queue);

        sidebar.widget.set_margin_end(8);
        self.library_split
            .set_sidebar(Some(&shrinkable(&sidebar.widget)));
        self.library_split.set_content(Some(&self.queue_split));
        self.library_split.add_css_class("tm-body");
        self.library_split.set_vexpand(true);
        let body = &self.library_split;

        // In a narrow window the sidebar folds away behind this button.
        let sidebar_btn = gtk::ToggleButton::builder()
            .icon_name("sidebar-show-symbolic")
            .tooltip_text("Your Library")
            .css_classes(["tm-nav-circle", "flat"])
            .build();
        self.library_split
            .bind_property("show-sidebar", &sidebar_btn, "active")
            .bidirectional()
            .sync_create()
            .build();
        self.library_split
            .bind_property("collapsed", &sidebar_btn, "visible")
            .sync_create()
            .build();
        header.pack_start(&sidebar_btn);

        let wide_queue = adw::Breakpoint::new(
            adw::BreakpointCondition::parse(&format!("max-width: {QUEUE_BESIDE}sp"))
                .expect("condition"),
        );
        wide_queue.add_setter(&self.queue_split, "collapsed", Some(&true.to_value()));
        self.window.add_breakpoint(wide_queue);
        let narrow = adw::Breakpoint::new(
            adw::BreakpointCondition::parse(&format!("max-width: {SIDEBAR_BESIDE}sp"))
                .expect("condition"),
        );
        narrow.add_setter(&self.queue_split, "collapsed", Some(&true.to_value()));
        narrow.add_setter(&self.library_split, "collapsed", Some(&true.to_value()));

        let bar = ui::nowplaying::NowPlaying::new(self);
        // Narrow: the slider goes; the volume button still mutes and unmutes.
        narrow.add_setter(&bar.volume, "visible", Some(&false.to_value()));
        self.window.add_breakpoint(narrow);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&header);
        root.append(body);
        root.append(&bar.widget);
        self.window.set_content(Some(&root));
        drop(bar);
    }

    fn connect_signals(self: &Rc<Self>) {
        let app = self.clone();
        self.back_btn.connect_clicked(move |_| app.back());
        let app = self.clone();
        self.forward_btn.connect_clicked(move |_| app.forward());

        let app = self.clone();
        self.search_entry.connect_search_changed(move |e| {
            let text = e.text().to_string();
            app.on_search_text(text);
        });
        let app = self.clone();
        self.search_entry.connect_activate(move |e| {
            let text = e.text().to_string();
            app.on_search_text(text);
        });
        let app = self.clone();
        let click = gtk::GestureClick::new();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        click.connect_pressed(move |_, _, _, _| {
            if !matches!(app.route(), Route::Search(_)) {
                let text = app.search_entry.text().to_string();
                app.navigate(Route::Search(text));
            }
        });
        self.search_entry.add_controller(click);

        let weak = Rc::downgrade(self);
        self.store.connect(move |change| {
            let Some(app) = weak.upgrade() else { return };
            app.on_store_change(change);
        });
        let weak = Rc::downgrade(self);
        self.player.connect(move |change| {
            let Some(app) = weak.upgrade() else { return };
            if matches!(change, Change::Track) {
                app.refresh_track_rows();
                if let Some(s) = app.sidebar.borrow().as_ref() {
                    s.update_playing();
                }
            }
        });

        // Space toggles playback unless a text field has focus.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let app = self.clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            if key == gdk::Key::space && mods.is_empty() {
                let focus = gtk::prelude::GtkWindowExt::focus(&app.window);
                let typing = focus.is_some_and(|w| {
                    w.is::<gtk::Text>() || w.ancestor(gtk::Text::static_type()).is_some()
                });
                if !typing {
                    app.player.toggle();
                    return glib::Propagation::Stop;
                }
            }
            glib::Propagation::Proceed
        });
        self.window.add_controller(keys);

        let app = self.clone();
        self.window.connect_close_request(move |_| {
            if app.background.borrow().is_some() {
                return glib::Propagation::Stop;
            }
            if app.downloader.has_pending_work() {
                app.ask_background();
                return glib::Propagation::Stop;
            }
            app.player.shutdown();
            glib::Propagation::Proceed
        });

        let weak = Rc::downgrade(self);
        self.downloader.connect(move || {
            if weak.upgrade().is_some() {
                ui::downloads::refresh_status();
            }
        });
    }

    fn install_actions(self: &Rc<Self>) {
        let a = &self.gtk_app;
        type Handler = Box<dyn Fn(&App, Option<&glib::Variant>)>;
        let add = |name: &str, param: Option<&glib::VariantTy>, f: Handler| {
            let action = gio::SimpleAction::new(name, param);
            let weak = Rc::downgrade(self);
            action.connect_activate(move |_, v| {
                if let Some(app) = weak.upgrade() {
                    f(&app, v);
                }
            });
            a.add_action(&action);
        };
        add("play-pause", None, Box::new(|app, _| app.player.toggle()));
        add("next", None, Box::new(|app, _| app.player.next()));
        add("previous", None, Box::new(|app, _| app.player.previous()));
        add("back", None, Box::new(|app, _| app.back()));
        add("toggle-queue", None, Box::new(|app, _| app.toggle_queue()));
        add(
            "focus-search",
            None,
            Box::new(|app, _| {
                app.navigate(Route::Search(app.search_entry.text().to_string()));
                app.search_entry.grab_focus();
            }),
        );
        add("quit", None, Box::new(|app, _| app.quit()));
        add(
            "setup",
            None,
            Box::new(|app, _| {
                app.present();
                ui::setup::open(&app.rc());
            }),
        );
        add(
            "navigate",
            Some(glib::VariantTy::STRING),
            Box::new(|app, v| {
                let s: String = v.and_then(|v| v.get()).unwrap_or_default();
                match Route::parse(app, &s) {
                    Some(r) => {
                        if let Route::Search(q) = &r {
                            app.search_entry.set_text(q);
                        }
                        app.navigate(r)
                    }
                    None => eprintln!("tunemesh: unknown route {s:?}"),
                }
            }),
        );
        add(
            "play-route",
            Some(glib::VariantTy::STRING),
            Box::new(|app, v| {
                let s: String = v.and_then(|v| v.get()).unwrap_or_default();
                if let Some(r) = Route::parse(app, &s) {
                    let (name, tracks) = ui::pages::route_tracks(app, &r);
                    app.player.play_tracks(&name, tracks, 0, Some(false));
                }
            }),
        );
        add(
            "seek",
            Some(glib::VariantTy::DOUBLE),
            Box::new(|app, v| {
                if let Some(secs) = v.and_then(|v| v.get::<f64>()) {
                    app.player.seek(secs);
                }
            }),
        );
        a.set_accels_for_action("app.focus-search", &["<Control>f", "<Control>k"]);
        a.set_accels_for_action("app.next", &["<Control>Right"]);
        a.set_accels_for_action("app.previous", &["<Control>Left"]);
        a.set_accels_for_action("app.back", &["<Alt>Left"]);
        a.set_accels_for_action("app.toggle-queue", &["<Control><Alt>q"]);
        a.set_accels_for_action("app.quit", &["<Control>q"]);
    }

    // ---- navigation --------------------------------------------------

    pub fn route(&self) -> Route {
        self.history.borrow()[self.hist_pos.get()].clone()
    }

    pub fn navigate(&self, route: Route) {
        // Picking a page in the folded sidebar closes it.
        if self.library_split.is_collapsed() {
            self.library_split.set_show_sidebar(false);
        }
        if route == self.route() {
            return;
        }
        {
            let mut h = self.history.borrow_mut();
            let pos = self.hist_pos.get();
            h.truncate(pos + 1);
            h.push(route);
            self.hist_pos.set(pos + 1);
        }
        self.show_current();
    }

    /// Replaces the current history entry (used while typing a search).
    fn replace(&self, route: Route) {
        self.history.borrow_mut()[self.hist_pos.get()] = route;
        self.show_current();
    }

    pub fn back(&self) {
        if self.hist_pos.get() > 0 {
            self.hist_pos.set(self.hist_pos.get() - 1);
            self.show_current();
        }
    }

    pub fn forward(&self) {
        if self.hist_pos.get() + 1 < self.history.borrow().len() {
            self.hist_pos.set(self.hist_pos.get() + 1);
            self.show_current();
        }
    }

    fn on_search_text(&self, text: String) {
        let text = cap_search(&text);
        if let Some(id) = self.search_timer.borrow_mut().take() {
            id.remove();
        }
        // Clearing the box when leaving the search page must not navigate back.
        if text.is_empty() && !matches!(self.route(), Route::Search(_)) {
            return;
        }
        let weak = self.me.clone();
        let id = glib::timeout_add_local_once(Duration::from_millis(120), move || {
            let Some(app) = weak.upgrade() else { return };
            app.search_timer.borrow_mut().take();
            match app.route() {
                Route::Search(old) if old == text => {}
                Route::Search(_) => app.replace(Route::Search(text)),
                _ => app.navigate(Route::Search(text)),
            }
        });
        *self.search_timer.borrow_mut() = Some(id);
    }

    fn show_current(&self) {
        let route = self.route();
        match &route {
            Route::Search(q) if self.search_entry.text() != q.as_str() => {
                self.search_entry.set_text(q)
            }
            Route::Search(_) => {}
            _ if !self.search_entry.text().is_empty() => self.search_entry.set_text(""),
            _ => {}
        }
        self.set_page(ui::pages::build(&self.rc(), &route));
        self.back_btn.set_sensitive(self.hist_pos.get() > 0);
        self.forward_btn
            .set_sensitive(self.hist_pos.get() + 1 < self.history.borrow().len());
        if let Some(s) = self.sidebar.borrow().as_ref() {
            s.set_active(&route);
        }
    }

    fn set_page(&self, page: Page) {
        while let Some(child) = self.content.first_child() {
            self.content.remove(&child);
        }
        page.widget.set_vexpand(true);
        self.content.append(&page.widget);
        *self.scroller.borrow_mut() = page.scroller;
        trim_heap_soon();
    }

    /// Rebuilds the current page, keeping its scroll position.
    pub fn refresh(&self) {
        let value = self
            .scroller
            .borrow()
            .as_ref()
            .map(|s| s.vadjustment().value());
        self.set_page(ui::pages::build(&self.rc(), &self.route()));
        if let (Some(v), Some(s)) = (value, self.scroller.borrow().clone()) {
            // The new page has no size yet; restore once laid out.
            glib::idle_add_local_once(move || s.vadjustment().set_value(v));
        }
    }

    fn on_store_change(&self, change: StoreChange) {
        let route = self.route();
        match change {
            StoreChange::Library => {
                self.covers.invalidate();
                if !matches!(route, Route::Downloads | Route::Settings) {
                    self.refresh();
                }
                if let Some(s) = self.sidebar.borrow().as_ref() {
                    s.rebuild();
                }
                if let Some(q) = self.queue_panel.borrow().as_ref() {
                    q.rebuild();
                }
            }
            StoreChange::Playlists => {
                let affected = match &route {
                    Route::Home | Route::Playlists | Route::Search(_) => true,
                    Route::Playlist(name) => name != LIKED_SONGS,
                    _ => false,
                };
                if affected {
                    self.refresh();
                }
                if let Some(s) = self.sidebar.borrow().as_ref() {
                    s.rebuild();
                }
            }
            StoreChange::Liked => {
                if matches!(&route, Route::Playlist(n) if n == LIKED_SONGS)
                    || matches!(route, Route::Home | Route::Playlists)
                {
                    self.refresh();
                }
                self.refresh_track_rows();
                if let Some(s) = self.sidebar.borrow().as_ref() {
                    s.rebuild();
                }
            }
            StoreChange::Downloads => ui::downloads::refresh_if_shown(),
            StoreChange::Scanning => {
                if route == Route::Settings {
                    self.refresh();
                }
            }
        }
    }

    // ---- shared helpers ------------------------------------------------

    pub fn register_track_list(&self, shared: &Rc<ui::tracklist::Shared>) {
        let mut v = self.track_lists.borrow_mut();
        v.retain(|w| w.strong_count() > 0);
        v.push(Rc::downgrade(shared));
    }

    pub fn refresh_track_rows(&self) {
        let lists: Vec<_> = self
            .track_lists
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .collect();
        for l in lists {
            l.refresh_rows(self);
        }
    }

    pub fn queue_shown(&self) -> bool {
        self.queue_split.shows_sidebar()
    }

    pub fn toggle_queue(&self) {
        let reveal = !self.queue_shown();
        self.queue_split.set_show_sidebar(reveal);
        if reveal && let Some(q) = self.queue_panel.borrow().as_ref() {
            q.rebuild();
        }
    }

    pub fn toast(&self, text: &str) {
        let toast = adw::Toast::new(&tunemesh_core::limits::clip_line(text));
        // Toasts are markup by default; messages carry playlist names, paths
        // and error text from synced files.
        toast.set_use_markup(false);
        toast.set_timeout(3);
        self.toasts.add_toast(toast);
    }

    pub fn error(&self, context: &str, e: impl std::fmt::Display) {
        eprintln!("tunemesh: {context}: {e}");
        self.toast(&format!("{context}: {e}"));
    }

    /// Switches to another library folder and remembers it.
    pub fn set_library_path(&self, path: PathBuf) {
        let mut config = self.config.borrow_mut();
        config.library_path = path.clone();
        if let Err(e) = config.save() {
            drop(config);
            self.error("Could not save settings", e);
        } else {
            drop(config);
        }
        self.covers.set_root(path.clone());
        self.player.set_root(path.clone());
        self.downloader.set_root(path.clone());
        self.store.set_root(path);
        self.toast("Library folder changed");
    }

    /// Points history entries at a renamed playlist and re-shows the page.
    pub fn playlist_renamed(&self, old: &str, new: &str) {
        for r in self.history.borrow_mut().iter_mut() {
            if *r == Route::Playlist(old.to_owned()) {
                *r = Route::Playlist(new.to_owned());
            }
        }
        self.show_current();
    }

    pub fn present(&self) {
        self.leave_background();
        self.window.present();
    }

    /// Saves playback state, stops the downloader and quits.
    pub fn quit(&self) {
        self.leave_background();
        self.player.shutdown();
        self.downloader.stop_blocking();
        self.gtk_app.quit();
    }

    /// Turns the built-in downloader on or off and remembers it.
    pub fn set_downloader(&self, on: bool) {
        if self.config.borrow().downloader != on {
            let mut config = self.config.borrow_mut();
            config.downloader = on;
            let saved = config.save();
            drop(config);
            if let Err(e) = saved {
                self.error("Could not save settings", e);
            }
        }
        self.downloader.set_enabled(on);
    }

    // ---- running in the background --------------------------------------

    /// The window is being closed while this device still has downloads to
    /// finish: offer to keep downloading in the background.
    fn ask_background(&self) {
        let n = self.downloader.pending_count();
        let dialog = adw::AlertDialog::new(
            Some("Downloads are still running"),
            Some(&format!(
                "{} left. TuneMesh can keep downloading in the background and quit when the queue is empty, or stop now and continue next time.",
                ui::widgets::plural(n, "download is", "downloads are")
            )),
        );
        dialog.add_responses(&[
            ("quit", "Stop and Quit"),
            ("background", "Keep Downloading"),
        ]);
        dialog.set_response_appearance("background", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("background"));
        dialog.set_close_response("cancel");
        let weak = self.me.clone();
        dialog.connect_response(None, move |_, response| {
            let Some(app) = weak.upgrade() else { return };
            match response {
                "background" => app.enter_background(),
                "quit" => app.quit(),
                _ => {}
            }
        });
        dialog.present(Some(&self.window));
    }

    fn enter_background(&self) {
        if self.background.borrow().is_some() {
            return;
        }
        self.player.pause();
        self.player.save();
        *self.background.borrow_mut() = Some(self.gtk_app.hold());
        self.window.set_visible(false);
        let n = self.downloader.pending_count();
        let note = gio::Notification::new("TuneMesh is downloading in the background");
        note.set_body(Some(&format!(
            "It quits when the {} finished. Open TuneMesh to see progress.",
            ui::widgets::plural(n, "download has", "downloads have")
        )));
        note.add_button("Stop and Quit", "app.quit");
        self.gtk_app.send_notification(Some("background"), &note);
        let weak = self.me.clone();
        let id = glib::timeout_add_seconds_local(3, move || {
            let Some(app) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if app.downloader.has_pending_work() {
                return glib::ControlFlow::Continue;
            }
            app.background_timer.borrow_mut().take();
            app.quit();
            glib::ControlFlow::Break
        });
        *self.background_timer.borrow_mut() = Some(id);
    }

    fn leave_background(&self) {
        if let Some(id) = self.background_timer.borrow_mut().take() {
            id.remove();
        }
        if self.background.borrow_mut().take().is_some() {
            self.gtk_app.withdraw_notification("background");
        }
    }
}

/// `child` in a pane that may be allocated smaller than the child's minimum
/// (the window's minimum height leaves a short pane on a 400-pixel screen).
/// While a split view animates its sidebar (or the window shrinks past a
/// breakpoint) it briefly gives the pane less than that, which GTK reports
/// as a critical; the clipped frame is never seen.
fn shrinkable(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::External)
        .vscrollbar_policy(gtk::PolicyType::External)
        .propagate_natural_width(true)
        .propagate_natural_height(true)
        .child(child)
        .build()
}

/// Returns the memory freed by the pages just left to the system, a moment
/// after a page change. glibc keeps freed heap memory for reuse; with large
/// pages (a 40,000-track album, 200 download requests) that is hundreds of
/// megabytes the process would hold until exit.
fn trim_heap_soon() {
    thread_local! {
        static PENDING: Cell<bool> = const { Cell::new(false) };
    }
    if PENDING.with(|p| p.replace(true)) {
        return;
    }
    glib::timeout_add_seconds_local_once(2, || {
        PENDING.with(|p| p.set(false));
        // SAFETY: malloc_trim only returns free pages; no allocation is touched.
        unsafe { libc::malloc_trim(0) };
    });
}
