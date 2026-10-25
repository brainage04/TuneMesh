//! Left navigation: main sections plus "Your Library".

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gio, glib};
use tunemesh_core::{LIKED_SONGS, limits};

use crate::app::{App, Route};
use crate::covers::Source;
use crate::ui::pages::create_playlist_dialog;
use crate::ui::widgets::{self, label, plural};

pub struct Sidebar {
    pub widget: gtk::Box,
    app: Weak<App>,
    nav: Vec<(Route, gtk::Button)>,
    /// The library list's model and the entries it holds.
    store: gio::ListStore,
    entries: RefCell<Vec<Entry>>,
    /// Every row widget the list has made (bound or not).
    rows: RefCell<Vec<Row>>,
}

#[derive(Clone, PartialEq)]
enum Art {
    Liked,
    Icon(&'static str),
    Cover(Source),
}

/// One entry of "Your Library".
#[derive(Clone, PartialEq)]
struct Entry {
    art: Art,
    title: String,
    subtitle: String,
    route: Route,
}

/// A recycled row of the library list.
#[derive(Clone)]
struct Row {
    button: gtk::Button,
    art: gtk::Box,
    title: gtk::Label,
    subtitle: gtk::Label,
    /// The bound entry's page; empty while unbound.
    route: Rc<RefCell<Option<Route>>>,
}

fn nav_button(icon: &str, text: &str) -> gtk::Button {
    let b = gtk::Box::builder().spacing(16).build();
    b.append(&gtk::Image::builder().icon_name(icon).pixel_size(20).build());
    b.append(&gtk::Label::new(Some(text)));
    gtk::Button::builder()
        .child(&b)
        .css_classes(["flat"])
        .build()
}

fn icon_tile(icon: &str) -> gtk::Widget {
    let b = gtk::Box::builder()
        .width_request(44)
        .height_request(44)
        .hexpand(false)
        .vexpand(false)
        .valign(gtk::Align::Center)
        .css_classes(["tm-cover", "tm-icon-tile"])
        .build();
    let img = gtk::Image::builder()
        .icon_name(icon)
        .pixel_size(20)
        .hexpand(true)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    b.append(&img);
    b.upcast()
}

impl Sidebar {
    pub fn new(app: &Rc<App>) -> Rc<Self> {
        let widget = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .hexpand(false)
            .build();

        let top = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["tm-panel", "tm-sidebar", "tm-sidebar-nav"])
            .build();
        let nav = vec![
            (Route::Home, nav_button("go-home-symbolic", "Home")),
            (
                Route::Search(String::new()),
                nav_button("edit-find-symbolic", "Search"),
            ),
            (
                Route::Downloads,
                nav_button("folder-download-symbolic", "Downloads"),
            ),
            (
                Route::Settings,
                nav_button("emblem-system-symbolic", "Settings"),
            ),
        ];
        for (route, btn) in &nav {
            top.append(btn);
            let (a, r) = (app.clone(), route.clone());
            btn.connect_clicked(move |_| match &r {
                Route::Search(_) => a.gtk_app.activate_action("focus-search", None),
                r => a.navigate(r.clone()),
            });
        }
        widget.append(&top);

        let lib_panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["tm-panel", "tm-sidebar"])
            .vexpand(true)
            .build();
        let head = gtk::Box::builder()
            .spacing(12)
            .margin_start(10)
            .margin_end(4)
            .margin_top(4)
            .margin_bottom(8)
            .build();
        head.append(
            &gtk::Image::builder()
                .icon_name("tm-library-symbolic")
                .pixel_size(20)
                .css_classes(["dim"])
                .build(),
        );
        let title = label("Your Library", &["tm-sidebar-heading"]);
        title.set_margin_start(0);
        title.set_margin_top(0);
        title.set_margin_bottom(0);
        title.set_hexpand(true);
        head.append(&title);
        let add = widgets::icon_button("list-add-symbolic", "Create playlist");
        let a = app.clone();
        add.connect_clicked(move |_| create_playlist_dialog(&a));
        head.append(&add);
        lib_panel.append(&head);
        // A recycling list: a library may hold a thousand playlists, and a
        // box of a thousand rows took a quarter of a second to rebuild and
        // measure after every playlist, like or library change.
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let factory = gtk::SignalListItemFactory::new();
        let list = gtk::ListView::builder()
            .model(&gtk::NoSelection::new(Some(store.clone())))
            .factory(&factory)
            .css_classes(["tm-sidebar-list"])
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list)
            .build();
        lib_panel.append(&scroller);
        widget.append(&lib_panel);

        let s = Rc::new(Self {
            widget,
            app: Rc::downgrade(app),
            nav,
            store,
            entries: RefCell::new(Vec::new()),
            rows: RefCell::new(Vec::new()),
        });
        s.connect_factory(&factory);
        s.rebuild();
        s
    }

    fn connect_factory(self: &Rc<Self>, factory: &gtk::SignalListItemFactory) {
        let me = Rc::downgrade(self);
        factory.connect_setup(move |_, obj| {
            let Some(me) = me.upgrade() else { return };
            let li = obj.downcast_ref::<gtk::ListItem>().expect("list item");
            li.set_activatable(false);
            li.set_focusable(false);
            let art = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            let b = gtk::Box::builder().spacing(12).build();
            b.append(&art);
            let text = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .valign(gtk::Align::Center)
                .build();
            let title = label("", &["title"]);
            title.set_max_width_chars(1);
            title.set_hexpand(true);
            text.append(&title);
            let subtitle = label("", &["subtitle"]);
            text.append(&subtitle);
            b.append(&text);
            let button = gtk::Button::builder()
                .child(&b)
                .css_classes(["tm-sidebar-playlist"])
                .build();
            let route: Rc<RefCell<Option<Route>>> = Rc::default();
            let (a, r) = (me.app.clone(), route.clone());
            button.connect_clicked(move |_| {
                let route = r.borrow().clone();
                if let (Some(a), Some(route)) = (a.upgrade(), route) {
                    a.navigate(route);
                }
            });
            li.set_child(Some(&button));
            me.rows.borrow_mut().push(Row {
                button,
                art,
                title,
                subtitle,
                route,
            });
        });
        let me = Rc::downgrade(self);
        factory.connect_bind(move |_, obj| {
            let (Some(me), Some(app)) = (me.upgrade(), me.upgrade().and_then(|m| m.app.upgrade()))
            else {
                return;
            };
            let li = obj.downcast_ref::<gtk::ListItem>().expect("list item");
            let Some(row) = me.row_of(li) else { return };
            let item = li
                .item()
                .and_downcast::<glib::BoxedAnyObject>()
                .expect("item");
            let e = item.borrow::<Entry>();
            while let Some(c) = row.art.first_child() {
                row.art.remove(&c);
            }
            let art: gtk::Widget = match &e.art {
                Art::Liked => widgets::liked_cover(44),
                Art::Icon(icon) => icon_tile(icon),
                Art::Cover(source) => widgets::cover(&app, source.clone(), 44, false).upcast(),
            };
            row.art.append(&art);
            row.title.set_label(&limits::clip_line(&e.title));
            row.subtitle.set_label(&e.subtitle);
            *row.route.borrow_mut() = Some(e.route.clone());
            me.style(&row, &app.route(), &app.player.context_name());
        });
        let me = Rc::downgrade(self);
        factory.connect_unbind(move |_, obj| {
            let Some(me) = me.upgrade() else { return };
            let li = obj.downcast_ref::<gtk::ListItem>().expect("list item");
            if let Some(row) = me.row_of(li) {
                row.route.borrow_mut().take();
            }
        });
        let me = Rc::downgrade(self);
        factory.connect_teardown(move |_, obj| {
            let Some(me) = me.upgrade() else { return };
            let li = obj.downcast_ref::<gtk::ListItem>().expect("list item");
            if let Some(button) = li.child() {
                me.rows.borrow_mut().retain(|r| r.button != button);
            }
        });
    }

    fn row_of(&self, li: &gtk::ListItem) -> Option<Row> {
        let button = li.child()?;
        self.rows
            .borrow()
            .iter()
            .find(|r| r.button == button)
            .cloned()
    }

    /// Marks a row as the page shown and as the context playing.
    fn style(&self, row: &Row, shown: &Route, context: &str) {
        let route = row.route.borrow();
        let active = route.as_ref() == Some(shown);
        let playing = matches!(route.as_ref(), Some(Route::Playlist(n)) if n == context);
        for (class, on) in [("active", active), ("playing", playing)] {
            if on {
                row.button.add_css_class(class)
            } else {
                row.button.remove_css_class(class)
            }
        }
    }

    /// Updates the library list in place: only the entries that changed
    /// are replaced, so the list keeps its scroll position.
    pub fn rebuild(&self) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let lib = app.store.library();
        let mut new = vec![
            Entry {
                art: Art::Liked,
                title: LIKED_SONGS.to_owned(),
                subtitle: format!(
                    "Playlist • {}",
                    plural(app.store.liked_count(), "song", "songs")
                ),
                route: Route::Playlist(LIKED_SONGS.into()),
            },
            Entry {
                art: Art::Icon("folder-music-symbolic"),
                title: "Albums".to_owned(),
                subtitle: plural(lib.albums.len(), "album", "albums"),
                route: Route::Albums,
            },
            Entry {
                art: Art::Icon("avatar-default-symbolic"),
                title: "Artists".to_owned(),
                subtitle: plural(lib.artists.len(), "artist", "artists"),
                route: Route::Artists,
            },
        ];
        for p in app.store.playlists() {
            let source =
                Source::for_paths(&lib, p.first_tracks.iter().map(String::as_str), &p.name);
            new.push(Entry {
                art: Art::Cover(source),
                subtitle: if app.store.is_too_large(&p.name) {
                    "Too large to open".to_owned()
                } else {
                    format!("Playlist • {}", plural(p.count, "song", "songs"))
                },
                route: Route::Playlist(p.name.clone()),
                title: p.name,
            });
        }
        let old = self.entries.borrow();
        let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(new[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let removed = old.len() - prefix - suffix;
        let added: Vec<glib::BoxedAnyObject> = new[prefix..new.len() - suffix]
            .iter()
            .cloned()
            .map(glib::BoxedAnyObject::new)
            .collect();
        drop(old);
        if removed > 0 || !added.is_empty() {
            self.store.splice(prefix as u32, removed as u32, &added);
        }
        *self.entries.borrow_mut() = new;
        self.set_active(&app.route());
    }

    pub fn set_active(&self, route: &Route) {
        for (r, b) in &self.nav {
            let active =
                r == route || (matches!(r, Route::Search(_)) && matches!(route, Route::Search(_)));
            if active {
                b.add_css_class("active")
            } else {
                b.remove_css_class("active")
            }
        }
        let context = self
            .app
            .upgrade()
            .map(|a| a.player.context_name())
            .unwrap_or_default();
        for row in self.rows.borrow().iter() {
            self.style(row, route, &context);
        }
    }

    /// Marks the playlist currently playing.
    pub fn update_playing(&self) {
        if let Some(app) = self.app.upgrade() {
            self.set_active(&app.route());
        }
    }
}
