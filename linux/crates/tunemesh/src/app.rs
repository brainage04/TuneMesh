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
use crate::player::{Change, Changes, Player};
use crate::store::{Store, StoreChange};
use crate::ui;

/// Longest search text kept; longer input (typed, pasted or sent over
/// D-Bus) is cut here.
pub const MAX_SEARCH_CHARS: usize = 200;

pub fn cap_search(text: &str) -> String {
    text.chars().take(MAX_SEARCH_CHARS).collect()
}

pub const APP_ID: &str = "io.github.brainage04.TuneMesh";

/// The app's colour scheme, style sheets and icons, for the default display
/// (on application startup).
pub fn load_style() {
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
    let display = gdk::Display::default().expect("display");
    // libadwaita 1.6 moved its named colours to CSS variables; 1.5 still
    // uses @define-color.
    let colors = if adw::minor_version() >= 6 {
        "colors.css"
    } else {
        "colors-adw15.css"
    };
    for name in [colors, "style.css"] {
        let provider = gtk::CssProvider::new();
        provider.load_from_resource(&format!("/io/github/brainage04/TuneMesh/{name}"));
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
    gtk::IconTheme::for_display(&display).add_resource_path("/io/github/brainage04/TuneMesh/icons");
    gtk::Window::set_default_icon_name(APP_ID);
}

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

    /// Which pages stay alive when left, so that going back to them is a
    /// matter of showing them again: the library's top pages, one detail
    /// page (album, artist or playlist) and the search page.
    fn kept(&self) -> Option<Kept> {
        match self {
            Route::Home | Route::Albums | Route::Artists | Route::Playlists => Some(Kept::Top),
            Route::Album(_) | Route::Artist(_) | Route::Playlist(_) => Some(Kept::Detail),
            Route::Search(_) => Some(Kept::Search),
            Route::Downloads | Route::Settings => None,
        }
    }

    /// Whether `change` changes what the page of this route shows.
    fn shows(&self, change: StoreChange) -> bool {
        match change {
            StoreChange::Library => !matches!(self, Route::Downloads | Route::Settings),
            StoreChange::Playlists => match self {
                Route::Home | Route::Playlists | Route::Search(_) => true,
                Route::Playlist(name) => name != LIKED_SONGS,
                _ => false,
            },
            StoreChange::Liked => {
                matches!(self, Route::Playlist(n) if n == LIKED_SONGS)
                    || matches!(self, Route::Home | Route::Playlists | Route::Search(_))
            }
            StoreChange::Scanning => *self == Route::Settings,
            StoreChange::Downloads => false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kept {
    Top,
    Detail,
    Search,
}

/// Brings a page up to date in place after a store change; `false` when it
/// cannot, and the page is built again.
pub type Update = Rc<dyn Fn(&Rc<App>, StoreChange) -> bool>;
/// Shows another route of the same kind in place (the search page, for a
/// new query); `false` when it cannot.
pub type Retarget = Rc<dyn Fn(&Rc<App>, &Route) -> bool>;
/// Takes a chunk of a page's rows away; `true` once none are left.
pub type Drain = Rc<dyn Fn() -> bool>;

/// A built page: its root widget, the scroller whose position survives
/// rebuilds, and how it changes without being built again.
pub struct Page {
    pub widget: gtk::Widget,
    pub scroller: Option<gtk::ScrolledWindow>,
    pub update: Option<Update>,
    pub retarget: Option<Retarget>,
    pub drain: Option<Drain>,
}

impl Page {
    /// A page that is built again whenever what it shows changes.
    pub fn plain(widget: impl IsA<gtk::Widget>, scroller: Option<gtk::ScrolledWindow>) -> Self {
        Self {
            widget: widget.upcast(),
            scroller,
            update: None,
            retarget: None,
            drain: None,
        }
    }
}

/// A page in the window: shown, or kept for when its route is shown again.
struct Slot {
    route: Route,
    page: Page,
    /// Store changes since the page was last brought up to date (while it
    /// was hidden).
    pending: Vec<StoreChange>,
}

/// Closing the window while the downloader is on.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CloseCheck {
    Idle,
    /// The pending requests are being counted.
    Counting,
    /// None are left: the window closes.
    Closing,
}

/// Shows the player's changes in part of the window.
type PlayerView = Rc<dyn Fn(Changes)>;

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
    /// Where closing the window stands: the pending downloads are counted
    /// on a thread before it closes.
    close_check: Cell<CloseCheck>,
    /// Set while the background mode's pending count runs.
    background_check: Cell<bool>,
    pub toasts: adw::ToastOverlay,
    content: gtk::Box,
    pub search_entry: gtk::SearchEntry,
    back_btn: gtk::Button,
    forward_btn: gtk::Button,
    history: RefCell<Vec<Route>>,
    hist_pos: Cell<usize>,
    /// The pages in the window: the one shown (`shown`) and those kept.
    slots: RefCell<Vec<Slot>>,
    shown: RefCell<Option<Route>>,
    /// Pages left for good, emptied a chunk per frame and then removed.
    retired: RefCell<Vec<Page>>,
    retiring: Cell<bool>,
    sidebar: RefCell<Option<Rc<ui::sidebar::Sidebar>>>,
    /// The queue panel beside (or, in a narrow window, over) the page.
    pub queue_split: adw::OverlaySplitView,
    /// The library sidebar beside (or, in a narrow window, over) the rest.
    library_split: adw::OverlaySplitView,
    queue_panel: RefCell<Option<Rc<ui::queue::QueuePanel>>>,
    /// Live track lists, refreshed in place on like/now-playing changes.
    track_lists: RefCell<Vec<Weak<ui::tracklist::Shared>>>,
    search_timer: RefCell<Option<glib::SourceId>>,
    /// Player changes not shown yet, and who shows them (once a frame).
    player_changes: Cell<Changes>,
    player_frame: RefCell<Option<ui::widgets::PerFrame>>,
    player_views: RefCell<Vec<PlayerView>>,
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
        // Drawn on the CPU (no GPU, or GSK_RENDERER=cairo): see `.tm-software`
        // in style.css (no shadows, no CSS transitions). Animations are off
        // (a dialog's scale animation took 18-24 ms a frame) and covers come
        // with their rounded corners drawn in, instead of clipped at every
        // redraw.
        let c = covers.clone();
        window.connect_realize(move |w| {
            if w.renderer()
                .is_some_and(|r| r.is::<gtk::gsk::CairoRenderer>())
            {
                w.add_css_class("tm-software");
                gtk::Settings::for_display(&WidgetExt::display(w)).set_gtk_enable_animations(false);
                c.set_software(true);
            }
        });

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
            close_check: Cell::new(CloseCheck::Idle),
            background_check: Cell::new(false),
            toasts: adw::ToastOverlay::new(),
            content: gtk::Box::new(gtk::Orientation::Vertical, 0),
            search_entry,
            back_btn,
            forward_btn,
            history: RefCell::new(vec![Route::Home]),
            hist_pos: Cell::new(0),
            slots: RefCell::new(Vec::new()),
            shown: RefCell::new(None),
            retired: RefCell::new(Vec::new()),
            retiring: Cell::new(false),
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
            player_changes: Cell::new(Changes::default()),
            player_frame: RefCell::new(None),
            player_views: RefCell::new(Vec::new()),
        });
        app.build_window();
        app.connect_signals();
        app.install_actions();
        // The renderer decides how covers are made (see above): known once
        // the window is realized, before the first page asks for any.
        WidgetExt::realize(&app.window);
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
        let weak = Rc::downgrade(&queue);
        self.connect_player(move |changes| {
            if (changes.has(Change::Queue) || changes.has(Change::Track))
                && let Some(q) = weak.upgrade()
            {
                q.rebuild();
            }
        });
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
        // Player changes reach the window once a frame: a queue of files
        // mpv cannot play fails through hundreds a second, and each change
        // rebuilt rows and set the window title.
        let weak = Rc::downgrade(self);
        *self.player_frame.borrow_mut() =
            Some(ui::widgets::PerFrame::new(&self.window, move || {
                let Some(app) = weak.upgrade() else { return };
                let changes = app.player_changes.take();
                let views: Vec<_> = app.player_views.borrow().clone();
                for v in views {
                    v(changes);
                }
            }));
        let weak = Rc::downgrade(self);
        self.player.connect(move |change| {
            let Some(app) = weak.upgrade() else { return };
            let mut changes = app.player_changes.get();
            changes.add(change);
            app.player_changes.set(changes);
            if let Some(f) = app.player_frame.borrow().as_ref() {
                f.request();
            }
        });
        let weak = Rc::downgrade(self);
        self.connect_player(move |changes| {
            let Some(app) = weak.upgrade() else { return };
            if changes.has(Change::Track) {
                app.mark_current_rows();
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
            match app.close_check.get() {
                CloseCheck::Counting => return glib::Propagation::Stop,
                CloseCheck::Idle if app.downloader.is_on() => {
                    // Counting the requests reads a folder another device
                    // fills: never on the main loop.
                    app.close_check.set(CloseCheck::Counting);
                    let weak = app.me.clone();
                    app.downloader.pending_work(move |n| {
                        let Some(app) = weak.upgrade() else { return };
                        if n > 0 {
                            app.close_check.set(CloseCheck::Idle);
                            app.ask_background(n);
                        } else {
                            app.close_check.set(CloseCheck::Closing);
                            app.window.close();
                        }
                    });
                    return glib::Propagation::Stop;
                }
                CloseCheck::Idle | CloseCheck::Closing => {}
            }
            app.player.shutdown();
            app.store.flush(crate::store::FLUSH_WAIT);
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
        self.show(&route);
        self.back_btn.set_sensitive(self.hist_pos.get() > 0);
        self.forward_btn
            .set_sensitive(self.hist_pos.get() + 1 < self.history.borrow().len());
        if let Some(s) = self.sidebar.borrow().as_ref() {
            s.set_active(&route);
        }
    }

    /// Shows `route`'s page: a kept one (brought up to date, or pointed at
    /// the new route), else a new one.
    fn show(&self, route: &Route) {
        if self.shown.borrow().as_ref() == Some(route) {
            return;
        }
        let app = self.rc();
        let found = {
            let slots = self.slots.borrow();
            slots.iter().position(|s| s.route == *route).or_else(|| {
                if route.kept() == Some(Kept::Search) {
                    slots
                        .iter()
                        .position(|s| matches!(s.route, Route::Search(_)))
                } else {
                    None
                }
            })
        };
        match found {
            Some(i) => {
                let (same, retarget) = {
                    let s = &self.slots.borrow()[i];
                    (s.route == *route, s.page.retarget.clone())
                };
                if !same {
                    if retarget.is_some_and(|r| r(&app, route)) {
                        self.slots.borrow_mut()[i].route = route.clone();
                    } else {
                        self.rebuild_slot(i, route);
                    }
                }
                self.catch_up(i);
            }
            None => {
                let page = ui::pages::build(&app, route);
                self.slots.borrow_mut().push(Slot {
                    route: route.clone(),
                    page,
                    pending: Vec::new(),
                });
            }
        }
        self.switch_to(route);
    }

    /// Builds the page of slot `i` again, for `route`.
    fn rebuild_slot(&self, i: usize, route: &Route) {
        let page = ui::pages::build(&self.rc(), route);
        let old = {
            let mut slots = self.slots.borrow_mut();
            let s = &mut slots[i];
            s.route = route.clone();
            s.pending.clear();
            std::mem::replace(&mut s.page, page)
        };
        self.retire(old);
    }

    /// Applies the store changes a hidden page missed.
    fn catch_up(&self, i: usize) {
        let (pending, update, route) = {
            let mut slots = self.slots.borrow_mut();
            let s = &mut slots[i];
            (
                std::mem::take(&mut s.pending),
                s.page.update.clone(),
                s.route.clone(),
            )
        };
        if pending.is_empty() {
            return;
        }
        let app = self.rc();
        let current = update.is_some_and(|u| pending.iter().all(|c| u(&app, *c)));
        if !current {
            self.rebuild_slot(i, &route);
        }
    }

    /// Makes `route`'s slot the page shown; the page shown before is hidden
    /// (kept) or retired.
    fn switch_to(&self, route: &Route) {
        // A page hidden or removed while the keyboard focus is inside it
        // keeps it (GTK 4.22 keeps its scrolled window and the focus chain
        // below it alive). Take the focus out first and give it to the new
        // page.
        let focus = gtk::prelude::GtkWindowExt::focus(&self.window)
            .is_some_and(|f| f.is_ancestor(&self.content));
        if focus {
            gtk::prelude::GtkWindowExt::set_focus(&self.window, None::<&gtk::Widget>);
        }
        self.shown.replace(Some(route.clone()));
        let kind = route.kept();
        let mut gone = Vec::new();
        let mut widget = None;
        {
            let mut slots = self.slots.borrow_mut();
            let mut i = 0;
            while i < slots.len() {
                let s = &slots[i];
                if s.route == *route {
                    widget = Some(s.page.widget.clone());
                    i += 1;
                    continue;
                }
                // One detail page and one search page stay; the other pages
                // left for good go.
                let k = s.route.kept();
                if k.is_none() || (k != Some(Kept::Top) && k == kind) {
                    gone.push(slots.remove(i).page);
                } else {
                    s.page.widget.set_visible(false);
                    i += 1;
                }
            }
        }
        if let Some(w) = &widget {
            w.set_vexpand(true);
            if w.parent().is_none() {
                self.content.append(w);
            }
            w.set_visible(true);
            if focus {
                self.focus_page(w);
            }
        }
        for page in gone {
            self.retire(page);
        }
        trim_heap_soon();
    }

    /// Moves the keyboard focus into `page`, or to the search box when the
    /// page has nothing to focus. The window must not be left without a
    /// focus widget: a dialog opened then (libadwaita 1.9) keeps its sheet
    /// and everything in it alive once closed.
    fn focus_page(&self, page: &gtk::Widget) {
        if !page.child_focus(gtk::DirectionType::TabForward) {
            self.search_entry.grab_focus();
        }
    }

    /// Hides `page` and removes it a chunk per frame, starting once the page
    /// shown instead has drawn its first frame: destroying a list of 200
    /// rows at once took 20-30 ms.
    fn retire(&self, page: Page) {
        page.widget.set_visible(false);
        self.retired.borrow_mut().push(page);
        if self.retiring.replace(true) {
            return;
        }
        let weak = self.me.clone();
        let first = Cell::new(true);
        self.window.add_tick_callback(move |_, _| {
            let Some(app) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if first.replace(false) || app.drain_retired() {
                glib::ControlFlow::Continue
            } else {
                app.retiring.set(false);
                glib::ControlFlow::Break
            }
        });
    }

    /// One step of taking retired pages away; whether any are left.
    fn drain_retired(&self) -> bool {
        let next = self
            .retired
            .borrow()
            .first()
            .map(|p| (p.drain.clone(), p.widget.clone()));
        let Some((drain, widget)) = next else {
            return false;
        };
        if drain.is_none_or(|d| d()) {
            self.retired.borrow_mut().remove(0);
            if widget.parent().as_ref() == Some(self.content.upcast_ref()) {
                self.content.remove(&widget);
            }
            trim_heap_soon();
        }
        !self.retired.borrow().is_empty()
    }

    /// Builds the page shown again, keeping its scroll position.
    pub fn refresh(&self) {
        let Some(route) = self.shown.borrow().clone() else {
            return;
        };
        let Some(i) = self.slots.borrow().iter().position(|s| s.route == route) else {
            return;
        };
        let value = self.slots.borrow()[i]
            .page
            .scroller
            .as_ref()
            .map(|s| s.vadjustment().value());
        let focus = gtk::prelude::GtkWindowExt::focus(&self.window)
            .is_some_and(|f| f.is_ancestor(&self.slots.borrow()[i].page.widget));
        if focus {
            gtk::prelude::GtkWindowExt::set_focus(&self.window, None::<&gtk::Widget>);
        }
        self.rebuild_slot(i, &route);
        let (widget, scroller) = {
            let slots = self.slots.borrow();
            (slots[i].page.widget.clone(), slots[i].page.scroller.clone())
        };
        widget.set_vexpand(true);
        self.content.append(&widget);
        if focus {
            self.focus_page(&widget);
        }
        if let (Some(v), Some(s)) = (value, scroller)
            && v > 0.0
        {
            // The new page has no size yet (a playlist's entries even arrive
            // from a thread): restore once it is tall enough.
            let handler: Rc<RefCell<Option<glib::SignalHandlerId>>> = Rc::default();
            let h = handler.clone();
            let id = s.vadjustment().connect_upper_notify(move |adj| {
                if adj.upper() - adj.page_size() >= v {
                    adj.set_value(v);
                    if let Some(id) = h.borrow_mut().take() {
                        adj.disconnect(id);
                    }
                }
            });
            *handler.borrow_mut() = Some(id);
        }
    }

    /// The pages that show what `change` changed: the one shown is updated
    /// in place (or built again), hidden ones when they are shown next.
    fn update_pages(&self, change: StoreChange) {
        let shown = self.shown.borrow().clone();
        let mut now = None;
        let mut gone = Vec::new();
        {
            let mut slots = self.slots.borrow_mut();
            let mut i = 0;
            while i < slots.len() {
                let s = &mut slots[i];
                if !s.route.shows(change) {
                    i += 1;
                } else if Some(&s.route) == shown.as_ref() {
                    now = Some(s.page.update.clone());
                    i += 1;
                } else if s.page.update.is_some() {
                    if !s.pending.contains(&change) {
                        s.pending.push(change);
                    }
                    i += 1;
                } else {
                    gone.push(slots.remove(i).page);
                }
            }
        }
        for page in gone {
            self.retire(page);
        }
        if let Some(update) = now
            && !update.is_some_and(|u| u(&self.rc(), change))
        {
            self.refresh();
        }
    }

    fn on_store_change(&self, change: StoreChange) {
        match change {
            StoreChange::Library => {
                self.covers.invalidate(&self.store.library());
                if let Some(s) = self.sidebar.borrow().as_ref() {
                    s.rebuild();
                }
                if let Some(q) = self.queue_panel.borrow().as_ref() {
                    q.rebuild();
                }
            }
            StoreChange::Playlists => {
                if let Some(s) = self.sidebar.borrow().as_ref() {
                    s.rebuild();
                }
            }
            StoreChange::Liked => {
                self.refresh_track_rows();
                if let Some(s) = self.sidebar.borrow().as_ref() {
                    s.rebuild();
                }
            }
            StoreChange::Downloads => ui::downloads::refresh_if_shown(),
            StoreChange::Scanning => {}
        }
        self.update_pages(change);
    }

    // ---- shared helpers ------------------------------------------------

    /// Calls `f` with the player's changes once a frame (see
    /// `connect_signals`).
    pub fn connect_player(&self, f: impl Fn(Changes) + 'static) {
        self.player_views.borrow_mut().push(Rc::new(f));
    }

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

    /// Marks the track playing in every track list.
    fn mark_current_rows(&self) {
        let current = self.player.current_path();
        let lists: Vec<_> = self
            .track_lists
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .collect();
        for l in lists {
            l.mark_current(current.as_deref());
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

    /// The callback for a background store edit: shows `context` and the
    /// error if it failed.
    pub fn on_error(&self, context: &'static str) -> impl FnOnce(std::io::Result<()>) + 'static {
        let me = self.me.clone();
        move |result| {
            if let (Err(e), Some(app)) = (result, me.upgrade()) {
                app.error(context, e);
            }
        }
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

    /// Saves playback state and queued playlist edits, stops the downloader
    /// and quits.
    pub fn quit(&self) {
        self.leave_background();
        self.player.shutdown();
        self.store.flush(crate::store::FLUSH_WAIT);
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

    /// The window is being closed while this device still has `n` downloads
    /// to finish: offer to keep downloading in the background.
    fn ask_background(&self, n: usize) {
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
                "background" => app.enter_background(n),
                "quit" => app.quit(),
                _ => {}
            }
        });
        dialog.present(Some(&self.window));
    }

    fn enter_background(&self, n: usize) {
        if self.background.borrow().is_some() {
            return;
        }
        self.player.pause();
        self.player.save();
        *self.background.borrow_mut() = Some(self.gtk_app.hold());
        self.window.set_visible(false);
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
            // The count runs on a thread; one at a time.
            if !app.background_check.replace(true) {
                let weak = app.me.clone();
                app.downloader.pending_work(move |n| {
                    let Some(app) = weak.upgrade() else { return };
                    app.background_check.set(false);
                    if n == 0 && app.background.borrow().is_some() {
                        app.quit();
                    }
                });
            }
            glib::ControlFlow::Continue
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

/// What the GUI tests look at.
#[cfg(test)]
impl App {
    /// The root widget of the page shown.
    pub fn page_widget(&self) -> Option<gtk::Widget> {
        let shown = self.shown.borrow();
        self.slots
            .borrow()
            .iter()
            .find(|s| Some(&s.route) == shown.as_ref())
            .map(|s| s.page.widget.clone())
    }

    /// Lets go of the pages kept for going back (they are freed once
    /// emptied, a frame or more later), and says how many there were.
    pub fn forget_kept_pages(&self) -> usize {
        let shown = self.shown.borrow().clone();
        let gone: Vec<Page> = {
            let mut slots = self.slots.borrow_mut();
            let (keep, gone): (Vec<Slot>, Vec<Slot>) = slots
                .drain(..)
                .partition(|s| Some(&s.route) == shown.as_ref());
            *slots = keep;
            gone.into_iter().map(|s| s.page).collect()
        };
        let n = gone.len();
        for page in gone {
            self.retire(page);
        }
        n
    }

    /// The queue panel's widget.
    pub fn queue_widget(&self) -> gtk::Widget {
        let q = self.queue_panel.borrow();
        q.as_ref().expect("queue panel").widget.clone().upcast()
    }
}
