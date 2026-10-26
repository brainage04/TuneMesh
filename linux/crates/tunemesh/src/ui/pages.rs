//! Page builders: home, search, library grids and detail pages.
//!
//! The pages the window keeps (see `App::show`) update in place when the
//! library changes: grids and track lists replace only the items that
//! changed, so a synced file arriving does not build the page again.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use adw::prelude::*;
use gtk::{gio, glib};
use tunemesh_core::playlist::Summary;
use tunemesh_core::search::{Query, fold};
use tunemesh_core::{LIKED_SONGS, Library, Playlist};

use crate::app::{App, Page, Route};
use crate::covers::Source;
use crate::store::StoreChange;
use crate::ui::menu::{self, MenuCtx};
use crate::ui::rows::{self, LazyRows};
use crate::ui::tracklist::{self, Entries, Spec, TrackList};
use crate::ui::widgets::{
    self, TILE_COVER, Tile, TileArt, fmt_total, label, plural, section_title, tile_flow, tile_host,
};
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
    Page::plain(s.clone(), Some(s))
}

/// A page with tiles: the scrolled window in a `tile_host`.
fn with_tiles(child: &impl IsA<gtk::Widget>) -> Page {
    let s = scrolled(child);
    Page::plain(tile_host(&s), Some(s))
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

fn not_found_widget() -> gtk::Widget {
    let b = page_box();
    b.append(&widgets::empty("This item is no longer in your library."));
    b.upcast()
}

fn not_found() -> Page {
    simple(&not_found_widget())
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
    // The track list is built when the tile is played, not on every bind:
    // an album may hold 40,000 tracks.
    let (app1, app2, name, lib2) = (app.clone(), app.clone(), a.name.clone(), lib.clone());
    tile.set(
        app,
        TileArt::Cover(Source::for_album(lib, i)),
        &a.name,
        &subtitle,
        move || app1.navigate(Route::Album(key.clone())),
        move || {
            app2.player
                .play_tracks(&name, album_paths(&lib2, i), 0, Some(false))
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
    let (app1, app2, name, name2, lib2) = (
        app.clone(),
        app.clone(),
        ar.name.clone(),
        ar.name.clone(),
        lib.clone(),
    );
    tile.set(
        app,
        TileArt::Cover(source),
        &ar.name,
        "Artist",
        move || app1.navigate(Route::Artist(name.clone())),
        move || {
            let paths = lib2.artists[i]
                .tracks
                .iter()
                .map(|&t| lib2.tracks[t].path.clone())
                .collect();
            app2.player.play_tracks(&name2, paths, 0, Some(false))
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
    with_tiles(&b)
}

// ---- search -------------------------------------------------------------

/// What a search found, worked out on a thread.
struct Found {
    /// The first 50 songs.
    tracks: Vec<String>,
    /// The first 12 artists and albums (library indices).
    artists: Vec<usize>,
    albums: Vec<usize>,
    /// The first 12 playlists, and whether more matched.
    lists: Vec<Summary>,
    more_lists: bool,
    nothing: bool,
}

fn find(lib: &Library, playlists: Vec<Summary>, text: &str) -> Found {
    let q = Query::new(text);
    let r = lib.search(&q);
    let mut lists: Vec<Summary> = playlists
        .into_iter()
        .filter(|p| q.matches(&fold(&p.name)))
        .collect();
    let nothing =
        r.tracks.is_empty() && r.albums.is_empty() && r.artists.is_empty() && lists.is_empty();
    // Twelve tiles like the other sections: "a" matches nearly every
    // playlist of a library, and a tile per playlist stalled the window.
    let more_lists = lists.len() > 12;
    lists.truncate(12);
    Found {
        tracks: r
            .tracks
            .iter()
            .take(50)
            .map(|&i| lib.tracks[i].path.clone())
            .collect(),
        artists: r.artists.iter().take(12).copied().collect(),
        albums: r.albums.iter().take(12).copied().collect(),
        lists,
        more_lists,
        nothing,
    }
}

/// The search page. It stays while the query changes: each query is
/// searched on a thread (100,000 tracks take 5-7 ms), and only the newest
/// one's results are shown.
struct SearchPage {
    text: RefCell<String>,
    /// Bumped by every query; older results are dropped.
    generation: Cell<u64>,
    /// The page title and hint, or "No results".
    message: gtk::Box,
    songs: gtk::Box,
    list: TrackList,
    /// The artist, album and playlist sections.
    tiles: gtk::Box,
}

impl SearchPage {
    fn set_query(self: &Rc<Self>, app: &Rc<App>, text: &str) {
        *self.text.borrow_mut() = text.to_owned();
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        if Query::new(text).is_empty() {
            self.show(app, text, None);
            return;
        }
        let lib = app.store.library();
        let playlists: Vec<Summary> = std::iter::once(app.store.liked_summary())
            .chain(app.store.playlists())
            .collect();
        let (tx, rx) = async_channel::bounded(1);
        let t = text.to_owned();
        let lib2 = lib.clone();
        std::thread::Builder::new()
            .name("search".into())
            .spawn(move || {
                let _ = tx.send_blocking(find(&lib2, playlists, &t));
            })
            .expect("spawn search thread");
        let (me, app, text) = (Rc::downgrade(self), app.clone(), text.to_owned());
        glib::spawn_future_local(async move {
            let Ok(found) = rx.recv().await else { return };
            let Some(me) = me.upgrade() else { return };
            if me.generation.get() == generation {
                me.show(&app, &text, Some((&lib, found)));
            }
        });
    }

    fn show(&self, app: &Rc<App>, text: &str, found: Option<(&Arc<Library>, Found)>) {
        while let Some(c) = self.message.first_child() {
            self.message.remove(&c);
        }
        while let Some(c) = self.tiles.first_child() {
            self.tiles.remove(&c);
        }
        let Some((lib, found)) = found else {
            self.message.append(&label("Search", &["tm-page-title"]));
            self.message.append(&widgets::empty(
                "Find songs, artists, albums and playlists in your library.",
            ));
            self.songs.set_visible(false);
            self.list
                .set_entries(app, Entries::Tracks(Vec::new()), None);
            return;
        };
        if found.nothing {
            self.message
                .append(&widgets::empty(&format!("No results found for “{text}”")));
        }
        self.songs.set_visible(!found.tracks.is_empty());
        self.list.set_context(&format!("Search “{text}”"));
        self.list
            .set_entries(app, Entries::Tracks(found.tracks), None);
        let artists: Vec<gtk::Widget> = found
            .artists
            .iter()
            .map(|&i| {
                let t = Tile::new(true);
                artist_tile(app, lib, i, &t);
                t.widget.upcast()
            })
            .collect();
        if let Some(s) = flow_section("Artists", artists, None) {
            self.tiles.append(&s);
        }
        let albums: Vec<gtk::Widget> = found
            .albums
            .iter()
            .map(|&i| {
                let t = Tile::new(false);
                album_tile(app, lib, i, &t, true);
                t.widget.upcast()
            })
            .collect();
        if let Some(s) = flow_section("Albums", albums, None) {
            self.tiles.append(&s);
        }
        let more = found.more_lists.then(|| (app.clone(), Route::Playlists));
        let lists: Vec<gtk::Widget> = found
            .lists
            .iter()
            .map(|p| playlist_tile(app, lib, p))
            .collect();
        if let Some(s) = flow_section("Playlists", lists, more) {
            self.tiles.append(&s);
        }
    }
}

fn search(app: &Rc<App>, text: &str) -> Page {
    let b = page_box();
    let message = gtk::Box::new(gtk::Orientation::Vertical, 12);
    b.append(&message);
    let songs = gtk::Box::new(gtk::Orientation::Vertical, 12);
    songs.append(&section_title("Songs"));
    let list = tracklist::build(
        app,
        Spec {
            // Set per query when played: see `connect_activate` below.
            context: "Search".into(),
            entries: Entries::Tracks(Vec::new()),
            show_album: true,
            show_cover: true,
            playlist: None,
            header: None,
        },
    );
    list.view.add_css_class("tm-flush");
    songs.append(&list.view);
    b.append(&songs);
    let tiles = gtk::Box::new(gtk::Orientation::Vertical, 12);
    b.append(&tiles);
    let page = Rc::new(SearchPage {
        text: RefCell::new(String::new()),
        generation: Cell::new(0),
        message,
        songs,
        list,
        tiles,
    });
    page.set_query(app, text);
    let mut out = with_tiles(&b);
    let p = page.clone();
    out.retarget = Some(Rc::new(move |app, route| match route {
        Route::Search(q) => {
            p.set_query(app, q);
            true
        }
        _ => false,
    }));
    let p = page.clone();
    out.update = Some(Rc::new(move |app, _| {
        let text = p.text.borrow().clone();
        p.set_query(app, &text);
        true
    }));
    out.drain = Some(Rc::new(move || page.list.drain_step()));
    out
}

// ---- library grids --------------------------------------------------------

/// Binds the tile at a position of a grid.
type BindTile = Box<dyn Fn(&Rc<App>, usize, &Tile)>;

/// What a grid shows: an identity per tile (what tells which tiles an
/// update leaves alone) and how to bind tile `i`.
struct GridData {
    keys: Vec<String>,
    bind: BindTile,
}

type GridSource = Rc<dyn Fn(&Rc<App>) -> GridData>;

struct Grid {
    rows: LazyRows,
    view: gtk::GridView,
    keys: RefCell<Vec<String>>,
    bind: RefCell<BindTile>,
    /// Every tile the view has made, with its list item.
    tiles: RefCell<Vec<(glib::WeakRef<gtk::ListItem>, Tile)>>,
}

/// Tile rows a grid adds per frame while it fills.
const GRID_CHUNK_ROWS: u32 = 4;

impl Grid {
    /// Sizes GTK's tile window to the columns that fit `width`: GTK makes
    /// `max_columns` x 31 tiles whatever the width, 372 with 12 columns.
    fn fit(&self, width: f64) {
        let tile = self
            .view
            .first_child()
            .map(|c| c.measure(gtk::Orientation::Horizontal, -1).0)
            .filter(|w| *w > 0)
            .unwrap_or(TILE_COVER + 24) as f64;
        let cols = ((width / tile).floor() as u32).max(2);
        if cols != self.view.max_columns() {
            self.view.set_max_columns(cols);
        }
        self.rows.set_window(cols * 31, cols * GRID_CHUNK_ROWS);
    }

    fn update(&self, app: &Rc<App>, data: GridData) {
        let (p, removed, added) = rows::diff(&self.keys.borrow(), &data.keys);
        *self.keys.borrow_mut() = data.keys;
        *self.bind.borrow_mut() = data.bind;
        self.rows.splice(p as u32, removed as u32, added as u32);
        // Tiles that stayed show what changed in them (counts, covers).
        let bound: Vec<(u32, Tile)> = self
            .tiles
            .borrow()
            .iter()
            .filter_map(|(li, t)| Some((rows::position(li)?, t.clone())))
            .collect();
        let bind = self.bind.borrow();
        for (pos, tile) in bound {
            bind(app, pos as usize, &tile);
        }
    }
}

fn grid_page(
    app: &Rc<App>,
    title: &str,
    action: Option<gtk::Widget>,
    round: bool,
    source: GridSource,
) -> Page {
    let data = source(app);
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
    outer.append(&head);
    if data.keys.is_empty() {
        outer.append(&widgets::empty("Nothing here yet."));
        let mut page = Page::plain(outer, None);
        // Still empty: nothing to do; else the page is built again.
        page.update = Some(Rc::new(move |app, _| source(app).keys.is_empty()));
        return page;
    }
    let factory = gtk::SignalListItemFactory::new();
    let view = gtk::GridView::builder()
        .factory(&factory)
        .min_columns(2)
        .css_classes(["tm-grid"])
        .margin_start(12)
        .margin_end(12)
        .build();
    let grid = Rc::new(Grid {
        rows: LazyRows::new(0, 1),
        view: view.clone(),
        keys: RefCell::new(Vec::new()),
        bind: RefCell::new(data.bind),
        tiles: RefCell::new(Vec::new()),
    });
    view.set_model(Some(&gtk::NoSelection::new(Some(grid.rows.clone()))));
    let weak = Rc::downgrade(&grid);
    factory.connect_setup(move |_, obj| {
        let Some(grid) = weak.upgrade() else { return };
        let li = obj.downcast_ref::<gtk::ListItem>().expect("list item");
        // Clicks are the tile host's; the view selects nothing.
        li.set_activatable(false);
        li.set_selectable(false);
        let tile = Tile::new(round);
        li.set_child(Some(&tile.widget));
        grid.rows.track(li);
        grid.tiles.borrow_mut().push((li.downgrade(), tile));
    });
    let (weak, a) = (Rc::downgrade(&grid), app.clone());
    factory.connect_bind(move |_, obj| {
        let Some(grid) = weak.upgrade() else { return };
        let li = obj.downcast_ref::<gtk::ListItem>().expect("list item");
        let tile = Tile::of(&li.child().expect("tile"));
        (grid.bind.borrow())(&a, li.position() as usize, &tile);
    });
    let weak = Rc::downgrade(&grid);
    factory.connect_teardown(move |_, obj| {
        let li = obj.downcast_ref::<gtk::ListItem>().expect("list item");
        if let Some(grid) = weak.upgrade() {
            grid.tiles
                .borrow_mut()
                .retain(|(l, _)| l.upgrade().is_some_and(|l| l != *li));
        }
    });
    let s = scrolled(&view);
    // Until the grid has a width: the window's, less the sidebar.
    let width = match app.window.width() {
        0 => app.window.default_width(),
        w => w,
    };
    grid.fit((width - 340).max(400) as f64);
    if let Some(adj) = view.hadjustment() {
        let weak = Rc::downgrade(&grid);
        adj.connect_page_size_notify(move |adj| {
            if let Some(grid) = weak.upgrade()
                && adj.page_size() > 0.0
            {
                grid.fit(adj.page_size());
            }
        });
    }
    *grid.keys.borrow_mut() = data.keys;
    grid.rows.reset(grid.keys.borrow().len() as u32, &view);
    outer.append(&tile_host(&s));
    let mut page = Page::plain(outer, Some(s));
    let g = grid.clone();
    page.update = Some(Rc::new(move |app, _| {
        let data = source(app);
        if data.keys.is_empty() {
            return false;
        }
        g.update(app, data);
        true
    }));
    page.drain = Some(Rc::new(move || grid.rows.drain_step()));
    page
}

fn albums(app: &Rc<App>) -> Page {
    grid_page(
        app,
        "Albums",
        None,
        false,
        Rc::new(|app| {
            let lib = app.store.library();
            let order = lib.albums_by_name();
            GridData {
                keys: order.iter().map(|&i| lib.albums[i].key()).collect(),
                bind: Box::new(move |app, i, tile| album_tile(app, &lib, order[i], tile, true)),
            }
        }),
    )
}

fn artists(app: &Rc<App>) -> Page {
    grid_page(
        app,
        "Artists",
        None,
        true,
        Rc::new(|app| {
            let lib = app.store.library();
            GridData {
                keys: lib.artists.iter().map(|a| a.name.clone()).collect(),
                bind: Box::new(move |app, i, tile| artist_tile(app, &lib, i, tile)),
            }
        }),
    )
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
    grid_page(
        app,
        "Playlists",
        Some(create.upcast()),
        false,
        Rc::new(|app| {
            let lib = app.store.library();
            let lists: Vec<Summary> = std::iter::once(app.store.liked_summary())
                .chain(app.store.playlists())
                .collect();
            GridData {
                keys: lists.iter().map(|p| p.name.clone()).collect(),
                bind: Box::new(move |app, i, tile| bind_playlist_tile(app, &lib, &lists[i], tile)),
            }
        }),
    )
}

pub fn create_playlist_dialog(app: &Rc<App>) {
    let a = app.clone();
    menu::ask_text(app, "New playlist", "", "Create", move |name| {
        let a2 = a.clone();
        a.store.create_playlist(&name, move |r| match r {
            Ok(name) => a2.navigate(Route::Playlist(name)),
            Err(e) => a2.error("Could not create playlist", e),
        });
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

/// A detail page's header and songs, as `parts` makes them from the
/// library: `None` once the album or artist is gone.
type DetailParts = Box<dyn Fn(&Rc<App>) -> Option<(gtk::Box, Vec<String>)>>;

/// An album or artist page: its header as row 0 of the track list. When
/// the library changes, the header is made again and only the rows that
/// changed are replaced.
fn detail_page(app: &Rc<App>, context: &str, show_album: bool, parts: DetailParts) -> Page {
    let Some((header, paths)) = parts(app) else {
        return not_found();
    };
    let list = tracklist::build(
        app,
        Spec {
            context: context.to_owned(),
            entries: Entries::Tracks(paths),
            show_album,
            show_cover: show_album,
            playlist: None,
            header: Some(header.upcast()),
        },
    );
    list.view.set_margin_bottom(24);
    let s = scrolled(&list.view);
    let mut page = Page::plain(tile_host(&s), Some(s));
    let list = Rc::new(list);
    let l = list.clone();
    page.update = Some(Rc::new(move |app, change| {
        if change != StoreChange::Library {
            return true;
        }
        match parts(app) {
            Some((header, paths)) => {
                l.set_entries(app, Entries::Tracks(paths), Some(header.upcast()));
                true
            }
            None => false,
        }
    }));
    page.drain = Some(Rc::new(move || list.drain_step()));
    page
}

fn album(app: &Rc<App>, key: &str) -> Page {
    let name = {
        let lib = app.store.library();
        match lib.album_index(key) {
            Some(i) => lib.albums[i].name.clone(),
            None => return not_found(),
        }
    };
    let key = key.to_owned();
    detail_page(
        app,
        &name,
        false,
        Box::new(move |app| {
            let lib = app.store.library();
            let ai = lib.album_index(&key)?;
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
            if !paths.is_empty() {
                header.append(&tracklist::columns_header(false, false));
            }
            Some((header, paths))
        }),
    )
}

fn artist(app: &Rc<App>, name: &str) -> Page {
    let name = name.to_owned();
    detail_page(
        app,
        &name.clone(),
        true,
        Box::new(move |app| {
            let lib = app.store.library();
            let ari = lib.artist_index(&name)?;
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
            if !paths.is_empty() {
                header.append(&tracklist::columns_header(true, true));
            }
            Some((header, paths))
        }),
    )
}

/// What a playlist page shows, read off the main loop: a playlist may hold
/// 250,000 entries.
struct LoadedPlaylist {
    p: Playlist,
    paths: Vec<String>,
    total_ms: u64,
    art: Source,
}

/// A playlist page: its entries are read on a thread, first and after
/// every change to the playlist, and the list is updated in place.
struct PlaylistPage {
    name: String,
    scroller: glib::WeakRef<gtk::ScrolledWindow>,
    list: RefCell<Option<TrackList>>,
    /// Bumped by every read; an older read's result is dropped.
    generation: Cell<u64>,
}

impl PlaylistPage {
    fn load(self: &Rc<Self>, app: &Rc<App>) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let lib = app.store.library();
        let name = &self.name;
        let read: Box<dyn FnOnce() -> Option<Playlist> + Send> = if name == LIKED_SONGS {
            let p = app.store.liked();
            Box::new(move || Some(p))
        } else if app.store.has_playlist(name) {
            let (root, n) = (app.store.root(), name.clone());
            Box::new(move || {
                Playlist::read(&root, &n)
                    .inspect_err(|e| eprintln!("tunemesh: cannot read playlist {n:?}: {e}"))
                    .ok()
            })
        } else {
            Box::new(|| None)
        };
        let (tx, rx) = async_channel::bounded::<Option<LoadedPlaylist>>(1);
        let n = name.clone();
        std::thread::Builder::new()
            .name("playlist-page".into())
            .spawn(move || {
                let loaded = read().map(|p| {
                    let paths = available_paths(&lib, &p);
                    let total_ms = paths
                        .iter()
                        .filter_map(|t| lib.track(t))
                        .map(|t| t.duration_ms)
                        .sum();
                    let art =
                        Source::for_paths(&lib, p.entries.iter().filter_map(|e| e.track()), &n);
                    LoadedPlaylist {
                        p,
                        paths,
                        total_ms,
                        art,
                    }
                });
                let _ = tx.send_blocking(loaded);
            })
            .expect("spawn playlist thread");
        let (me, app) = (Rc::downgrade(self), app.clone());
        glib::spawn_future_local(async move {
            let Ok(loaded) = rx.recv().await else { return };
            let Some(me) = me.upgrade() else { return };
            if me.generation.get() == generation {
                me.show(&app, loaded);
            }
        });
    }

    fn show(&self, app: &Rc<App>, loaded: Option<LoadedPlaylist>) {
        let Some(s) = self.scroller.upgrade() else {
            return;
        };
        let Some(l) = loaded else {
            self.list.borrow_mut().take();
            s.set_child(Some(&not_found_widget()));
            return;
        };
        let (header, entries) = playlist_parts(app, &self.name, l);
        if let Some(list) = self.list.borrow().as_ref() {
            list.set_entries(app, entries, Some(header.upcast()));
            return;
        }
        let list = tracklist::build(
            app,
            Spec {
                context: self.name.clone(),
                entries,
                show_album: true,
                show_cover: true,
                playlist: Some(self.name.clone()),
                header: Some(header.upcast()),
            },
        );
        list.view.set_margin_bottom(24);
        s.set_child(Some(&list.view));
        *self.list.borrow_mut() = Some(list);
    }
}

fn playlist(app: &Rc<App>, name: &str) -> Page {
    if app.store.is_too_large(name) {
        let mut page = too_large(name);
        let n = name.to_owned();
        page.update = Some(Rc::new(move |app, _| app.store.is_too_large(&n)));
        return page;
    }
    if name != LIKED_SONGS && !app.store.has_playlist(name) {
        return not_found();
    }
    let s = scrolled(&gtk::Box::new(gtk::Orientation::Vertical, 0));
    let state = Rc::new(PlaylistPage {
        name: name.to_owned(),
        scroller: s.downgrade(),
        list: RefCell::new(None),
        generation: Cell::new(0),
    });
    state.load(app);
    let mut page = Page::plain(tile_host(&s), Some(s));
    let st = state.clone();
    page.update = Some(Rc::new(move |app, _| {
        if app.store.is_too_large(&st.name) {
            return false;
        }
        st.load(app);
        true
    }));
    page.drain = Some(Rc::new(move || {
        state
            .list
            .borrow()
            .as_ref()
            .is_none_or(TrackList::drain_step)
    }));
    page
}

/// The playlist's header and entries, once read.
fn playlist_parts(app: &Rc<App>, name: &str, l: LoadedPlaylist) -> (gtk::Box, Entries) {
    let liked = name == LIKED_SONGS;
    let LoadedPlaylist {
        p,
        paths,
        total_ms,
        art,
    } = l;
    let mut parts: Vec<gtk::Widget> =
        vec![gtk::Label::new(Some(&plural(p.entries.len(), "song", "songs"))).upcast()];
    if total_ms > 0 {
        parts.push(gtk::Label::new(Some(&fmt_total(total_ms))).upcast());
    }
    let art = if liked {
        HeroArt::Liked
    } else {
        HeroArt::Cover(art)
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
    } else {
        header.append(&tracklist::columns_header(true, true));
    }
    (header, Entries::Playlist(p))
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
                    let a3 = a2.clone();
                    let o = old.clone();
                    a2.store.rename_playlist(&old, &new, move |r| match r {
                        Ok(new) => a3.playlist_renamed(&o, &new),
                        Err(e) => a3.error("Could not rename playlist", e),
                    });
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
                    move || {
                        let a3 = a2.clone();
                        let n3 = n2.clone();
                        a2.store.delete_playlist(&n2, move |r| match r {
                            Ok(()) => {
                                a3.toast(&format!("Deleted {n3}"));
                                a3.navigate(Route::Playlists);
                            }
                            Err(e) => a3.error("Could not delete playlist", e),
                        });
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
