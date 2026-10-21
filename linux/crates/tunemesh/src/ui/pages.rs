//! Page builders: home, search, library grids and detail pages.

use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::{gio, glib};
use tunemesh_core::playlist::Summary;
use tunemesh_core::search::{Query, fold};
use tunemesh_core::{LIKED_SONGS, Library, Playlist};

use crate::app::{App, Page, Route};
use crate::covers::Source;
use crate::ui::menu::{self, MenuCtx};
use crate::ui::tracklist::{self, Entries, Spec};
use crate::ui::widgets::{self, Tile, TileArt, fmt_total, label, plural, section_title, tile_flow};
use crate::ui::{downloads, settings};

pub fn build(app: &Rc<App>, route: &Route) -> Page {
    match route {
        Route::Home => home(app),
        Route::Search(q) => search(app, q),
        Route::Albums => albums(app),
        Route::Artists => artists(app),
        Route::Playlists => playlists(app),
        Route::Album(key) => album(app, key),
        Route::Artist(name) => artist(app, name),
        Route::Playlist(name) => playlist(app, name),
        Route::Downloads => downloads::page(app),
        Route::Settings => settings::page(app),
    }
}

/// The tracks a route plays, with its context name.
pub fn route_tracks(app: &App, route: &Route) -> (String, Vec<String>) {
    let lib = app.store.library();
    match route {
        Route::Album(key) => match lib.album_index(key) {
            Some(i) => (lib.albums[i].name.clone(), album_paths(&lib, i)),
            None => (String::new(), Vec::new()),
        },
        Route::Artist(name) => match lib.artist_index(name) {
            Some(i) => (
                name.clone(),
                lib.artists[i]
                    .tracks
                    .iter()
                    .map(|&t| lib.tracks[t].path.clone())
                    .collect(),
            ),
            None => (String::new(), Vec::new()),
        },
        Route::Playlist(name) => match app.store.playlist(name) {
            Some(p) => (name.clone(), available_paths(&lib, &p)),
            None => (String::new(), Vec::new()),
        },
        Route::Search(q) => {
            let r = lib.search(&Query::new(q));
            (
                format!("Search “{q}”"),
                r.tracks
                    .iter()
                    .map(|&t| lib.tracks[t].path.clone())
                    .collect(),
            )
        }
        _ => (
            "Library".into(),
            lib.tracks.iter().map(|t| t.path.clone()).collect(),
        ),
    }
}

fn album_paths(lib: &Library, album: usize) -> Vec<String> {
    lib.albums[album]
        .tracks
        .iter()
        .map(|&t| lib.tracks[t].path.clone())
        .collect()
}

fn available_paths(lib: &Library, p: &Playlist) -> Vec<String> {
    p.entries
        .iter()
        .filter_map(|e| e.track().map(str::to_owned))
        .filter(|t| lib.track_index(t).is_some())
        .collect()
}

fn scrolled(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(child)
        .build()
}

fn page_box() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_start(24)
        .margin_end(24)
        .margin_top(20)
        .margin_bottom(24)
        .build()
}

fn simple(child: &impl IsA<gtk::Widget>) -> Page {
    let s = scrolled(child);
    Page {
        widget: s.clone().upcast(),
        scroller: Some(s),
    }
}

/// A playlist over the contract's size limits: it is not opened, played or
/// changed on any device.
fn too_large(name: &str) -> Page {
    let b = page_box();
    b.append(&label(name, &["tm-page-title"]));
    b.append(&widgets::empty(&format!(
        "Too large to open. Playlists can hold at most {} songs and {} MiB, \
         so TuneMesh leaves this one unchanged on every device.",
        fmt_count(tunemesh_core::limits::MAX_PLAYLIST_ENTRIES),
        tunemesh_core::limits::MAX_PLAYLIST_BYTES / (1024 * 1024),
    )));
    simple(&b)
}

/// `250000` as `250,000`.
fn fmt_count(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn not_found() -> Page {
    let b = page_box();
    b.append(&widgets::empty("This item is no longer in your library."));
    simple(&b)
}

// ---- tiles --------------------------------------------------------------

fn album_tile(app: &Rc<App>, lib: &Arc<Library>, i: usize, tile: &Tile, subtitle_artist: bool) {
    let a = &lib.albums[i];
    let subtitle = if subtitle_artist {
        a.artist.clone()
    } else {
        a.year
            .map_or("Album".to_owned(), |y| format!("{y} • Album"))
    };
    let key = a.key();
    let (app1, app2, name, paths) = (
        app.clone(),
        app.clone(),
        a.name.clone(),
        album_paths(lib, i),
    );
    tile.set(
        app,
        TileArt::Cover(Source::for_album(lib, i)),
        &a.name,
        &subtitle,
        move || app1.navigate(Route::Album(key.clone())),
        move || {
            app2.player
                .play_tracks(&name, paths.clone(), 0, Some(false))
        },
    );
}

fn artist_tile(app: &Rc<App>, lib: &Arc<Library>, i: usize, tile: &Tile) {
    let ar = &lib.artists[i];
    let source = ar
        .albums
        .first()
        .map(|&a| Source::for_album(lib, a))
        .unwrap_or(Source::Placeholder(ar.name.clone()));
    let (app1, app2, name, name2) = (app.clone(), app.clone(), ar.name.clone(), ar.name.clone());
    let paths: Vec<String> = ar
        .tracks
        .iter()
        .map(|&t| lib.tracks[t].path.clone())
        .collect();
    tile.set(
        app,
        TileArt::Cover(source),
        &ar.name,
        "Artist",
        move || app1.navigate(Route::Artist(name.clone())),
        move || {
            app2.player
                .play_tracks(&name2, paths.clone(), 0, Some(false))
        },
    );
}

fn playlist_tile(app: &Rc<App>, lib: &Arc<Library>, p: &Summary) -> gtk::Widget {
    let tile = Tile::new(false);
    bind_playlist_tile(app, lib, p, &tile);
    tile.widget.upcast()
}

fn bind_playlist_tile(app: &Rc<App>, lib: &Arc<Library>, p: &Summary, tile: &Tile) {
    let liked = p.name == LIKED_SONGS;
    let art = if liked {
        TileArt::Liked
    } else {
        TileArt::Cover(Source::for_paths(
            lib,
            p.first_tracks.iter().map(String::as_str),
            &p.name,
        ))
    };
    let (app1, app2, name, name2) = (app.clone(), app.clone(), p.name.clone(), p.name.clone());
    tile.set(
        app,
        art,
        &p.name,
        &if app.store.is_too_large(&p.name) {
            "Too large to open".to_owned()
        } else {
            format!("Playlist • {}", plural(p.count, "song", "songs"))
        },
        move || app1.navigate(Route::Playlist(name.clone())),
        move || {
            // The entries are read when the user asks to play them.
            let Some(p) = app2.store.playlist(&name2) else {
                return;
            };
            let paths = available_paths(&app2.store.library(), &p);
            app2.player.play_tracks(&name2, paths, 0, Some(false))
        },
    );
}

fn flow_section(
    title: &str,
    tiles: Vec<gtk::Widget>,
    see_all: Option<(Rc<App>, Route)>,
) -> Option<gtk::Box> {
    if tiles.is_empty() {
        return None;
    }
    let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let t = section_title(title);
    t.set_hexpand(true);
    head.append(&t);
    if let Some((app, route)) = see_all {
        let btn = gtk::Button::builder()
            .label("Show all")
            .css_classes(["flat", "tm-link"])
            .valign(gtk::Align::Center)
            .build();
        btn.add_css_class("dim");
        btn.connect_clicked(move |_| app.navigate(route.clone()));
        head.append(&btn);
    }
    b.append(&head);
    let flow = tile_flow();
    flow.set_max_children_per_line(8);
    for t in tiles {
        flow.append(&t);
    }
    b.append(&flow);
    Some(b)
}

// ---- home ---------------------------------------------------------------

fn home(app: &Rc<App>) -> Page {
    let lib = app.store.library();
    let b = page_box();
    let hour = glib::DateTime::now_local().map(|d| d.hour()).unwrap_or(12);
    let greeting = match hour {
        5..=11 => "Good morning",
        12..=17 => "Good afternoon",
        _ => "Good evening",
    };
    b.append(&label(greeting, &["tm-page-title"]));

    if lib.tracks.is_empty() && app.store.playlists().is_empty() {
        let root = app.store.root();
        let msg = if app.store.is_scanning() {
            "Scanning your library…".to_owned()
        } else if !root.is_dir() {
            format!(
                "The library folder {} does not exist yet. Choose another one in Settings.",
                root.display()
            )
        } else {
            format!(
                "Your library is empty. Download songs from the Downloads page, or add audio files to {}.",
                root.display()
            )
        };
        b.append(&widgets::empty(&msg));
        return simple(&b);
    }

    let recent: Vec<gtk::Widget> = lib
        .recently_added_albums()
        .into_iter()
        .take(12)
        .map(|i| {
            let t = Tile::new(false);
            album_tile(app, &lib, i, &t, true);
            t.widget.upcast()
        })
        .collect();
    if let Some(s) = flow_section("Recently added", recent, Some((app.clone(), Route::Albums))) {
        b.append(&s);
    }

    let mut lists = vec![playlist_tile(app, &lib, &app.store.liked_summary())];
    lists.extend(
        app.store
            .playlists()
            .iter()
            .take(11)
            .map(|p| playlist_tile(app, &lib, p)),
    );
    if let Some(s) = flow_section(
        "Your playlists",
        lists,
        Some((app.clone(), Route::Playlists)),
    ) {
        b.append(&s);
    }

    let mut seen = Vec::new();
    for ai in lib.recently_added_albums() {
        for &ti in &lib.albums[ai].tracks {
            if let Some(i) = lib.artist_index(lib.tracks[ti].listed_artist())
                && !seen.contains(&i)
            {
                seen.push(i);
            }
        }
        if seen.len() >= 12 {
            break;
        }
    }
    let artists: Vec<gtk::Widget> = seen
        .into_iter()
        .take(12)
        .map(|i| {
            let t = Tile::new(true);
            artist_tile(app, &lib, i, &t);
            t.widget.upcast()
        })
        .collect();
    if let Some(s) = flow_section("Artists", artists, Some((app.clone(), Route::Artists))) {
        b.append(&s);
    }
    simple(&b)
}

// ---- search -------------------------------------------------------------

fn search(app: &Rc<App>, text: &str) -> Page {
    let lib = app.store.library();
    let b = page_box();
    let q = Query::new(text);
    if q.is_empty() {
        b.append(&label("Search", &["tm-page-title"]));
        b.append(&widgets::empty(
            "Find songs, artists, albums and playlists in your library.",
        ));
        return simple(&b);
    }
    let r = lib.search(&q);
    let lists: Vec<Summary> = std::iter::once(app.store.liked_summary())
        .chain(app.store.playlists())
        .filter(|p| q.matches(&fold(&p.name)))
        .collect();
    if r.tracks.is_empty() && r.albums.is_empty() && r.artists.is_empty() && lists.is_empty() {
        b.append(&widgets::empty(&format!("No results found for “{text}”")));
        return simple(&b);
    }
    if !r.tracks.is_empty() {
        b.append(&section_title("Songs"));
        let entries = Entries::Tracks(
            r.tracks
                .iter()
                .take(50)
                .map(|&i| lib.tracks[i].path.clone())
                .collect(),
        );
        let list = tracklist::build(
            app,
            Spec {
                context: format!("Search “{text}”"),
                entries,
                show_album: true,
                show_cover: true,
                playlist: None,
                header: None,
            },
        );
        list.add_css_class("tm-flush");
        b.append(&list);
    }
    let artists: Vec<gtk::Widget> = r
        .artists
        .iter()
        .take(12)
        .map(|&i| {
            let t = Tile::new(true);
            artist_tile(app, &lib, i, &t);
            t.widget.upcast()
        })
        .collect();
    if let Some(s) = flow_section("Artists", artists, None) {
        b.append(&s);
    }
    let albums: Vec<gtk::Widget> = r
        .albums
        .iter()
        .take(12)
        .map(|&i| {
            let t = Tile::new(false);
            album_tile(app, &lib, i, &t, true);
            t.widget.upcast()
        })
        .collect();
    if let Some(s) = flow_section("Albums", albums, None) {
        b.append(&s);
    }
    let lists: Vec<gtk::Widget> = lists.iter().map(|p| playlist_tile(app, &lib, p)).collect();
    if let Some(s) = flow_section("Playlists", lists, None) {
        b.append(&s);
    }
    simple(&b)
}

// ---- library grids --------------------------------------------------------

fn grid_page(
    app: &Rc<App>,
    title: &str,
    action: Option<gtk::Widget>,
    count: usize,
    round: bool,
    bind: impl Fn(&Rc<App>, &Arc<Library>, usize, &Tile) + 'static,
) -> Page {
    let lib = app.store.library();
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let head = gtk::Box::builder()
        .spacing(12)
        .margin_start(24)
        .margin_end(24)
        .margin_top(20)
        .margin_bottom(8)
        .build();
    let title = label(title, &["tm-page-title"]);
    title.set_hexpand(true);
    head.append(&title);
    if let Some(w) = action {
        head.append(&w);
    }
    if count == 0 {
        outer.append(&head);
        outer.append(&widgets::empty("Nothing here yet."));
        return Page {
            widget: outer.upcast(),
            scroller: None,
        };
    }
    outer.append(&head);
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let items: Vec<glib::BoxedAnyObject> = (0..count).map(glib::BoxedAnyObject::new).collect();
    store.extend_from_slice(&items);
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, obj| {
        let li = obj.downcast_ref::<gtk::ListItem>().expect("list item");
        let tile = Tile::new(round);
        li.set_child(Some(&tile.widget));
        // SAFETY: only read back as `Tile`.
        unsafe { tile.widget.set_data("tm-tile", tile.clone()) };
    });
    let app2 = app.clone();
    factory.connect_bind(move |_, obj| {
        let li = obj.downcast_ref::<gtk::ListItem>().expect("list item");
        let widget = li.child().expect("tile");
        // SAFETY: set in setup with this type.
        let tile = unsafe {
            widget
                .data::<Tile>("tm-tile")
                .expect("tile")
                .as_ref()
                .clone()
        };
        let i = *li
            .item()
            .and_downcast::<glib::BoxedAnyObject>()
            .expect("item")
            .borrow::<usize>();
        bind(&app2, &lib, i, &tile);
    });
    let grid = gtk::GridView::builder()
        .model(&gtk::NoSelection::new(Some(store)))
        .factory(&factory)
        .min_columns(2)
        .max_columns(12)
        .css_classes(["tm-grid"])
        .margin_start(12)
        .margin_end(12)
        .build();
    let s = scrolled(&grid);
    outer.append(&s);
    Page {
        widget: outer.upcast(),
        scroller: Some(s),
    }
}

fn albums(app: &Rc<App>) -> Page {
    let lib = app.store.library();
    let order = Rc::new(lib.albums_by_name());
    let n = order.len();
    grid_page(app, "Albums", None, n, false, move |app, lib, i, tile| {
        album_tile(app, lib, order[i], tile, true)
    })
}

fn artists(app: &Rc<App>) -> Page {
    let n = app.store.library().artists.len();
    grid_page(app, "Artists", None, n, true, artist_tile)
}

fn playlists(app: &Rc<App>) -> Page {
    let create = gtk::Button::builder()
        .label("Create playlist")
        .css_classes(["tm-pill-outline"])
        .valign(gtk::Align::Center)
        .build();
    let a = app.clone();
    create.connect_clicked(move |_| create_playlist_dialog(&a));
    // Liked Songs first; a grid of recycled tiles, as there may be
    // a thousand playlists.
    let lists: Rc<Vec<Summary>> = Rc::new(
        std::iter::once(app.store.liked_summary())
            .chain(app.store.playlists())
            .collect(),
    );
    grid_page(
        app,
        "Playlists",
        Some(create.upcast()),
        lists.len(),
        false,
        move |app, lib, i, tile| bind_playlist_tile(app, lib, &lists[i], tile),
    )
}

pub fn create_playlist_dialog(app: &Rc<App>) {
    let a = app.clone();
    menu::ask_text(app, "New playlist", "", "Create", move |name| {
        match a.store.create_playlist(&name) {
            Ok(name) => a.navigate(Route::Playlist(name)),
            Err(e) => a.error("Could not create playlist", e),
        }
    });
}

// ---- detail pages -------------------------------------------------------

/// Opens a page's "more options" menu from its button.
type MoreMenu = Rc<dyn Fn(&gtk::Button)>;

enum HeroArt {
    Cover(Source),
    Round(Source),
    Liked,
}

fn hero(app: &App, art: HeroArt, kind: &str, title: &str, meta: &gtk::Widget) -> gtk::Box {
    let b = gtk::Box::builder()
        .spacing(24)
        .css_classes(["tm-hero"])
        .build();
    let art_widget: gtk::Widget = match art {
        HeroArt::Cover(s) => widgets::cover(app, s, 232, false).upcast(),
        HeroArt::Round(s) => widgets::cover(app, s, 232, true).upcast(),
        HeroArt::Liked => {
            b.add_css_class("liked");
            widgets::liked_cover(232)
        }
    };
    art_widget.add_css_class("tm-cover-shadow");
    b.append(&art_widget);
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .valign(gtk::Align::End)
        .hexpand(true)
        .build();
    text.append(&label(kind, &["tm-hero-kind"]));
    let t = gtk::Label::builder()
        .label(tunemesh_core::limits::clip_line(title))
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .lines(2)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["tm-hero-title"])
        .build();
    if title.chars().count() > 24 {
        t.add_css_class("long");
    }
    text.append(&t);
    text.append(meta);
    b.append(&text);
    b
}

fn meta_line(parts: Vec<gtk::Widget>) -> gtk::Widget {
    let b = gtk::Box::builder()
        .spacing(6)
        .css_classes(["tm-hero-meta"])
        .build();
    for (i, p) in parts.into_iter().enumerate() {
        if i > 0 {
            b.append(&gtk::Label::new(Some("•")));
        }
        b.append(&p);
    }
    b.upcast()
}

fn artist_link(app: &Rc<App>, name: &str) -> gtk::Widget {
    let btn = gtk::Button::builder()
        .css_classes(["tm-link", "flat"])
        .build();
    btn.set_child(Some(&label(name, &["strong"])));
    let (a, n) = (app.clone(), name.to_owned());
    btn.connect_clicked(move |_| a.navigate(Route::Artist(n.clone())));
    btn.upcast()
}

fn actions(app: &Rc<App>, name: &str, paths: Vec<String>, more: Option<MoreMenu>) -> gtk::Box {
    let b = gtk::Box::builder()
        .spacing(20)
        .css_classes(["tm-actions"])
        .build();
    let play = widgets::play_button(true);
    let shuffle = widgets::icon_button("media-playlist-shuffle-symbolic", "Shuffle play");
    shuffle.add_css_class("big");
    let empty = paths.is_empty();
    play.set_sensitive(!empty);
    shuffle.set_sensitive(!empty);
    let paths = Rc::new(paths);
    let (a, n, p) = (app.clone(), name.to_owned(), paths.clone());
    play.connect_clicked(move |_| a.player.play_tracks(&n, (*p).clone(), 0, Some(false)));
    let (a, n, p) = (app.clone(), name.to_owned(), paths.clone());
    shuffle.connect_clicked(move |_| a.player.play_tracks(&n, (*p).clone(), 0, Some(true)));
    b.append(&play);
    b.append(&shuffle);
    let more_btn = widgets::icon_button("view-more-symbolic", "More options");
    more_btn.add_css_class("big");
    match more {
        Some(f) => {
            more_btn.connect_clicked(move |btn| f(btn));
        }
        None => {
            let (a, p) = (app.clone(), paths.clone());
            more_btn.connect_clicked(move |btn| {
                let ctx = MenuCtx {
                    paths: (*p).clone(),
                    playlist: None,
                    queue_index: None,
                };
                menu::popup(
                    &a,
                    btn.upcast_ref(),
                    btn.width() as f64 / 2.0,
                    btn.height() as f64,
                    ctx,
                );
            });
            more_btn.set_sensitive(!empty);
        }
    }
    b.append(&more_btn);
    b
}

fn detail_page(
    app: &Rc<App>,
    header: gtk::Box,
    spec_entries: Entries,
    context: &str,
    show_album: bool,
    show_cover: bool,
    playlist: Option<String>,
) -> Page {
    if !spec_entries.is_empty() {
        header.append(&tracklist::columns_header(show_album, show_cover));
    }
    let list = tracklist::build(
        app,
        Spec {
            context: context.to_owned(),
            entries: spec_entries,
            show_album,
            show_cover,
            playlist,
            header: Some(header.upcast()),
        },
    );
    list.set_margin_bottom(24);
    let s = scrolled(&list);
    Page {
        widget: s.clone().upcast(),
        scroller: Some(s),
    }
}

fn album(app: &Rc<App>, key: &str) -> Page {
    let lib = app.store.library();
    let Some(ai) = lib.album_index(key) else {
        return not_found();
    };
    let a = &lib.albums[ai];
    let paths = album_paths(&lib, ai);
    let mut parts = vec![artist_link(app, &a.artist)];
    if let Some(y) = a.year {
        parts.push(gtk::Label::new(Some(&y.to_string())).upcast());
    }
    parts.push(
        gtk::Label::new(Some(&format!(
            "{}, {}",
            plural(a.tracks.len(), "song", "songs"),
            fmt_total(a.duration_ms)
        )))
        .upcast(),
    );
    let header = gtk::Box::new(gtk::Orientation::Vertical, 0);
    header.append(&hero(
        app,
        HeroArt::Cover(Source::for_album(&lib, ai)),
        "Album",
        &a.name,
        &meta_line(parts),
    ));
    header.append(&actions(app, &a.name, paths.clone(), None));
    let entries = Entries::Tracks(paths);
    detail_page(app, header, entries, &a.name, false, false, None)
}

fn artist(app: &Rc<App>, name: &str) -> Page {
    let lib = app.store.library();
    let Some(ari) = lib.artist_index(name) else {
        return not_found();
    };
    let ar = &lib.artists[ari];
    let paths: Vec<String> = ar
        .tracks
        .iter()
        .map(|&t| lib.tracks[t].path.clone())
        .collect();
    let source = ar
        .albums
        .first()
        .map(|&a| Source::for_album(&lib, a))
        .unwrap_or(Source::Placeholder(ar.name.clone()));
    let meta = meta_line(vec![
        gtk::Label::new(Some(&plural(ar.tracks.len(), "song", "songs"))).upcast(),
        gtk::Label::new(Some(&plural(ar.albums.len(), "album", "albums"))).upcast(),
    ]);
    let header = gtk::Box::new(gtk::Orientation::Vertical, 0);
    header.append(&hero(
        app,
        HeroArt::Round(source),
        "Artist",
        &ar.name,
        &meta,
    ));
    header.append(&actions(app, &ar.name, paths.clone(), None));
    let tiles: Vec<gtk::Widget> = ar
        .albums
        .iter()
        .map(|&i| {
            let t = Tile::new(false);
            album_tile(app, &lib, i, &t, false);
            t.widget.upcast()
        })
        .collect();
    if let Some(s) = flow_section("Albums", tiles, None) {
        s.set_margin_start(24);
        s.set_margin_end(24);
        header.append(&s);
    }
    let songs = section_title("Songs");
    songs.set_margin_start(24);
    songs.set_margin_top(12);
    songs.set_margin_bottom(8);
    header.append(&songs);
    let entries = Entries::Tracks(paths);
    detail_page(app, header, entries, &ar.name, true, true, None)
}

fn playlist(app: &Rc<App>, name: &str) -> Page {
    if app.store.is_too_large(name) {
        return too_large(name);
    }
    let lib = app.store.library();
    let Some(p) = app.store.playlist(name) else {
        return not_found();
    };
    let liked = name == LIKED_SONGS;
    let paths = available_paths(&lib, &p);
    let total: u64 = paths
        .iter()
        .filter_map(|t| lib.track(t))
        .map(|t| t.duration_ms)
        .sum();
    let mut parts: Vec<gtk::Widget> =
        vec![gtk::Label::new(Some(&plural(p.entries.len(), "song", "songs"))).upcast()];
    if total > 0 {
        parts.push(gtk::Label::new(Some(&fmt_total(total))).upcast());
    }
    let art = if liked {
        HeroArt::Liked
    } else {
        HeroArt::Cover(Source::for_paths(
            &lib,
            p.entries.iter().filter_map(|e| e.track()),
            name,
        ))
    };
    let header = gtk::Box::new(gtk::Orientation::Vertical, 0);
    header.append(&hero(app, art, "Playlist", name, &meta_line(parts)));
    let more: MoreMenu = {
        let (a, n, all) = (app.clone(), name.to_owned(), paths.clone());
        Rc::new(move |btn: &gtk::Button| playlist_menu(&a, btn, &n, all.clone()))
    };
    header.append(&actions(app, name, paths, Some(more)));
    if p.entries.is_empty() {
        let msg = if liked {
            "Songs you like appear here. Save songs with the heart icon."
        } else {
            "This playlist is empty. Add songs from their right-click menu."
        };
        header.append(&widgets::empty(msg));
    }
    let entries = Entries::Playlist(p);
    detail_page(
        app,
        header,
        entries,
        name,
        true,
        true,
        Some(name.to_owned()),
    )
}

fn playlist_menu(app: &Rc<App>, btn: &gtk::Button, name: &str, paths: Vec<String>) {
    let menu = gio::Menu::new();
    let group = gio::SimpleActionGroup::new();
    let add = |id: &str, f: Box<dyn Fn()>| {
        let a = gio::SimpleAction::new(id, None);
        a.connect_activate(move |_, _| f());
        group.add_action(&a);
    };
    menu.append(Some("Add to queue"), Some("pl.queue"));
    let (a, p) = (app.clone(), paths.clone());
    add(
        "queue",
        Box::new(move || {
            a.player.add_to_queue(&p);
            a.toast(&format!(
                "Added {} to queue",
                plural(p.len(), "song", "songs")
            ));
        }),
    );
    if name != LIKED_SONGS {
        let section = gio::Menu::new();
        section.append(Some("Rename…"), Some("pl.rename"));
        section.append(Some("Delete…"), Some("pl.delete"));
        menu.append_section(None, &section);
        let (a, n) = (app.clone(), name.to_owned());
        add(
            "rename",
            Box::new(move || {
                let (a2, old) = (a.clone(), n.clone());
                menu::ask_text(&a, "Rename playlist", &n, "Rename", move |new| {
                    match a2.store.rename_playlist(&old, &new) {
                        Ok(new) => a2.playlist_renamed(&old, &new),
                        Err(e) => a2.error("Could not rename playlist", e),
                    }
                });
            }),
        );
        let (a, n) = (app.clone(), name.to_owned());
        add(
            "delete",
            Box::new(move || {
                let (a2, n2) = (a.clone(), n.clone());
                menu::confirm(
                    &a,
                    &format!("Delete “{n}”?"),
                    "The playlist is removed from every device. The songs stay in your library.",
                    "Delete",
                    move || match a2.store.delete_playlist(&n2) {
                        Ok(()) => {
                            a2.toast(&format!("Deleted {n2}"));
                            a2.navigate(Route::Playlists);
                        }
                        Err(e) => a2.error("Could not delete playlist", e),
                    },
                );
            }),
        );
    }
    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.insert_action_group("pl", Some(&group));
    popover.set_parent(btn);
    popover.connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
    popover.popup();
}
