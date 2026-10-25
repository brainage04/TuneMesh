//! The toggleable queue side panel: now playing, user-queued tracks, then
//! the rest of the context. Drag to reorder, × to remove, double-click to play.

use std::cell::Cell;
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gdk, glib};
use tunemesh_core::limits;

use crate::app::App;
use crate::covers::Source;
use crate::player::Change;
use crate::ui::menu::{self, MenuCtx};
use crate::ui::widgets::{icon_button, label};

enum QItem {
    Heading(String),
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

/// The panel's list model: a few segments, with row objects made when GTK
/// asks for them (the rows on screen). A 40,000-entry queue costs nothing
/// to show again after every track change.
mod rows {
    use gtk::subclass::prelude::*;
    use gtk::{gio, glib};

    mod imp {
        use std::cell::{Cell, RefCell};

        use gtk::subclass::prelude::*;
        use gtk::{gio, glib, prelude::*};

        use super::super::{QItem, Segment};

        #[derive(Default)]
        pub struct Rows {
            pub(in super::super) segments: RefCell<Vec<Segment>>,
            pub len: Cell<u32>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for Rows {
            const NAME: &'static str = "TmQueueRows";
            type Type = super::Rows;
            type Interfaces = (gio::ListModel,);
        }

        impl ObjectImpl for Rows {}

        impl ListModelImpl for Rows {
            fn item_type(&self) -> glib::Type {
                glib::BoxedAnyObject::static_type()
            }

            fn n_items(&self) -> u32 {
                self.len.get()
            }

            fn item(&self, position: u32) -> Option<glib::Object> {
                let mut pos = position as usize;
                for s in self.segments.borrow().iter() {
                    if pos < s.len() {
                        let item = match s {
                            Segment::Heading(text) => QItem::Heading(text.clone()),
                            Segment::Tracks { first, current, .. } => QItem::Track {
                                index: first + pos,
                                current: *current,
                            },
                        };
                        return Some(glib::BoxedAnyObject::new(item).upcast());
                    }
                    pos -= s.len();
                }
                None
            }
        }
    }

    glib::wrapper! {
        pub struct Rows(ObjectSubclass<imp::Rows>) @implements gio::ListModel;
    }

    impl Rows {
        pub fn new() -> Self {
            glib::Object::new()
        }

        /// Replaces every row.
        pub(super) fn set(&self, segments: Vec<super::Segment>) {
            let imp = self.imp();
            let old = imp.len.get();
            let len = segments.iter().map(super::Segment::len).sum::<usize>();
            imp.len.set(u32::try_from(len).unwrap_or(u32::MAX));
            *imp.segments.borrow_mut() = segments;
            gio::prelude::ListModelExt::items_changed(self, 0, old, imp.len.get());
        }

        /// The queue index shown at `position` (none for headings).
        pub(super) fn index_at(&self, position: u32) -> Option<usize> {
            let mut pos = position as usize;
            for s in self.imp().segments.borrow().iter() {
                if pos < s.len() {
                    return match s {
                        super::Segment::Heading(_) => None,
                        super::Segment::Tracks { first, .. } => Some(first + pos),
                    };
                }
                pos -= s.len();
            }
            None
        }
    }
}

#[derive(Clone)]
struct QRow {
    heading: gtk::Label,
    row: gtk::Box,
    cover: gtk::Image,
    title: gtk::Label,
    artist: gtk::Label,
    remove: gtk::Button,
    index: Rc<Cell<Option<usize>>>,
    current: Rc<Cell<bool>>,
}

pub struct QueuePanel {
    pub widget: gtk::Box,
    app: Weak<App>,
    rows: rows::Rows,
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

        let factory = gtk::SignalListItemFactory::new();
        let weak_app = Rc::downgrade(app);
        factory.connect_setup(move |_, obj| {
            let li = obj.downcast_ref::<gtk::ListItem>().expect("list item");
            let slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
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
            let remove = icon_button("window-close-symbolic", "Remove from queue");
            remove.add_css_class("tm-remove");
            remove.set_valign(gtk::Align::Center);
            row.append(&cover);
            row.append(&text);
            row.append(&remove);
            slot.append(&heading);
            slot.append(&row);
            li.set_child(Some(&slot));
            let q = QRow {
                heading,
                row: row.clone(),
                cover,
                title,
                artist,
                remove: remove.clone(),
                index: Rc::new(Cell::new(None)),
                current: Rc::new(Cell::new(false)),
            };

            let (wa, idx) = (weak_app.clone(), q.index.clone());
            remove.connect_clicked(move |_| {
                if let (Some(a), Some(i)) = (wa.upgrade(), idx.get()) {
                    a.player.remove_at(i);
                }
            });
            let click = gtk::GestureClick::builder()
                .button(gdk::BUTTON_SECONDARY)
                .build();
            let (wa, idx, cur, r) = (
                weak_app.clone(),
                q.index.clone(),
                q.current.clone(),
                row.downgrade(),
            );
            click.connect_pressed(move |g, _, x, y| {
                g.set_state(gtk::EventSequenceState::Claimed);
                let (Some(a), Some(i)) = (wa.upgrade(), idx.get()) else {
                    return;
                };
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
                    queue_index: (!cur.get()).then_some(i),
                };
                if let Some(r) = r.upgrade() {
                    menu::popup(&a, r.upcast_ref(), x, y, ctx);
                }
            });
            row.add_controller(click);

            let source = gtk::DragSource::builder()
                .actions(gdk::DragAction::MOVE)
                .build();
            let (idx, cur) = (q.index.clone(), q.current.clone());
            source.connect_prepare(move |_, _, _| {
                let i = idx.get().filter(|_| !cur.get())?;
                Some(gdk::ContentProvider::for_value(
                    &format!("tm-q:{i}").to_value(),
                ))
            });
            row.add_controller(source);
            let target = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
            // A row's own controllers reach it through the controller, never
            // by a captured reference that would keep the row alive.
            let (wa, idx) = (weak_app.clone(), q.index.clone());
            target.connect_drop(move |t, value, _, _| {
                if let Some(r) = t.widget() {
                    r.remove_css_class("tm-drop-before");
                }
                let (Some(a), Some(to)) = (wa.upgrade(), idx.get()) else {
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
            target.connect_enter(|t, _, _| {
                if let Some(r) = t.widget() {
                    r.add_css_class("tm-drop-before");
                }
                gdk::DragAction::MOVE
            });
            target.connect_leave(|t| {
                if let Some(r) = t.widget() {
                    r.remove_css_class("tm-drop-before");
                }
            });
            row.add_controller(target);

            // SAFETY: only read back as `QRow`.
            unsafe { slot.set_data("tm-qrow", q) };
        });
        let weak_app = Rc::downgrade(app);
        factory.connect_bind(move |_, obj| {
            let li = obj.downcast_ref::<gtk::ListItem>().expect("list item");
            let slot = li.child().expect("slot");
            // SAFETY: set in setup with this type.
            let q = unsafe { slot.data::<QRow>("tm-qrow").expect("qrow").as_ref().clone() };
            let item = li
                .item()
                .and_downcast::<glib::BoxedAnyObject>()
                .expect("item");
            let Some(app) = weak_app.upgrade() else {
                return;
            };
            match &*item.borrow::<QItem>() {
                QItem::Heading(text) => {
                    q.heading.set_label(&limits::clip_line(text));
                    q.heading.set_visible(true);
                    q.row.set_visible(false);
                    q.index.set(None);
                    li.set_activatable(false);
                    li.set_selectable(false);
                }
                QItem::Track { index, current } => {
                    q.heading.set_visible(false);
                    q.row.set_visible(true);
                    q.index.set(Some(*index));
                    q.current.set(*current);
                    li.set_activatable(!*current);
                    li.set_selectable(false);
                    q.remove.set_visible(!*current);
                    if *current {
                        q.row.add_css_class("current")
                    } else {
                        q.row.remove_css_class("current")
                    }
                    let path = app.player.item(*index).map(|i| i.path).unwrap_or_default();
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
            }
        });
        let rows = rows::Rows::new();
        let list = gtk::ListView::builder()
            .model(&gtk::NoSelection::new(Some(rows.clone())))
            .factory(&factory)
            .css_classes(["tm-queue-list"])
            .build();
        let weak_app = Rc::downgrade(app);
        let panel = Rc::new(Self {
            widget,
            app: Rc::downgrade(app),
            rows,
        });
        let weak_panel = Rc::downgrade(&panel);
        list.connect_activate(move |_, pos| {
            let (Some(a), Some(p)) = (weak_app.upgrade(), weak_panel.upgrade()) else {
                return;
            };
            if let Some(i) = p.rows.index_at(pos) {
                a.player.jump_to(i);
            }
        });
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list)
            .build();
        panel.widget.append(&scroller);

        let weak = Rc::downgrade(&panel);
        app.player.connect(move |c| {
            if matches!(c, Change::Queue | Change::Track)
                && let Some(p) = weak.upgrade()
            {
                p.rebuild();
            }
        });
        panel
    }

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
        self.rows.set(segments);
    }
}
