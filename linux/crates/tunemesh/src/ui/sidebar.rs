//! Left navigation: main sections plus "Your Library".

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use tunemesh_core::LIKED_SONGS;

use crate::app::{App, Route};
use crate::covers::Source;
use crate::ui::pages::create_playlist_dialog;
use crate::ui::widgets::{self, label, plural};

pub struct Sidebar {
    pub widget: gtk::Box,
    app: Weak<App>,
    nav: Vec<(Route, gtk::Button)>,
    list: gtk::Box,
    rows: RefCell<Vec<(Route, gtk::Button)>>,
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
        let list = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
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
            list,
            rows: RefCell::new(Vec::new()),
        });
        s.rebuild();
        s
    }

    fn row(
        &self,
        app: &Rc<App>,
        art: gtk::Widget,
        title: &str,
        subtitle: &str,
        route: Route,
    ) -> gtk::Button {
        let b = gtk::Box::builder().spacing(12).build();
        b.append(&art);
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .valign(gtk::Align::Center)
            .build();
        let t = label(title, &["title"]);
        t.set_max_width_chars(1);
        t.set_hexpand(true);
        text.append(&t);
        text.append(&label(subtitle, &["subtitle"]));
        b.append(&text);
        let btn = gtk::Button::builder()
            .child(&b)
            .css_classes(["tm-sidebar-playlist"])
            .build();
        let (a, r) = (app.clone(), route.clone());
        btn.connect_clicked(move |_| a.navigate(r.clone()));
        self.list.append(&btn);
        self.rows.borrow_mut().push((route, btn.clone()));
        btn
    }

    pub fn rebuild(&self) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        while let Some(c) = self.list.first_child() {
            self.list.remove(&c);
        }
        self.rows.borrow_mut().clear();
        let lib = app.store.library();
        self.row(
            &app,
            widgets::liked_cover(44),
            LIKED_SONGS,
            &format!(
                "Playlist • {}",
                plural(app.store.liked_count(), "song", "songs")
            ),
            Route::Playlist(LIKED_SONGS.into()),
        );
        self.row(
            &app,
            icon_tile("folder-music-symbolic"),
            "Albums",
            &plural(lib.albums.len(), "album", "albums"),
            Route::Albums,
        );
        self.row(
            &app,
            icon_tile("avatar-default-symbolic"),
            "Artists",
            &plural(lib.artists.len(), "artist", "artists"),
            Route::Artists,
        );
        for p in app.store.playlists() {
            let source =
                Source::for_paths(&lib, p.first_tracks.iter().map(String::as_str), &p.name);
            let art = widgets::cover(&app, source, 44, false);
            self.row(
                &app,
                art.upcast(),
                &p.name,
                &if app.store.is_too_large(&p.name) {
                    "Too large to open".to_owned()
                } else {
                    format!("Playlist • {}", plural(p.count, "song", "songs"))
                },
                Route::Playlist(p.name.clone()),
            );
        }
        self.set_active(&app.route());
        self.update_playing();
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
        for (r, b) in self.rows.borrow().iter() {
            if r == route {
                b.add_css_class("active")
            } else {
                b.remove_css_class("active")
            }
        }
    }

    /// Marks the playlist currently playing.
    pub fn update_playing(&self) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let context = app.player.context_name();
        for (r, b) in self.rows.borrow().iter() {
            let playing = matches!(r, Route::Playlist(n) if *n == context);
            if playing {
                b.add_css_class("playing")
            } else {
                b.remove_css_class("playing")
            }
        }
    }
}
