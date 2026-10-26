//! The toggleable queue side panel: now playing, user-queued tracks, then
//! the rest of the context. Drag to reorder, × to remove, double-click to play.
//!
//! The panel's rows follow the queue by diffing: the next track replaces
//! the two or three rows around "Now playing" instead of all 200 rows GTK
//! keeps, and the rows have no controllers of their own (the list view
//! handles clicks, drags and drops for all of them).

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gdk, glib};
use tunemesh_core::limits;

use crate::app::App;
use crate::covers::Source;
use crate::ui::menu::{self, MenuCtx};
use crate::ui::rows::{self, LIST_CHUNK, LIST_WINDOW, LazyRows};
use crate::ui::widgets::{icon_button, label};

/// What a row of the panel shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum QItem<'a> {
    Heading(&'a str),
    Track { index: usize, current: bool },
}

/// A run of the panel's rows: a heading, or consecutive queue entries.
#[derive(Clone)]
enum Segment {
    Heading(String),
    Tracks {
        first: usize,
        len: usize,
        current: bool,
    },
}

impl Segment {
    fn len(&self) -> usize {
        match self {
            Segment::Heading(_) => 1,
            Segment::Tracks { len, .. } => *len,
        }
    }
}

/// The rows shown: a few segments over the queue, and a hash of each queue
/// entry's path (what tells which rows an edit of the queue left alone).
#[derive(Default)]
struct Shown {
    segments: Vec<Segment>,
    hashes: Rc<Vec<u64>>,
    /// The player's queue version `hashes` are of.
    version: Option<u64>,
}

impl Shown {
    fn len(&self) -> usize {
        self.segments.iter().map(Segment::len).sum()
    }

    fn item(&self, position: usize) -> Option<QItem<'_>> {
        let mut pos = position;
        for s in &self.segments {
            if pos < s.len() {
                return Some(match s {
                    Segment::Heading(text) => QItem::Heading(text),
                    Segment::Tracks { first, current, .. } => QItem::Track {
                        index: first + pos,
                        current: *current,
                    },
                });
            }
            pos -= s.len();
        }
        None
    }

    /// What a row stands for, across queue edits.
    fn identity(&self, position: usize) -> Option<(Option<&str>, u64, bool)> {
        Some(match self.item(position)? {
            QItem::Heading(text) => (Some(text), 0, false),
            QItem::Track { index, current } => {
                (None, self.hashes.get(index).copied().unwrap_or(0), current)
            }
        })
    }
}

#[derive(Clone)]
struct QRow {
    heading: gtk::Label,
    row: gtk::Box,
    cover: gtk::Image,
    title: gtk::Label,
    artist: gtk::Label,
    remove: gtk::Image,
    li: glib::WeakRef<gtk::ListItem>,
}

pub struct QueuePanel {
    pub widget: gtk::Box,
    app: Weak<App>,
    rows: LazyRows,
    view: gtk::ListView,
    shown: RefCell<Shown>,
    /// Every row the list has made.
    widgets: RefCell<Vec<QRow>>,
}

fn qrow_of(li: &gtk::ListItem) -> QRow {
    // SAFETY: set in setup with this type.
    unsafe { li.data::<QRow>("tm-qrow").expect("qrow").as_ref().clone() }
}

impl QueuePanel {
    pub fn new(app: &Rc<App>) -> Rc<Self> {
        let widget = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["tm-panel", "tm-queue"])
            .hexpand(false)
            .build();
        let head = gtk::Box::builder().spacing(8).build();
        let title = label("Queue", &["tm-queue-heading"]);
        title.set_hexpand(true);
        head.append(&title);
        let close = icon_button("window-close-symbolic", "Close");
        let a = app.clone();
        close.connect_clicked(move |_| a.toggle_queue());
        head.append(&close);
        widget.append(&head);

        let rows = LazyRows::new(LIST_WINDOW, LIST_CHUNK);
        let factory = gtk::SignalListItemFactory::new();
        let view = gtk::ListView::builder()
            .model(&gtk::NoSelection::new(Some(rows.clone())))
            .factory(&factory)
            .css_classes(["tm-queue-list"])
            .build();
        let panel = Rc::new(Self {
            widget,
            app: Rc::downgrade(app),
            rows,
            view: view.clone(),
            shown: RefCell::new(Shown::default()),
            widgets: RefCell::new(Vec::new()),
        });

        let weak = Rc::downgrade(&panel);
        factory.connect_setup(move |_, obj| {
            let Some(p) = weak.upgrade() else { return };
            let li = obj.downcast_ref::<gtk::ListItem>().expect("list item");
            li.set_selectable(false);
            let heading = label("", &["tm-queue-section"]);
            let row = gtk::Box::builder()
                .spacing(12)
                .css_classes(["tm-queue-row"])
                .build();
            let cover = crate::ui::widgets::cover_image(40);
            let text = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .valign(gtk::Align::Center)
                .hexpand(true)
                .build();
            let title = label("", &["title"]);
            title.set_max_width_chars(1);
            let artist = label("", &["dim", "tm-small"]);
            artist.set_max_width_chars(1);
            text.append(&title);
            text.append(&artist);
            let remove = gtk::Image::builder()
                .icon_name("window-close-symbolic")
                .css_classes(["tm-remove"])
                .valign(gtk::Align::Center)
                .build();
            row.append(&cover);
            row.append(&text);
            row.append(&remove);
            li.set_child(Some(&row));
            let q = QRow {
                heading,
                row,
                cover,
                title,
                artist,
                remove,
                li: li.downgrade(),
            };
            p.rows.track(li);
            p.widgets.borrow_mut().push(q.clone());
            // SAFETY: only read back as `QRow` (`qrow_of`).
            unsafe { li.set_data("tm-qrow", q) };
        });
        let weak = Rc::downgrade(&panel);
        factory.connect_bind(move |_, obj| {
            let Some(p) = weak.upgrade() else { return };
            let li = obj.downcast_ref::<gtk::ListItem>().expect("list item");
            p.bind(li);
        });
        let weak = Rc::downgrade(&panel);
        factory.connect_teardown(move |_, obj| {
            let li = obj.downcast_ref::<gtk::ListItem>().expect("list item");
            if let Some(p) = weak.upgrade() {
                let row = qrow_of(li).row;
                p.widgets.borrow_mut().retain(|q| q.row != row);
            }
        });

        let (wa, wp) = (Rc::downgrade(app), Rc::downgrade(&panel));
        view.connect_activate(move |_, pos| {
            let (Some(a), Some(p)) = (wa.upgrade(), wp.upgrade()) else {
                return;
            };
            let index = match p.shown.borrow().item(pos as usize) {
                Some(QItem::Track { index, .. }) => Some(index),
                _ => None,
            };
            if let Some(index) = index {
                a.player.jump_to(index);
            }
        });
        panel.connect_pointer(app);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&view)
            .build();
        panel.widget.append(&scroller);
        panel
    }

    fn bind(&self, li: &gtk::ListItem) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let q = qrow_of(li);
        let shown = self.shown.borrow();
        match shown.item(li.position() as usize) {
            Some(QItem::Heading(text)) => {
                q.heading.set_label(&limits::clip_line(text));
                li.set_child(Some(&q.heading));
                li.set_activatable(false);
            }
            Some(QItem::Track { index, current }) => {
                li.set_child(Some(&q.row));
                li.set_activatable(!current);
                q.remove.set_visible(!current);
                if current {
                    q.row.add_css_class("current")
                } else {
                    q.row.remove_css_class("current")
                }
                let path = app.player.item(index).map(|i| i.path).unwrap_or_default();
                let lib = app.store.library();
                match lib.track(&path) {
                    Some(t) => {
                        q.title.set_label(&limits::clip_line(&t.title));
                        q.artist.set_label(&limits::clip_line(&t.artist));
                        app.covers.apply(&q.cover, Source::for_track(t), 40);
                    }
                    None => {
                        q.title.set_label(&limits::clip_line(
                            path.rsplit('/').next().unwrap_or(&path),
                        ));
                        q.artist.set_label("Unavailable");
                        app.covers.apply(&q.cover, Source::Placeholder(path), 40);
                    }
                }
            }
            None => {}
        }
    }

    /// The queue row under `(x, y)` of the list: its widgets, queue index
    /// and whether it is the current track.
    fn row_at(&self, x: f64, y: f64) -> Option<(QRow, usize, bool)> {
        let mut w = self.view.pick(x, y, gtk::PickFlags::DEFAULT);
        while let Some(c) = w {
            if c.has_css_class("tm-queue-row") {
                let q = self
                    .widgets
                    .borrow()
                    .iter()
                    .find(|q| *q.row.upcast_ref::<gtk::Widget>() == c)
                    .cloned()?;
                let pos = rows::position(&q.li)?;
                return match self.shown.borrow().item(pos as usize)? {
                    QItem::Track { index, current } => Some((q, index, current)),
                    QItem::Heading(_) => None,
                };
            }
            w = c.parent();
        }
        None
    }

    /// The × button, right-click menus and drag and drop, for every row.
    fn connect_pointer(self: &Rc<Self>, app: &Rc<App>) {
        let click = gtk::GestureClick::builder()
            .button(0)
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        let (wa, wp) = (Rc::downgrade(app), Rc::downgrade(self));
        click.connect_pressed(move |g, _, x, y| {
            let (Some(a), Some(p)) = (wa.upgrade(), wp.upgrade()) else {
                return;
            };
            let Some((q, i, current)) = p.row_at(x, y) else {
                g.set_state(gtk::EventSequenceState::Denied);
                return;
            };
            match g.current_button() {
                gdk::BUTTON_PRIMARY => {
                    let on_remove = p.view.pick(x, y, gtk::PickFlags::DEFAULT).as_ref()
                        == Some(q.remove.upcast_ref());
                    if on_remove && !current {
                        g.set_state(gtk::EventSequenceState::Claimed);
                        a.player.remove_at(i);
                    } else {
                        g.set_state(gtk::EventSequenceState::Denied);
                    }
                }
                gdk::BUTTON_SECONDARY => {
                    g.set_state(gtk::EventSequenceState::Claimed);
                    let Some(item) = a.player.item(i) else {
                        return;
                    };
                    let paths = if a.store.library().track_index(&item.path).is_some() {
                        vec![item.path]
                    } else {
                        vec![]
                    };
                    let ctx = MenuCtx {
                        paths,
                        playlist: None,
                        queue_index: (!current).then_some(i),
                    };
                    let at = p
                        .view
                        .compute_point(&q.row, &gtk::graphene::Point::new(x as f32, y as f32))
                        .unwrap_or_else(|| gtk::graphene::Point::new(x as f32, y as f32));
                    menu::popup(&a, q.row.upcast_ref(), at.x() as f64, at.y() as f64, ctx);
                }
                _ => {
                    g.set_state(gtk::EventSequenceState::Denied);
                }
            }
        });
        self.view.add_controller(click);

        self.view.set_has_tooltip(true);
        let wp = Rc::downgrade(self);
        self.view
            .connect_query_tooltip(move |_, x, y, keyboard, tooltip| {
                let Some(p) = wp.upgrade().filter(|_| !keyboard) else {
                    return false;
                };
                let (x, y) = (x as f64, y as f64);
                let Some((q, _, false)) = p.row_at(x, y) else {
                    return false;
                };
                if p.view.pick(x, y, gtk::PickFlags::DEFAULT).as_ref()
                    != Some(q.remove.upcast_ref())
                {
                    return false;
                }
                tooltip.set_text(Some("Remove from queue"));
                true
            });

        let source = gtk::DragSource::builder()
            .actions(gdk::DragAction::MOVE)
            .build();
        let dragged: Rc<RefCell<Option<gtk::Box>>> = Rc::default();
        let (wp, d) = (Rc::downgrade(self), dragged.clone());
        source.connect_prepare(move |_, x, y| {
            let (q, i, current) = wp.upgrade()?.row_at(x, y)?;
            if current {
                return None;
            }
            *d.borrow_mut() = Some(q.row.clone());
            Some(gdk::ContentProvider::for_value(
                &format!("tm-q:{i}").to_value(),
            ))
        });
        source.connect_drag_begin(move |s, _| {
            if let Some(row) = dragged.borrow_mut().take() {
                s.set_icon(Some(&gtk::WidgetPaintable::new(Some(&row))), 0, 0);
            }
        });
        self.view.add_controller(source);

        let target = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
        let marked: Rc<RefCell<Option<glib::WeakRef<gtk::Box>>>> = Rc::default();
        let unmark = {
            let marked = marked.clone();
            move || {
                if let Some(r) = marked.borrow_mut().take().and_then(|w| w.upgrade()) {
                    r.remove_css_class("tm-drop-before");
                }
            }
        };
        let (wp, m, u) = (Rc::downgrade(self), marked.clone(), unmark.clone());
        target.connect_motion(move |_, x, y| {
            let row = wp
                .upgrade()
                .and_then(|p| p.row_at(x, y))
                .map(|(q, _, _)| q.row);
            let current = m.borrow().as_ref().and_then(|w| w.upgrade());
            if row != current {
                u();
                if let Some(r) = row {
                    r.add_css_class("tm-drop-before");
                    *m.borrow_mut() = Some(r.downgrade());
                }
            }
            gdk::DragAction::MOVE
        });
        let u = unmark.clone();
        target.connect_leave(move |_| u());
        let (wa, wp) = (Rc::downgrade(app), Rc::downgrade(self));
        target.connect_drop(move |_, value, x, y| {
            unmark();
            let (Some(a), Some(p)) = (wa.upgrade(), wp.upgrade()) else {
                return false;
            };
            let Some((_, to, _)) = p.row_at(x, y) else {
                return false;
            };
            let Ok(s) = value.get::<String>() else {
                return false;
            };
            let Some(from) = s
                .strip_prefix("tm-q:")
                .and_then(|n| n.parse::<usize>().ok())
            else {
                return false;
            };
            a.player.move_item(from, to);
            true
        });
        self.view.add_controller(target);
    }

    /// Brings the rows up to date with the player's queue, replacing only
    /// the rows that changed.
    pub fn rebuild(&self) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        if !app.queue_shown() {
            return;
        }
        let (len, current, user) = app.player.queue_shape();
        let mut segments = Vec::new();
        match current {
            Some(c) if c < len => {
                segments.push(Segment::Heading("Now playing".into()));
                segments.push(Segment::Tracks {
                    first: c,
                    len: 1,
                    current: true,
                });
                if user > 0 {
                    segments.push(Segment::Heading("Next in queue".into()));
                    segments.push(Segment::Tracks {
                        first: c + 1,
                        len: user,
                        current: false,
                    });
                }
                let rest = len - (c + 1) - user;
                if rest > 0 {
                    let ctx = app.player.context_name();
                    segments.push(Segment::Heading(if ctx.is_empty() {
                        "Next up".into()
                    } else {
                        format!("Next from: {ctx}")
                    }));
                    segments.push(Segment::Tracks {
                        first: c + 1 + user,
                        len: rest,
                        current: false,
                    });
                }
            }
            _ => segments.push(Segment::Heading("Nothing is playing".into())),
        }
        let version = app.player.queue_version();
        let mut shown = self.shown.borrow_mut();
        let hashes = if shown.version == Some(version) {
            shown.hashes.clone()
        } else {
            Rc::new(app.player.queue_hashes())
        };
        let new = Shown {
            segments,
            hashes,
            version: Some(version),
        };
        let (p, removed, added) = rows::diff_by(shown.len(), new.len(), |i, j| {
            shown.identity(i) == new.identity(j)
        });
        let first = self.rows.len() == 0;
        *shown = new;
        let total = shown.len() as u32;
        drop(shown);
        if first {
            self.rows.reset(total, &self.view);
        } else {
            self.rows.splice(p as u32, removed as u32, added as u32);
        }
    }
}
