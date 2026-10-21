//! The toggleable queue side panel: now playing, user-queued tracks, then
//! the rest of the context. Drag to reorder, × to remove, double-click to play.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use tunemesh_core::limits;

use crate::app::App;
use crate::covers::Source;
use crate::player::Change;
use crate::ui::menu::{self, MenuCtx};
use crate::ui::widgets::{icon_button, label};

enum QItem {
    Heading(String),
    Track {
        index: usize,
        path: String,
        current: bool,
    },
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
    store: gio::ListStore,
    /// Queue index per model position (None for headings).
    indices: RefCell<Vec<Option<usize>>>,
}

impl QueuePanel {
    pub fn new(app: &Rc<App>) -> Rc<Self> {
        let widget = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["tm-panel", "tm-queue"])
            .width_request(340)
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

        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
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
                let Some(item) = a.player.items().get(i).cloned() else {
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
                QItem::Track {
                    index,
                    path,
                    current,
                } => {
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
                    let lib = app.store.library();
                    match lib.track(path) {
                        Some(t) => {
                            q.title.set_label(&limits::clip_line(&t.title));
                            q.artist.set_label(&limits::clip_line(&t.artist));
                            app.covers.apply(&q.cover, Source::for_track(t), 40);
                        }
                        None => {
                            q.title.set_label(&limits::clip_line(
                                path.rsplit('/').next().unwrap_or(path),
                            ));
                            q.artist.set_label("Unavailable");
                            app.covers
                                .apply(&q.cover, Source::Placeholder(path.clone()), 40);
                        }
                    }
                }
            }
        });
        let list = gtk::ListView::builder()
            .model(&gtk::NoSelection::new(Some(store.clone())))
            .factory(&factory)
            .css_classes(["tm-queue-list"])
            .build();
        let weak_app = Rc::downgrade(app);
        let panel = Rc::new(Self {
            widget,
            app: Rc::downgrade(app),
            store,
            indices: RefCell::new(Vec::new()),
        });
        let weak_panel = Rc::downgrade(&panel);
        list.connect_activate(move |_, pos| {
            let (Some(a), Some(p)) = (weak_app.upgrade(), weak_panel.upgrade()) else {
                return;
            };
            let index = p.indices.borrow().get(pos as usize).copied().flatten();
            if let Some(i) = index {
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
        let items = app.player.items();
        let current = app.player.current_index();
        let mut model: Vec<QItem> = Vec::new();
        let mut indices: Vec<Option<usize>> = Vec::new();
        let mut push = |item: QItem| {
            indices.push(match &item {
                QItem::Track { index, .. } => Some(*index),
                QItem::Heading(_) => None,
            });
            model.push(item);
        };
        match current {
            Some(c) if c < items.len() => {
                push(QItem::Heading("Now playing".into()));
                push(QItem::Track {
                    index: c,
                    path: items[c].path.clone(),
                    current: true,
                });
                let upcoming: Vec<usize> = (c + 1..items.len()).collect();
                let user: Vec<usize> = upcoming
                    .iter()
                    .copied()
                    .take_while(|&i| items[i].user)
                    .collect();
                if !user.is_empty() {
                    push(QItem::Heading("Next in queue".into()));
                    for &i in &user {
                        push(QItem::Track {
                            index: i,
                            path: items[i].path.clone(),
                            current: false,
                        });
                    }
                }
                let rest: Vec<usize> = upcoming.into_iter().skip(user.len()).collect();
                if !rest.is_empty() {
                    let ctx = app.player.context_name();
                    push(QItem::Heading(if ctx.is_empty() {
                        "Next up".into()
                    } else {
                        format!("Next from: {ctx}")
                    }));
                    for i in rest {
                        push(QItem::Track {
                            index: i,
                            path: items[i].path.clone(),
                            current: false,
                        });
                    }
                }
            }
            _ => push(QItem::Heading("Nothing is playing".into())),
        }
        let objs: Vec<glib::BoxedAnyObject> =
            model.into_iter().map(glib::BoxedAnyObject::new).collect();
        self.store.splice(0, self.store.n_items(), &objs);
        *self.indices.borrow_mut() = indices;
    }
}
