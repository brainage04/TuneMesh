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
use crate::player::{Change, Player};
use crate::store::{Store, StoreChange};
use crate::ui;

pub const APP_ID: &str = "io.github.brainage04.TuneMesh";

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
            "search" => Route::Search(arg.to_owned()),
            "albums" => Route::Albums,
            "artists" => Route::Artists,
            "playlists" => Route::Playlists,
            "liked" => Route::Playlist(LIKED_SONGS.to_owned()),
            "downloads" => Route::Downloads,
            "settings" => Route::Settings,
            "album" => Route::Album(lib.albums.iter().find(|a| a.name == arg || a.key() == arg)?.key()),
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
    pub toasts: adw::ToastOverlay,
    content: gtk::Box,
    pub search_entry: gtk::SearchEntry,
    back_btn: gtk::Button,
    forward_btn: gtk::Button,
    history: RefCell<Vec<Route>>,
    hist_pos: Cell<usize>,
    scroller: RefCell<Option<gtk::ScrolledWindow>>,
    sidebar: RefCell<Option<Rc<ui::sidebar::Sidebar>>>,
    pub queue_revealer: gtk::Revealer,
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
        let covers = Covers::new(root);

        let window = adw::ApplicationWindow::builder()
            .application(gtk_app)
            .title("TuneMesh")
            .default_width(1280)
            .default_height(820)
            .width_request(900)
            .height_request(560)
            .build();
        window.add_css_class("tunemesh");

        let search_entry = gtk::SearchEntry::builder()
            .placeholder_text("What do you want to play?")
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
            toasts: adw::ToastOverlay::new(),
            content: gtk::Box::new(gtk::Orientation::Vertical, 0),
            search_entry,
            back_btn,
            forward_btn,
            history: RefCell::new(vec![Route::Home]),
            hist_pos: Cell::new(0),
            scroller: RefCell::new(None),
            sidebar: RefCell::new(None),
            queue_revealer: gtk::Revealer::builder()
                .transition_type(gtk::RevealerTransitionType::SlideLeft)
                .transition_duration(180)
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
        self.queue_revealer.set_child(Some(&queue.widget));
        // A collapsed revealer still claims its child's width; hide it once closed.
        self.queue_revealer.set_visible(false);
        self.queue_revealer.connect_child_revealed_notify(|r| {
            if !r.is_child_revealed() && !r.reveals_child() {
                r.set_visible(false);
            }
        });
        *self.queue_panel.borrow_mut() = Some(queue);

        let body = gtk::Box::builder().spacing(8).css_classes(["tm-body"]).vexpand(true).build();
        body.append(&sidebar.widget);
        body.append(&self.toasts);
        body.append(&self.queue_revealer);

        let bar = ui::nowplaying::NowPlaying::new(self);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&header);
        root.append(&body);
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
                let typing = focus.is_some_and(|w| w.is::<gtk::Text>() || w.ancestor(gtk::Text::static_type()).is_some());
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
            app.player.shutdown();
            glib::Propagation::Proceed
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
        add("focus-search", None, Box::new(|app, _| {
            app.navigate(Route::Search(app.search_entry.text().to_string()));
            app.search_entry.grab_focus();
        }));
        add("quit", None, Box::new(|app, _| {
            app.player.shutdown();
            app.gtk_app.quit();
        }));
        add("navigate", Some(glib::VariantTy::STRING), Box::new(|app, v| {
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
        }));
        add("play-route", Some(glib::VariantTy::STRING), Box::new(|app, v| {
            let s: String = v.and_then(|v| v.get()).unwrap_or_default();
            if let Some(r) = Route::parse(app, &s) {
                let (name, tracks) = ui::pages::route_tracks(app, &r);
                app.player.play_tracks(&name, tracks, 0, Some(false));
            }
        }));
        add("seek", Some(glib::VariantTy::DOUBLE), Box::new(|app, v| {
            if let Some(secs) = v.and_then(|v| v.get::<f64>()) {
                app.player.seek(secs);
            }
        }));
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
            Route::Search(q) if self.search_entry.text() != q.as_str() => self.search_entry.set_text(q),
            Route::Search(_) => {}
            _ if !self.search_entry.text().is_empty() => self.search_entry.set_text(""),
            _ => {}
        }
        self.set_page(ui::pages::build(&self.rc(), &route));
        self.back_btn.set_sensitive(self.hist_pos.get() > 0);
        self.forward_btn.set_sensitive(self.hist_pos.get() + 1 < self.history.borrow().len());
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
    }

    /// Rebuilds the current page, keeping its scroll position.
    pub fn refresh(&self) {
        let value = self.scroller.borrow().as_ref().map(|s| s.vadjustment().value());
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
                if matches!(&route, Route::Playlist(n) if n == LIKED_SONGS) || matches!(route, Route::Home | Route::Playlists) {
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
        let lists: Vec<_> = self.track_lists.borrow().iter().filter_map(Weak::upgrade).collect();
        for l in lists {
            l.refresh_rows(self);
        }
    }

    pub fn toggle_queue(&self) {
        let reveal = !self.queue_revealer.reveals_child();
        if reveal {
            self.queue_revealer.set_visible(true);
        }
        self.queue_revealer.set_reveal_child(reveal);
        if reveal
            && let Some(q) = self.queue_panel.borrow().as_ref() {
                q.rebuild();
            }
    }

    pub fn toast(&self, text: &str) {
        let toast = adw::Toast::new(text);
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
        self.window.present();
    }
}
