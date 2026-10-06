//! The virtualised track list used by album, artist, playlist and search
//! pages: double-click plays, right-click opens the track menu, playlists
//! reorder by drag and drop. A page header can ride along as row 0.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::app::App;
use crate::covers::Source;
use crate::ui::menu::{self, MenuCtx};
use crate::ui::widgets::{fmt_duration, label};

pub struct Entry {
    /// Library path; `None` for playlist lines pointing outside the library.
    pub path: Option<String>,
    /// Display text when the track is unavailable.
    pub label: String,
}

pub struct Spec {
    /// Context name for "playing from".
    pub context: String,
    pub entries: Vec<Entry>,
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
    entries: Vec<Entry>,
    rows: RefCell<Vec<RowW>>,
}

impl Shared {
    fn available(&self, app: &App, i: usize) -> Option<String> {
        let path = self.entries[i].path.as_ref()?;
        app.store.library().track_index(path).map(|_| path.clone())
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
        let entry = &self.entries[i];
        let track = entry.path.as_deref().and_then(|p| lib.track(p));
        r.index.set_label(&(i + 1).to_string());
        match track {
            Some(t) => {
                r.title.set_label(&t.title);
                r.artist.set_label(&t.artist);
                if let Some(a) = &r.album {
                    a.set_label(&t.album);
                }
                r.duration.set_label(&t.duration_secs().map(fmt_duration).unwrap_or_default());
                if let Some(c) = &r.cover {
                    app.covers.apply(c, Source::for_track(t), 40);
                }
                r.root.remove_css_class("unavailable");
                r.heart.set_visible(true);
                let liked = app.store.is_liked(&t.path);
                r.heart.set_icon_name(if liked { "tm-heart-filled-symbolic" } else { "tm-heart-outline-symbolic" });
                r.heart.set_tooltip_text(Some(if liked { "Remove from Liked Songs" } else { "Save to Liked Songs" }));
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
                r.title.set_label(&entry.label);
                r.artist.set_label("Unavailable on this device");
                if let Some(a) = &r.album {
                    a.set_label("");
                }
                r.duration.set_label("");
                if let Some(c) = &r.cover {
                    app.covers.apply(c, Source::Placeholder(entry.label.clone()), 40);
                }
                r.root.add_css_class("unavailable");
                r.root.remove_css_class("current");
                r.heart.set_visible(false);
            }
        }
    }
}

fn new_row(spec_album: bool, spec_cover: bool) -> RowW {
    let root = gtk::Box::builder().spacing(16).css_classes(["tm-track-row"]).build();
    let index = gtk::Label::builder().width_chars(3).xalign(1.0).css_classes(["index"]).build();
    root.append(&index);
    let cover = spec_cover.then(|| {
        let p = crate::ui::widgets::cover_image(40);
        root.append(&p);
        p
    });
    // Title and album share the free width equally, in rows and the header.
    let cols = gtk::Box::builder().spacing(16).homogeneous(true).hexpand(true).build();
    let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).valign(gtk::Align::Center).build();
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
    let duration = gtk::Label::builder().width_request(54).xalign(1.0).css_classes(["duration"]).build();
    root.append(&duration);
    RowW { root, index, cover, title, artist, album, duration, heart, pos: Rc::new(Cell::new(None)) }
}

/// The column captions shown above the rows.
pub fn columns_header(show_album: bool, show_cover: bool) -> gtk::Box {
    let b = gtk::Box::builder().spacing(16).css_classes(["tm-tracks-head"]).build();
    b.append(&gtk::Label::builder().label("#").width_chars(3).xalign(1.0).build());
    if show_cover {
        b.append(&gtk::Box::builder().width_request(40).build());
    }
    let cols = gtk::Box::builder().spacing(16).homogeneous(true).hexpand(true).build();
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
    let shared = Rc::new(Shared { entries: spec.entries, rows: RefCell::new(Vec::new()) });
    app.register_track_list(&shared);

    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let has_header = spec.header.is_some();
    if has_header {
        store.append(&glib::BoxedAnyObject::new(Item::Header));
    }
    let items: Vec<glib::BoxedAnyObject> =
        (0..shared.entries.len()).map(|i| glib::BoxedAnyObject::new(Item::Track(i))).collect();
    store.extend_from_slice(&items);
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

            let (a, pos) = (app.clone(), row.pos.clone());
            let sh = shared.clone();
            row.heart.connect_clicked(move |_| {
                if let Some(path) = pos.get().and_then(|i| sh.available(&a, i))
                    && let Err(e) = a.store.toggle_like(&path) {
                        a.error("Could not update Liked Songs", e);
                    }
            });

            let click = gtk::GestureClick::builder().button(gdk::BUTTON_SECONDARY).build();
            let (a, pos, sh, sel, pl, root) =
                (app.clone(), row.pos.clone(), shared.clone(), selection.clone(), playlist.clone(), row.root.clone());
            click.connect_pressed(move |g, _, x, y| {
                g.set_state(gtk::EventSequenceState::Claimed);
                let Some(i) = pos.get() else { return };
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
                let paths: Vec<String> = targets.iter().filter_map(|&t| sh.available(&a, t)).collect();
                let ctx = MenuCtx {
                    paths,
                    playlist: pl.clone().map(|name| (name, targets.clone())),
                    queue_index: None,
                };
                menu::popup(&a, root.upcast_ref(), x, y, ctx);
            });
            row.root.add_controller(click);

            if let Some(name) = playlist.clone() {
                let source = gtk::DragSource::builder().actions(gdk::DragAction::MOVE).build();
                let pos = row.pos.clone();
                source.connect_prepare(move |_, _, _| {
                    let i = pos.get()?;
                    Some(gdk::ContentProvider::for_value(&format!("tm-pos:{i}").to_value()))
                });
                row.root.add_controller(source);
                let target = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
                let (a, pos, root) = (app.clone(), row.pos.clone(), row.root.clone());
                target.connect_drop(move |_, value, _, _| {
                    root.remove_css_class("tm-drop-before");
                    let Some(to) = pos.get() else { return false };
                    let Ok(s) = value.get::<String>() else { return false };
                    let Some(from) = s.strip_prefix("tm-pos:").and_then(|n| n.parse::<usize>().ok()) else {
                        return false;
                    };
                    if from != to
                        && let Err(e) = a.store.move_in_playlist(&name, from, to) {
                            a.error("Could not reorder playlist", e);
                        }
                    true
                });
                let root = row.root.clone();
                target.connect_enter(move |_, _, _| {
                    root.add_css_class("tm-drop-before");
                    gdk::DragAction::MOVE
                });
                let root = row.root.clone();
                target.connect_leave(move |_| root.remove_css_class("tm-drop-before"));
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
            let item = li.item().and_downcast::<glib::BoxedAnyObject>().expect("item");
            let item = item.borrow::<Item>();
            match *item {
                Item::Header => {
                    row.pos.set(None);
                    row.root.set_visible(false);
                    if let Some(h) = &header
                        && h.parent().as_ref() != Some(slot.upcast_ref()) {
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
                && h.parent().as_ref() == Some(slot.upcast_ref()) {
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
