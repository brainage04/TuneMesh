//! The virtualised track list used by album, artist, playlist and search
//! pages: double-click plays, right-click opens the track menu, playlists
//! reorder by drag and drop. A page header can ride along as row 0.
//!
//! A row is a few labels, the cover and the heart, without controllers of
//! its own: the clicks, drags and drops of every row are handled once on
//! the list view, which finds the row under the pointer. GTK keeps 200 rows
//! and three controllers per row were most of what they cost to make and to
//! destroy.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

use tunemesh_core::{Playlist, limits};

use crate::app::App;
use crate::covers::Source;
use crate::store::EntryRef;
use crate::ui::menu::{self, MenuCtx};
use crate::ui::rows::{self, LIST_CHUNK, LIST_WINDOW, LazyRows};
use crate::ui::widgets::{fmt_duration, label};

/// What a track list shows. A playlist is held once, as read: a long one
/// costs no second copy, and rows are made only for what is on screen.
pub enum Entries {
    /// Library paths (albums, artists, search results).
    Tracks(Vec<String>),
    Playlist(Playlist),
}

impl Entries {
    fn len(&self) -> usize {
        match self {
            Entries::Tracks(v) => v.len(),
            Entries::Playlist(p) => p.entries.len(),
        }
    }

    /// Library path; `None` for playlist lines pointing outside the library.
    fn path(&self, i: usize) -> Option<&str> {
        match self {
            Entries::Tracks(v) => v.get(i).map(String::as_str),
            Entries::Playlist(p) => p.entries.get(i)?.track(),
        }
    }

    /// Playlist pages: the entry's identity (`store::entry_key`), so edits
    /// find it again in a playlist another device changed.
    fn key(&self, i: usize) -> Option<&str> {
        match self {
            Entries::Tracks(_) => None,
            Entries::Playlist(p) => p.entries.get(i).map(crate::store::entry_key),
        }
    }

    /// What tells two versions of a list which rows stayed the same.
    fn identity(&self, i: usize) -> Option<&str> {
        match self {
            Entries::Tracks(v) => v.get(i).map(String::as_str),
            Entries::Playlist(_) => self.key(i),
        }
    }

    /// Display text when the track is unavailable.
    fn label(&self, i: usize) -> String {
        match self {
            Entries::Tracks(_) => String::new(),
            Entries::Playlist(p) => p.entries.get(i).map_or_else(String::new, |e| {
                e.label().map(str::to_owned).unwrap_or_else(|| {
                    let raw = e.key();
                    tunemesh_core::library::title_from_file_name(
                        raw.rsplit('/').next().unwrap_or(raw),
                    )
                })
            }),
        }
    }
}

/// What a playlist row drag carries. Only rows of this app create it, so
/// text dropped from other apps cannot reorder a playlist.
struct DraggedEntry {
    playlist: String,
    from: EntryRef,
}

pub struct Spec {
    /// Context name for "playing from".
    pub context: String,
    pub entries: Entries,
    pub show_album: bool,
    pub show_cover: bool,
    /// Enables remove-from-playlist and drag-and-drop reordering.
    pub playlist: Option<String>,
    pub header: Option<gtk::Widget>,
}

#[derive(Clone)]
struct RowW {
    root: gtk::Box,
    index: gtk::Label,
    cover: Option<gtk::Image>,
    title: gtk::Label,
    artist: gtk::Label,
    album: Option<gtk::Label>,
    duration: gtk::Label,
    heart: gtk::Image,
    /// The list item showing this row; its position is the row's.
    li: glib::WeakRef<gtk::ListItem>,
}

pub struct Shared {
    entries: RefCell<Entries>,
    /// 1 when the page header is row 0.
    offset: u32,
    rows: LazyRows,
    /// The page header's slot, if any.
    header: Option<gtk::Box>,
    /// Every row the list has made (bound or not).
    widgets: RefCell<Vec<RowW>>,
    /// Context name for "playing from".
    context: RefCell<String>,
}

/// A built list: the view to put into a scrolled window, and what updates
/// it in place.
pub struct TrackList {
    pub view: gtk::ListView,
    pub shared: Rc<Shared>,
}

impl Shared {
    fn entry_ref(&self, i: usize) -> Option<EntryRef> {
        Some((i, self.entries.borrow().key(i)?.to_owned()))
    }

    fn available(&self, app: &App, i: usize) -> Option<String> {
        let entries = self.entries.borrow();
        let path = entries.path(i)?;
        app.store
            .library()
            .track_index(path)
            .map(|_| path.to_owned())
    }

    /// The entry a row shows, if it shows one.
    fn entry_of(&self, r: &RowW) -> Option<usize> {
        let li = r.li.upgrade()?;
        if li.child().as_ref() != Some(r.root.upcast_ref()) {
            return None;
        }
        let p = rows::position(&r.li)?;
        p.checked_sub(self.offset).map(|i| i as usize)
    }

    /// The row `w` (picked under the pointer) is part of, and its entry.
    fn row_at(&self, w: Option<gtk::Widget>) -> Option<(RowW, usize)> {
        let mut w = w;
        while let Some(c) = w {
            if c.has_css_class("tm-track-row") {
                let r = self
                    .widgets
                    .borrow()
                    .iter()
                    .find(|r| *r.root.upcast_ref::<gtk::Widget>() == c)
                    .cloned()?;
                let i = self.entry_of(&r)?;
                return Some((r, i));
            }
            w = c.parent();
        }
        None
    }

    pub fn refresh_rows(&self, app: &App) {
        let widgets = self.widgets.borrow().clone();
        for r in &widgets {
            if let Some(i) = self.entry_of(r) {
                self.update_row(app, r, i);
            }
        }
    }

    fn update_row(&self, app: &App, r: &RowW, i: usize) {
        let lib = app.store.library();
        let entries = self.entries.borrow();
        let track = entries.path(i).and_then(|p| lib.track(p));
        r.index.set_label(&(i + 1).to_string());
        match track {
            Some(t) => {
                r.title.set_label(&limits::clip_line(&t.title));
                r.artist.set_label(&limits::clip_line(&t.artist));
                if let Some(a) = &r.album {
                    a.set_label(&limits::clip_line(&t.album));
                }
                r.duration
                    .set_label(&t.duration_secs().map(fmt_duration).unwrap_or_default());
                if let Some(c) = &r.cover {
                    app.covers.apply(c, Source::for_track(t), 40);
                }
                r.root.remove_css_class("unavailable");
                r.heart.set_visible(true);
                let liked = app.store.is_liked(&t.path);
                r.heart.set_icon_name(Some(if liked {
                    "tm-heart-filled-symbolic"
                } else {
                    "tm-heart-outline-symbolic"
                }));
                if liked {
                    r.heart.add_css_class("liked");
                } else {
                    r.heart.remove_css_class("liked");
                }
                if app.player.current_path().as_deref() == Some(t.path.as_str()) {
                    r.root.add_css_class("current");
                } else {
                    r.root.remove_css_class("current");
                }
            }
            None => {
                let label = entries.label(i);
                r.title.set_label(&limits::clip_line(&label));
                r.artist.set_label("Unavailable on this device");
                if let Some(a) = &r.album {
                    a.set_label("");
                }
                r.duration.set_label("");
                if let Some(c) = &r.cover {
                    app.covers.apply(c, Source::Placeholder(label), 40);
                }
                r.root.add_css_class("unavailable");
                r.root.remove_css_class("current");
                r.heart.set_visible(false);
            }
        }
    }
}

impl TrackList {
    /// Shows `entries` (and `header` in the page header's slot) in place:
    /// only the rows between what stayed the same at the start and at the
    /// end are replaced.
    pub fn set_entries(&self, app: &App, entries: Entries, header: Option<gtk::Widget>) {
        let sh = &self.shared;
        let (p, removed, added) = {
            let old = sh.entries.borrow();
            rows::diff_by(old.len(), entries.len(), |i, j| {
                old.identity(i) == entries.identity(j)
            })
        };
        *sh.entries.borrow_mut() = entries;
        if let (Some(slot), Some(h)) = (&sh.header, header) {
            while let Some(c) = slot.first_child() {
                slot.remove(&c);
            }
            slot.append(&h);
        }
        sh.rows
            .splice(sh.offset + p as u32, removed as u32, added as u32);
        sh.refresh_rows(app);
    }

    /// Sets the context name a double-click plays the list as.
    pub fn set_context(&self, context: &str) {
        *self.shared.context.borrow_mut() = context.to_owned();
    }

    /// Takes a chunk of rows away; whether the list is empty (see
    /// `LazyRows::drain_step`).
    pub fn drain_step(&self) -> bool {
        self.shared.rows.drain_step()
    }
}

fn new_row(spec_album: bool, spec_cover: bool) -> RowW {
    let root = gtk::Box::builder()
        .spacing(16)
        .css_classes(["tm-track-row"])
        .build();
    let index = gtk::Label::builder()
        .width_chars(3)
        .xalign(1.0)
        .css_classes(["index"])
        .build();
    root.append(&index);
    let cover = spec_cover.then(|| {
        let p = crate::ui::widgets::cover_image(40);
        root.append(&p);
        p
    });
    // Title and album share the free width equally, in rows and the header.
    let cols = gtk::Box::builder()
        .spacing(16)
        .homogeneous(true)
        .hexpand(true)
        .build();
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .valign(gtk::Align::Center)
        .build();
    let title = label("", &["title"]);
    title.set_max_width_chars(1);
    let artist = label("", &["artist"]);
    artist.set_max_width_chars(1);
    text.append(&title);
    text.append(&artist);
    cols.append(&text);
    let album = spec_album.then(|| {
        let a = label("", &["album"]);
        a.set_max_width_chars(1);
        cols.append(&a);
        a
    });
    root.append(&cols);
    let heart = heart_image();
    root.append(&heart);
    let duration = gtk::Label::builder()
        .width_request(54)
        .xalign(1.0)
        .css_classes(["duration"])
        .build();
    root.append(&duration);
    RowW {
        root,
        index,
        cover,
        title,
        artist,
        album,
        duration,
        heart,
        li: glib::WeakRef::new(),
    }
}

fn heart_image() -> gtk::Image {
    gtk::Image::builder()
        .icon_name("tm-heart-outline-symbolic")
        .css_classes(["tm-heart"])
        .valign(gtk::Align::Center)
        .build()
}

/// The column captions shown above the rows.
pub fn columns_header(show_album: bool, show_cover: bool) -> gtk::Box {
    let b = gtk::Box::builder()
        .spacing(16)
        .css_classes(["tm-tracks-head"])
        .build();
    b.append(
        &gtk::Label::builder()
            .label("#")
            .width_chars(3)
            .xalign(1.0)
            .build(),
    );
    if show_cover {
        b.append(&gtk::Box::builder().width_request(40).build());
    }
    let cols = gtk::Box::builder()
        .spacing(16)
        .homogeneous(true)
        .hexpand(true)
        .build();
    cols.append(&gtk::Label::builder().label("Title").xalign(0.0).build());
    if show_album {
        cols.append(&gtk::Label::builder().label("Album").xalign(0.0).build());
    }
    b.append(&cols);
    // Same size as the rows' heart.
    let spacer = heart_image();
    spacer.set_opacity(0.0);
    b.append(&spacer);
    let clock = gtk::Image::from_icon_name("document-open-recent-symbolic");
    let clock_box = gtk::Box::builder().width_request(54).hexpand(false).build();
    clock.set_hexpand(true);
    clock.set_halign(gtk::Align::End);
    clock_box.append(&clock);
    b.append(&clock_box);
    b
}

/// The row data stored on a list item.
fn row_of(li: &gtk::ListItem) -> RowW {
    // SAFETY: set in setup with this type.
    unsafe { li.data::<RowW>("tm-row").expect("row").as_ref().clone() }
}

/// Builds the list. Put it directly into a `ScrolledWindow` for long lists.
pub fn build(app: &Rc<App>, spec: Spec) -> TrackList {
    let offset = u32::from(spec.header.is_some());
    let header = spec.header.map(|h| {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
        b.add_css_class("tm-header-block");
        b.append(&h);
        b
    });
    let shared = Rc::new(Shared {
        entries: RefCell::new(spec.entries),
        offset,
        rows: LazyRows::new(LIST_WINDOW, LIST_CHUNK),
        header,
        widgets: RefCell::new(Vec::new()),
        context: RefCell::new(spec.context),
    });
    app.register_track_list(&shared);
    let selection = gtk::MultiSelection::new(Some(shared.rows.clone()));

    let factory = gtk::SignalListItemFactory::new();
    let (show_album, show_cover) = (spec.show_album, spec.show_cover);
    // Closures on the factory hold the shared list weakly: it holds the
    // rows, so a strong reference would keep the whole list alive after its
    // page is gone.
    let weak = Rc::downgrade(&shared);
    factory.connect_setup(move |_, obj| {
        let Some(sh) = weak.upgrade() else { return };
        let li = obj.downcast_ref::<gtk::ListItem>().expect("list item");
        let row = new_row(show_album, show_cover);
        row.li.set(Some(li));
        li.set_child(Some(&row.root));
        sh.rows.track(li);
        sh.widgets.borrow_mut().push(row.clone());
        // SAFETY: only ever read back as `RowW` (`row_of`).
        unsafe { li.set_data("tm-row", row) };
    });
    let (weak, a) = (Rc::downgrade(&shared), app.clone());
    factory.connect_bind(move |_, obj| {
        let Some(sh) = weak.upgrade() else { return };
        let li = obj.downcast_ref::<gtk::ListItem>().expect("list item");
        let row = row_of(li);
        let pos = li.position();
        match &sh.header {
            Some(h) if pos == 0 => {
                // The header can be shown by one item at a time.
                if h.parent().is_some() {
                    let holder = sh.widgets.borrow().iter().find_map(|r| {
                        r.li.upgrade()
                            .filter(|l| l.child().as_ref() == Some(h.upcast_ref()))
                            .map(|l| (l, r.root.clone()))
                    });
                    if let Some((l, root)) = holder {
                        l.set_child(Some(&root));
                    }
                }
                li.set_child(Some(h));
                li.set_activatable(false);
                li.set_selectable(false);
                li.set_focusable(false);
            }
            _ => {
                if li.child().as_ref() != Some(row.root.upcast_ref()) {
                    li.set_child(Some(&row.root));
                }
                li.set_activatable(true);
                li.set_selectable(true);
                li.set_focusable(true);
                sh.update_row(&a, &row, (pos - sh.offset) as usize);
            }
        }
    });
    factory.connect_unbind(move |_, obj| {
        let li = obj.downcast_ref::<gtk::ListItem>().expect("list item");
        let row = row_of(li);
        if li.child().as_ref() != Some(row.root.upcast_ref()) {
            li.set_child(Some(&row.root));
        }
    });
    let weak = Rc::downgrade(&shared);
    factory.connect_teardown(move |_, obj| {
        let li = obj.downcast_ref::<gtk::ListItem>().expect("list item");
        if let Some(sh) = weak.upgrade() {
            let root = row_of(li).root;
            sh.widgets.borrow_mut().retain(|r| r.root != root);
        }
    });

    let view = gtk::ListView::builder()
        .model(&selection)
        .factory(&factory)
        .single_click_activate(false)
        .css_classes(["tm-tracks"])
        .build();
    connect_pointer(app, &view, &shared, &selection, spec.playlist.clone());
    if let Some(name) = spec.playlist {
        connect_reorder(app, &view, &shared, name);
    }
    let (a, weak) = (app.clone(), Rc::downgrade(&shared));
    view.connect_activate(move |_, model_pos| {
        let Some(sh) = weak.upgrade() else { return };
        if model_pos < sh.offset {
            return;
        }
        let i = (model_pos - sh.offset) as usize;
        let len = sh.entries.borrow().len();
        let mut tracks = Vec::new();
        let mut start = 0;
        for k in 0..len {
            if let Some(p) = sh.available(&a, k) {
                if k == i {
                    start = tracks.len();
                }
                tracks.push(p);
            }
        }
        if sh.available(&a, i).is_some() {
            let context = sh.context.borrow().clone();
            a.player.play_tracks(&context, tracks, start, None);
        }
    });
    shared
        .rows
        .reset(shared.entries.borrow().len() as u32 + offset, &view);
    TrackList { view, shared }
}

/// The widget under `(x, y)` of `view`.
fn picked(view: &gtk::ListView, x: f64, y: f64) -> Option<gtk::Widget> {
    view.pick(x, y, gtk::PickFlags::DEFAULT)
}

/// Right-click menus, the heart and its tooltip, for every row.
fn connect_pointer(
    app: &Rc<App>,
    view: &gtk::ListView,
    shared: &Rc<Shared>,
    selection: &gtk::MultiSelection,
    playlist: Option<String>,
) {
    // Capture phase: before a row's own gesture selects it.
    let click = gtk::GestureClick::builder()
        .button(0)
        .propagation_phase(gtk::PropagationPhase::Capture)
        .build();
    let (a, sh, sel) = (app.clone(), Rc::downgrade(shared), selection.downgrade());
    click.connect_pressed(move |g, _, x, y| {
        let (Some(sh), Some(sel), Some(view)) = (
            sh.upgrade(),
            sel.upgrade(),
            g.widget().and_downcast::<gtk::ListView>(),
        ) else {
            return;
        };
        let target = picked(&view, x, y);
        let Some((row, i)) = sh.row_at(target.clone()) else {
            g.set_state(gtk::EventSequenceState::Denied);
            return;
        };
        if g.current_button() == gdk::BUTTON_PRIMARY {
            if target.as_ref() == Some(row.heart.upcast_ref()) {
                g.set_state(gtk::EventSequenceState::Claimed);
                if let Some(path) = sh.available(&a, i) {
                    a.store
                        .toggle_like(&path, a.on_error("Could not update Liked Songs"));
                }
            } else {
                g.set_state(gtk::EventSequenceState::Denied);
            }
            return;
        }
        if g.current_button() != gdk::BUTTON_SECONDARY {
            g.set_state(gtk::EventSequenceState::Denied);
            return;
        }
        g.set_state(gtk::EventSequenceState::Claimed);
        let model_pos = i as u32 + sh.offset;
        let mut targets: Vec<usize> = Vec::new();
        if sel.is_selected(model_pos) && sel.selection().size() > 1 {
            let bits = sel.selection();
            for k in 0..bits.size() {
                let m = bits.nth(k as u32);
                if m >= sh.offset {
                    targets.push((m - sh.offset) as usize);
                }
            }
        } else {
            sel.select_item(model_pos, true);
            targets.push(i);
        }
        let paths: Vec<String> = targets
            .iter()
            .filter_map(|&t| sh.available(&a, t))
            .collect();
        let ctx = MenuCtx {
            paths,
            playlist: playlist.clone().map(|name| {
                (
                    name,
                    targets.iter().filter_map(|&t| sh.entry_ref(t)).collect(),
                )
            }),
            queue_index: None,
        };
        let at = view
            .compute_point(&row.root, &gtk::graphene::Point::new(x as f32, y as f32))
            .unwrap_or_else(|| gtk::graphene::Point::new(x as f32, y as f32));
        menu::popup(&a, row.root.upcast_ref(), at.x() as f64, at.y() as f64, ctx);
    });
    view.add_controller(click);

    view.set_has_tooltip(true);
    let (a, sh) = (app.clone(), Rc::downgrade(shared));
    view.connect_query_tooltip(move |view, x, y, keyboard, tooltip| {
        let Some(sh) = sh.upgrade().filter(|_| !keyboard) else {
            return false;
        };
        let target = picked(view, x as f64, y as f64);
        let Some((row, i)) = sh.row_at(target.clone()) else {
            return false;
        };
        if target.as_ref() != Some(row.heart.upcast_ref()) {
            return false;
        }
        let Some(path) = sh.available(&a, i) else {
            return false;
        };
        tooltip.set_text(Some(if a.store.is_liked(&path) {
            "Remove from Liked Songs"
        } else {
            "Save to Liked Songs"
        }));
        true
    });
}

/// Drag and drop to reorder a playlist's rows.
fn connect_reorder(app: &Rc<App>, view: &gtk::ListView, shared: &Rc<Shared>, name: String) {
    let source = gtk::DragSource::builder()
        .actions(gdk::DragAction::MOVE)
        .build();
    // The row being dragged, for the drag icon.
    let dragged: Rc<RefCell<Option<gtk::Box>>> = Rc::default();
    let (sh, n, d) = (Rc::downgrade(shared), name.clone(), dragged.clone());
    source.connect_prepare(move |s, x, y| {
        let sh = sh.upgrade()?;
        let view = s.widget().and_downcast::<gtk::ListView>()?;
        let (row, i) = sh.row_at(picked(&view, x, y))?;
        let from = sh.entry_ref(i)?;
        *d.borrow_mut() = Some(row.root.clone());
        let payload = glib::BoxedAnyObject::new(DraggedEntry {
            playlist: n.clone(),
            from,
        });
        Some(gdk::ContentProvider::for_value(&payload.to_value()))
    });
    source.connect_drag_begin(move |s, _| {
        if let Some(root) = dragged.borrow_mut().take() {
            s.set_icon(Some(&gtk::WidgetPaintable::new(Some(&root))), 0, 0);
        }
    });
    view.add_controller(source);

    let target = gtk::DropTarget::new(glib::BoxedAnyObject::static_type(), gdk::DragAction::MOVE);
    // The row marked as where the drop goes.
    let marked: Rc<RefCell<Option<glib::WeakRef<gtk::Box>>>> = Rc::default();
    let unmark = {
        let marked = marked.clone();
        move || {
            if let Some(r) = marked.borrow_mut().take().and_then(|w| w.upgrade()) {
                r.remove_css_class("tm-drop-before");
            }
        }
    };
    let (sh, m, u) = (Rc::downgrade(shared), marked.clone(), unmark.clone());
    target.connect_motion(move |t, x, y| {
        let row = sh.upgrade().and_then(|sh| {
            let view = t.widget().and_downcast::<gtk::ListView>()?;
            sh.row_at(picked(&view, x, y))
        });
        let current = m.borrow().as_ref().and_then(|w| w.upgrade());
        if row.as_ref().map(|(r, _)| &r.root) != current.as_ref() {
            u();
            if let Some((r, _)) = row {
                r.root.add_css_class("tm-drop-before");
                *m.borrow_mut() = Some(r.root.downgrade());
            }
        }
        gdk::DragAction::MOVE
    });
    let u = unmark.clone();
    target.connect_leave(move |_| u());
    let (a, sh) = (app.clone(), Rc::downgrade(shared));
    target.connect_drop(move |t, value, x, y| {
        unmark();
        let Some(sh) = sh.upgrade() else { return false };
        let Some(view) = t.widget().and_downcast::<gtk::ListView>() else {
            return false;
        };
        let Some(to) = sh
            .row_at(picked(&view, x, y))
            .and_then(|(_, i)| sh.entry_ref(i))
        else {
            return false;
        };
        let Ok(obj) = value.get::<glib::BoxedAnyObject>() else {
            return false;
        };
        let Ok(dragged) = obj.try_borrow::<DraggedEntry>() else {
            return false;
        };
        if dragged.playlist != name {
            return false;
        }
        if dragged.from.0 != to.0 {
            a.store.move_in_playlist(
                &name,
                &dragged.from,
                &to,
                a.on_error("Could not reorder playlist"),
            );
        }
        true
    });
    view.add_controller(target);
}
