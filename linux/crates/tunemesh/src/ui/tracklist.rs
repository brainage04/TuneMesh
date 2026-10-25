//! The virtualised track list used by album, artist, playlist and search
//! pages: double-click plays, right-click opens the track menu, playlists
//! reorder by drag and drop. A page header can ride along as row 0.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

use tunemesh_core::{Playlist, limits};

use crate::app::App;
use crate::covers::Source;
use crate::store::EntryRef;
use crate::ui::menu::{self, MenuCtx};
use crate::ui::widgets::{fmt_duration, label};

/// What a track list shows. A playlist is held once, as read: a long one
/// costs no second copy, and rows are made only for what is on screen.
pub enum Entries {
    /// Library paths (albums, artists, search results).
    Tracks(Vec<String>),
    Playlist(Playlist),
}

impl Entries {
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn len(&self) -> usize {
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

enum Item {
    Header,
    Track(usize),
}

/// The list model: an optional header row, then one row per entry. Row
/// objects are made when GTK asks for them (the rows on screen), not one
/// per entry up front.
mod rows {
    use gtk::subclass::prelude::*;
    use gtk::{gio, glib};

    mod imp {
        use std::cell::Cell;

        use gtk::subclass::prelude::*;
        use gtk::{gio, glib, prelude::*};

        use super::super::Item;

        #[derive(Default)]
        pub struct Rows {
            pub header: Cell<bool>,
            pub len: Cell<u32>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for Rows {
            const NAME: &'static str = "TmTrackRows";
            type Type = super::Rows;
            type Interfaces = (gio::ListModel,);
        }

        impl ObjectImpl for Rows {}

        impl ListModelImpl for Rows {
            fn item_type(&self) -> glib::Type {
                glib::BoxedAnyObject::static_type()
            }

            fn n_items(&self) -> u32 {
                self.len.get() + u32::from(self.header.get())
            }

            fn item(&self, position: u32) -> Option<glib::Object> {
                let header = u32::from(self.header.get());
                let item = match position {
                    0 if header == 1 => Item::Header,
                    p if p < self.n_items() => Item::Track((p - header) as usize),
                    _ => return None,
                };
                Some(glib::BoxedAnyObject::new(item).upcast())
            }
        }
    }

    glib::wrapper! {
        pub struct Rows(ObjectSubclass<imp::Rows>) @implements gio::ListModel;
    }

    impl Rows {
        pub fn new(header: bool, len: usize) -> Self {
            let rows: Self = glib::Object::new();
            rows.imp().header.set(header);
            rows.imp().len.set(u32::try_from(len).unwrap_or(u32::MAX));
            rows
        }
    }
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
    heart: gtk::Button,
    pos: Rc<Cell<Option<usize>>>,
}

pub struct Shared {
    entries: Entries,
    rows: RefCell<Vec<RowW>>,
}

impl Shared {
    fn entry_ref(&self, i: usize) -> Option<EntryRef> {
        Some((i, self.entries.key(i)?.to_owned()))
    }

    fn available(&self, app: &App, i: usize) -> Option<String> {
        let path = self.entries.path(i)?;
        app.store
            .library()
            .track_index(path)
            .map(|_| path.to_owned())
    }

    pub fn refresh_rows(&self, app: &App) {
        for r in self.rows.borrow().iter() {
            if let Some(i) = r.pos.get() {
                self.update_row(app, r, i);
            }
        }
    }

    fn update_row(&self, app: &App, r: &RowW, i: usize) {
        let lib = app.store.library();
        let track = self.entries.path(i).and_then(|p| lib.track(p));
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
                r.heart.set_icon_name(if liked {
                    "tm-heart-filled-symbolic"
                } else {
                    "tm-heart-outline-symbolic"
                });
                r.heart.set_tooltip_text(Some(if liked {
                    "Remove from Liked Songs"
                } else {
                    "Save to Liked Songs"
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
                let label = self.entries.label(i);
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
    let heart = gtk::Button::builder()
        .icon_name("tm-heart-outline-symbolic")
        .css_classes(["tm-icon-btn", "tm-heart"])
        .valign(gtk::Align::Center)
        .build();
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
        pos: Rc::new(Cell::new(None)),
    }
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
    // Same size as the rows' heart button.
    let spacer = gtk::Button::builder()
        .icon_name("tm-heart-outline-symbolic")
        .css_classes(["tm-icon-btn"])
        .opacity(0.0)
        .can_target(false)
        .can_focus(false)
        .build();
    b.append(&spacer);
    let clock = gtk::Image::from_icon_name("document-open-recent-symbolic");
    let clock_box = gtk::Box::builder().width_request(54).hexpand(false).build();
    clock.set_hexpand(true);
    clock.set_halign(gtk::Align::End);
    clock_box.append(&clock);
    b.append(&clock_box);
    b
}

/// Builds the list. Put it directly into a `ScrolledWindow` for long lists.
pub fn build(app: &Rc<App>, spec: Spec) -> gtk::ListView {
    let shared = Rc::new(Shared {
        entries: spec.entries,
        rows: RefCell::new(Vec::new()),
    });
    app.register_track_list(&shared);

    let has_header = spec.header.is_some();
    let store = rows::Rows::new(has_header, shared.entries.len());
    let offset = u32::from(has_header);
    let selection = gtk::MultiSelection::new(Some(store));

    let header: Option<gtk::Widget> = spec.header.map(|h| {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
        b.add_css_class("tm-header-block");
        b.append(&h);
        b.upcast()
    });
    let factory = gtk::SignalListItemFactory::new();
    let (show_album, show_cover) = (spec.show_album, spec.show_cover);
    let playlist = spec.playlist.clone();
    {
        let shared = shared.clone();
        let app = app.clone();
        let selection = selection.clone();
        let playlist = playlist.clone();
        factory.connect_setup(move |_, obj| {
            let li = obj.downcast_ref::<gtk::ListItem>().expect("list item");
            let slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
            let row = new_row(show_album, show_cover);
            slot.append(&row.root);
            li.set_child(Some(&slot));

            // Closures on a row's widgets hold the shared list weakly: it
            // holds the rows, so a strong reference would keep the whole
            // list alive after its page is gone.
            let (a, pos) = (app.clone(), row.pos.clone());
            let sh = Rc::downgrade(&shared);
            row.heart.connect_clicked(move |_| {
                if let Some(sh) = sh.upgrade()
                    && let Some(path) = pos.get().and_then(|i| sh.available(&a, i))
                {
                    a.store
                        .toggle_like(&path, a.on_error("Could not update Liked Songs"));
                }
            });

            let click = gtk::GestureClick::builder()
                .button(gdk::BUTTON_SECONDARY)
                .build();
            let (a, pos, sh, sel, pl, root) = (
                app.clone(),
                row.pos.clone(),
                Rc::downgrade(&shared),
                selection.clone(),
                playlist.clone(),
                row.root.downgrade(),
            );
            click.connect_pressed(move |g, _, x, y| {
                g.set_state(gtk::EventSequenceState::Claimed);
                let (Some(i), Some(sh), Some(root)) = (pos.get(), sh.upgrade(), root.upgrade())
                else {
                    return;
                };
                let model_pos = i as u32 + offset;
                let mut targets: Vec<usize> = Vec::new();
                if sel.is_selected(model_pos) && sel.selection().size() > 1 {
                    let bits = sel.selection();
                    for k in 0..bits.size() {
                        let m = bits.nth(k as u32);
                        if m >= offset {
                            targets.push((m - offset) as usize);
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
                    playlist: pl.clone().map(|name| {
                        (
                            name,
                            targets.iter().filter_map(|&t| sh.entry_ref(t)).collect(),
                        )
                    }),
                    queue_index: None,
                };
                menu::popup(&a, root.upcast_ref(), x, y, ctx);
            });
            row.root.add_controller(click);

            if let Some(name) = playlist.clone() {
                let source = gtk::DragSource::builder()
                    .actions(gdk::DragAction::MOVE)
                    .build();
                let (pos, sh, n) = (row.pos.clone(), Rc::downgrade(&shared), name.clone());
                source.connect_prepare(move |_, _, _| {
                    let from = sh.upgrade()?.entry_ref(pos.get()?)?;
                    let payload = glib::BoxedAnyObject::new(DraggedEntry {
                        playlist: n.clone(),
                        from,
                    });
                    Some(gdk::ContentProvider::for_value(&payload.to_value()))
                });
                row.root.add_controller(source);
                let target = gtk::DropTarget::new(
                    glib::BoxedAnyObject::static_type(),
                    gdk::DragAction::MOVE,
                );
                let (a, pos, sh) = (app.clone(), row.pos.clone(), Rc::downgrade(&shared));
                target.connect_drop(move |t, value, _, _| {
                    if let Some(root) = t.widget() {
                        root.remove_css_class("tm-drop-before");
                    }
                    let Some(to) = sh
                        .upgrade()
                        .and_then(|sh| pos.get().and_then(|i| sh.entry_ref(i)))
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
                target.connect_enter(|t, _, _| {
                    if let Some(root) = t.widget() {
                        root.add_css_class("tm-drop-before");
                    }
                    gdk::DragAction::MOVE
                });
                target.connect_leave(|t| {
                    if let Some(root) = t.widget() {
                        root.remove_css_class("tm-drop-before");
                    }
                });
                row.root.add_controller(target);
            }

            // SAFETY: only ever read back as `RowW`.
            unsafe { slot.set_data("tm-row", row.clone()) };
            shared.rows.borrow_mut().push(row);
        });
    }
    {
        let shared = shared.clone();
        let app = app.clone();
        let header = header.clone();
        factory.connect_bind(move |_, obj| {
            let li = obj.downcast_ref::<gtk::ListItem>().expect("list item");
            let slot = li.child().and_downcast::<gtk::Box>().expect("slot");
            // SAFETY: set in setup with this type.
            let row = unsafe { slot.data::<RowW>("tm-row").expect("row").as_ref().clone() };
            let item = li
                .item()
                .and_downcast::<glib::BoxedAnyObject>()
                .expect("item");
            let item = item.borrow::<Item>();
            match *item {
                Item::Header => {
                    row.pos.set(None);
                    row.root.set_visible(false);
                    if let Some(h) = &header
                        && h.parent().as_ref() != Some(slot.upcast_ref())
                    {
                        h.unparent();
                        slot.append(h);
                    }
                    li.set_activatable(false);
                    li.set_selectable(false);
                    li.set_focusable(false);
                }
                Item::Track(i) => {
                    row.root.set_visible(true);
                    row.pos.set(Some(i));
                    li.set_activatable(true);
                    li.set_selectable(true);
                    li.set_focusable(true);
                    shared.update_row(&app, &row, i);
                }
            }
        });
    }
    {
        let header = header.clone();
        factory.connect_unbind(move |_, obj| {
            let li = obj.downcast_ref::<gtk::ListItem>().expect("list item");
            let slot = li.child().and_downcast::<gtk::Box>().expect("slot");
            // SAFETY: set in setup with this type.
            let row = unsafe { slot.data::<RowW>("tm-row").expect("row").as_ref().clone() };
            row.pos.set(None);
            if let Some(h) = &header
                && h.parent().as_ref() == Some(slot.upcast_ref())
            {
                slot.remove(h);
            }
        });
    }
    {
        let shared = shared.clone();
        factory.connect_teardown(move |_, obj| {
            let li = obj.downcast_ref::<gtk::ListItem>().expect("list item");
            if let Some(slot) = li.child().and_downcast::<gtk::Box>() {
                // SAFETY: set in setup with this type.
                if let Some(row) = unsafe { slot.data::<RowW>("tm-row") } {
                    let root = unsafe { row.as_ref() }.root.clone();
                    shared.rows.borrow_mut().retain(|r| r.root != root);
                }
            }
        });
    }

    let view = gtk::ListView::builder()
        .model(&selection)
        .factory(&factory)
        .single_click_activate(false)
        .css_classes(["tm-tracks"])
        .build();
    let (a, sh, context) = (app.clone(), shared.clone(), spec.context);
    view.connect_activate(move |_, model_pos| {
        if model_pos < offset {
            return;
        }
        let i = (model_pos - offset) as usize;
        let mut tracks = Vec::new();
        let mut start = 0;
        for k in 0..sh.entries.len() {
            if let Some(p) = sh.available(&a, k) {
                if k == i {
                    start = tracks.len();
                }
                tracks.push(p);
            }
        }
        if sh.available(&a, i).is_some() {
            a.player.play_tracks(&context, tracks, start, None);
        }
    });
    view
}
