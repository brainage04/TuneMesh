//! Small shared widgets: covers, tiles, buttons, formatting.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use tunemesh_core::limits;

use crate::app::App;
use crate::covers::Source;

pub const TILE_COVER: i32 = 168;

pub fn fmt_duration(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

pub fn fmt_total(ms: u64) -> String {
    let secs = ms / 1000;
    let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    if h > 0 {
        format!("{h} hr {m} min")
    } else if m > 0 {
        format!("{m} min {s} sec")
    } else {
        format!("{s} sec")
    }
}

pub fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A one-line label. Text from the synced folder is cut to the display
/// limit (`limits::clip_line`) before GTK lays it out.
pub fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::builder()
        .label(limits::clip_line(text))
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    for c in classes {
        l.add_css_class(c);
    }
    l
}

pub fn section_title(text: &str) -> gtk::Label {
    let l = label(text, &["tm-section-title"]);
    l.set_margin_bottom(4);
    l
}

pub fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .css_classes(["tm-icon-btn"])
        .build()
}

pub fn play_button(big: bool) -> gtk::Button {
    gtk::Button::builder()
        .icon_name("media-playback-start-symbolic")
        .tooltip_text("Play")
        .css_classes([if big { "tm-play-big" } else { "tm-play-small" }])
        .valign(gtk::Align::Center)
        .halign(gtk::Align::Center)
        .build()
}

/// An empty fixed-size cover image (`size` logical pixels square).
pub fn cover_image(size: i32) -> gtk::Image {
    gtk::Image::builder()
        .pixel_size(size)
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Center)
        .overflow(gtk::Overflow::Hidden)
        .css_classes(["tm-cover"])
        .build()
}

/// A square cover image of `size` logical pixels showing `source`.
pub fn cover(app: &App, source: Source, size: i32, round: bool) -> gtk::Image {
    let p = cover_image(size);
    p.set_valign(gtk::Align::Start);
    if round {
        p.add_css_class("round");
    }
    app.covers.apply(&p, source, size as u32);
    p
}

/// The Liked Songs artwork: a purple gradient with a heart.
pub fn liked_cover(size: i32) -> gtk::Widget {
    let b = gtk::Box::builder()
        .width_request(size)
        .height_request(size)
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Start)
        .css_classes(["tm-liked-cover"])
        .hexpand(false)
        .vexpand(false)
        .build();
    let icon = gtk::Image::builder()
        .icon_name("tm-heart-filled-symbolic")
        .pixel_size((size / 3).max(12))
        .hexpand(true)
        .vexpand(true)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    b.append(&icon);
    b.upcast()
}

pub fn tile_flow() -> gtk::FlowBox {
    gtk::FlowBox::builder()
        .homogeneous(true)
        .selection_mode(gtk::SelectionMode::None)
        .min_children_per_line(2)
        .max_children_per_line(12)
        .column_spacing(0)
        .row_spacing(0)
        .css_classes(["tm-grid"])
        .build()
}

type Callback = Rc<dyn Fn()>;

thread_local! {
    /// The hand cursor every tile shares.
    static POINTER: gtk::gdk::Cursor =
        gtk::gdk::Cursor::from_name("pointer", None).expect("pointer cursor");
}

/// A recyclable cover tile (album, artist, playlist): a cover and two
/// labels, nothing else. Clicks, the play button and the title's tooltip
/// are handled once for the whole page by `tile_host`; a gesture, button
/// and overlay per tile were most of what a grid of 372 tiles cost to make
/// and to destroy.
#[derive(Clone)]
pub struct Tile {
    pub widget: gtk::Box,
    parts: Rc<TileParts>,
}

/// The parts of a tile a binding changes. They are kept on the tile's
/// widget, which owns them; they never hold the widget itself, or the tile
/// would keep itself alive after its page is gone.
struct TileParts {
    picture: gtk::Image,
    /// The Liked Songs artwork, made for the one tile that shows it.
    liked: RefCell<Option<gtk::Widget>>,
    title: gtk::Label,
    subtitle: gtk::Label,
    /// The whole title, for the tooltip.
    full_title: RefCell<String>,
    open: RefCell<Option<Callback>>,
    play: RefCell<Option<Callback>>,
}

pub enum TileArt {
    Cover(Source),
    Liked,
}

impl Tile {
    pub fn new(round: bool) -> Self {
        let picture = cover_image(TILE_COVER);
        picture.add_css_class("tm-cover-shadow");
        if round {
            picture.add_css_class("round");
        }
        let title = label("", &["title"]);
        title.set_margin_top(8);
        title.set_max_width_chars(1);
        title.set_hexpand(true);
        let subtitle = label("", &["subtitle"]);
        subtitle.set_max_width_chars(1);
        if round {
            title.set_xalign(0.5);
            subtitle.set_xalign(0.5);
        }
        let widget = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["tm-tile"])
            .width_request(TILE_COVER + 24)
            .build();
        POINTER.with(|c| widget.set_cursor(Some(c)));
        widget.append(&picture);
        widget.append(&title);
        widget.append(&subtitle);
        let parts = Rc::new(TileParts {
            picture,
            liked: RefCell::new(None),
            title,
            subtitle,
            full_title: RefCell::new(String::new()),
            open: RefCell::new(None),
            play: RefCell::new(None),
        });
        // SAFETY: only read back as `Rc<TileParts>`, in `Tile::find`.
        unsafe { widget.set_data("tm-tile", parts.clone()) };
        Self { widget, parts }
    }

    /// The tile whose widget is `widget` (made by `Tile::new`).
    pub fn of(widget: &gtk::Widget) -> Self {
        Self::find(widget).expect("tile")
    }

    /// The tile `w` is part of, if any.
    fn find(w: &gtk::Widget) -> Option<Self> {
        let mut w = Some(w.clone());
        while let Some(c) = w {
            // SAFETY: set in `Tile::new` with this type.
            if let Some(parts) = unsafe { c.data::<Rc<TileParts>>("tm-tile") } {
                let parts = unsafe { parts.as_ref() }.clone();
                let widget = c.downcast::<gtk::Box>().expect("tile box");
                return Some(Self { widget, parts });
            }
            w = c.parent();
        }
        None
    }

    pub fn set(
        &self,
        app: &App,
        art: TileArt,
        title: &str,
        subtitle: &str,
        open: impl Fn() + 'static,
        play: impl Fn() + 'static,
    ) {
        let p = &self.parts;
        match art {
            TileArt::Cover(source) => {
                if let Some(liked) = p.liked.borrow_mut().take() {
                    self.widget.remove(&liked);
                }
                p.picture.set_visible(true);
                // A recycled tile shows another cover: not the old one
                // until it is loaded. A tile bound again to the same one
                // keeps it.
                if !app.covers.shows(&p.picture, &source) {
                    p.picture.set_paintable(None::<&gtk::gdk::Paintable>);
                }
                app.covers.apply(&p.picture, source, TILE_COVER as u32);
            }
            TileArt::Liked => {
                p.picture.set_visible(false);
                let mut liked = p.liked.borrow_mut();
                if liked.is_none() {
                    let w = liked_cover(TILE_COVER);
                    w.add_css_class("tm-cover-shadow");
                    self.widget.prepend(&w);
                    *liked = Some(w);
                }
            }
        }
        let title = limits::clip_line(title);
        p.title.set_label(&title);
        *p.full_title.borrow_mut() = title.into_owned();
        p.subtitle.set_label(&limits::clip_line(subtitle));
        *p.open.borrow_mut() = Some(Rc::new(open));
        *p.play.borrow_mut() = Some(Rc::new(play));
    }

    /// The cover's widget (picture or Liked Songs artwork).
    fn art(&self) -> gtk::Widget {
        match &*self.parts.liked.borrow() {
            Some(w) => w.clone(),
            None => self.parts.picture.clone().upcast(),
        }
    }
}

/// `child` (a page, or part of one, that holds tiles) with what its tiles
/// share: one play button that moves to the cover under the pointer, one
/// click gesture that opens the tile clicked, and one tooltip handler that
/// shows a tile's whole title.
pub fn tile_host(child: &impl IsA<gtk::Widget>) -> gtk::Overlay {
    let overlay = gtk::Overlay::builder().child(child).build();
    let button = play_button(false);
    button.add_css_class("tm-tile-play");
    button.set_visible(false);
    overlay.add_overlay(&button);
    // The tile under the pointer: held weakly, its list may go first.
    let hovered: Rc<RefCell<Option<glib::WeakRef<gtk::Box>>>> = Rc::default();
    let hovered_tile = {
        let hovered = hovered.clone();
        move || {
            hovered
                .borrow()
                .as_ref()
                .and_then(|w| w.upgrade())
                .map(|w| Tile::of(w.upcast_ref()))
        }
    };

    // Where the button sits: the hovered cover's bottom-right corner.
    let tile = hovered_tile.clone();
    overlay.connect_get_child_position(move |o, w| {
        let tile = tile()?;
        let b = tile.art().compute_bounds(o)?;
        let (_, nat, _, _) = w.measure(gtk::Orientation::Horizontal, -1);
        let (_, nat_h, _, _) = w.measure(gtk::Orientation::Vertical, -1);
        Some(gtk::gdk::Rectangle::new(
            (b.x() + b.width()) as i32 - nat - 8,
            (b.y() + b.height()) as i32 - nat_h - 8,
            nat,
            nat_h,
        ))
    });

    let set_hovered = {
        let (hovered, button, o) = (hovered.clone(), button.clone(), overlay.downgrade());
        move |tile: Option<Tile>| {
            let old = hovered.borrow_mut().take().and_then(|w| w.upgrade());
            if let Some(old) = &old {
                old.remove_css_class("tm-hovered");
            }
            if let Some(t) = &tile {
                t.widget.add_css_class("tm-hovered");
                *hovered.borrow_mut() = Some(t.widget.downgrade());
            }
            button.set_visible(tile.is_some());
            if tile.is_some()
                && old.as_ref() != tile.as_ref().map(|t| &t.widget)
                && let Some(o) = o.upgrade()
            {
                o.queue_allocate();
            }
        }
    };
    let motion = gtk::EventControllerMotion::new();
    let (b, set) = (button.clone(), set_hovered.clone());
    motion.connect_motion(move |m, x, y| {
        let Some(o) = m.widget() else { return };
        let Some(target) = o.pick(x, y, gtk::PickFlags::DEFAULT) else {
            return set(None);
        };
        if target == *b.upcast_ref::<gtk::Widget>() || target.is_ancestor(&b) {
            return;
        }
        set(Tile::find(&target));
    });
    let set = set_hovered.clone();
    motion.connect_leave(move |_| set(None));
    overlay.add_controller(motion);
    // Scrolling moves the tiles away from under the pointer.
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
    scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
    let set = set_hovered.clone();
    scroll.connect_scroll(move |_, _, _| {
        set(None);
        glib::Propagation::Proceed
    });
    overlay.add_controller(scroll);

    let tile = hovered_tile.clone();
    button.connect_clicked(move |_| {
        let cb = tile().and_then(|t| t.parts.play.borrow().clone());
        if let Some(cb) = cb {
            cb();
        }
    });

    // Before the list's own gestures see the click (capture phase).
    let click = gtk::GestureClick::new();
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    let pressed: Rc<RefCell<Option<glib::WeakRef<gtk::Box>>>> = Rc::default();
    let at = {
        let b = button.clone();
        move |g: &gtk::GestureClick, x: f64, y: f64| -> Option<Tile> {
            let target = g.widget()?.pick(x, y, gtk::PickFlags::DEFAULT)?;
            if target == *b.upcast_ref::<gtk::Widget>() || target.is_ancestor(&b) {
                return None;
            }
            Tile::find(&target)
        }
    };
    let (p, at2) = (pressed.clone(), at.clone());
    click.connect_pressed(move |g, n, x, y| {
        *p.borrow_mut() = (n == 1)
            .then(|| at2(g, x, y))
            .flatten()
            .map(|t| t.widget.downgrade());
    });
    click.connect_released(move |g, _, x, y| {
        let Some(down) = pressed.borrow_mut().take().and_then(|w| w.upgrade()) else {
            return;
        };
        let Some(tile) = at(g, x, y).filter(|t| t.widget == down) else {
            return;
        };
        g.set_state(gtk::EventSequenceState::Claimed);
        let cb = tile.parts.open.borrow().clone();
        if let Some(cb) = cb {
            cb();
        }
    });
    overlay.add_controller(click);

    overlay.set_has_tooltip(true);
    overlay.connect_query_tooltip(|o, x, y, keyboard, tooltip| {
        if keyboard {
            return false;
        }
        let Some(target) = o.pick(x as f64, y as f64, gtk::PickFlags::DEFAULT) else {
            return false;
        };
        let Some(tile) = Tile::find(&target) else {
            return false;
        };
        if target != *tile.parts.title.upcast_ref::<gtk::Widget>() {
            return false;
        }
        tooltip.set_text(Some(&tile.parts.full_title.borrow()));
        true
    });
    overlay
}

/// Runs `f` once on the next frame of `widget` however often `request` is
/// called before then: a burst of changes (a queue of broken files fails
/// through hundreds a second) is shown once per frame. While `widget` is
/// not shown, `f` runs when the main loop is idle.
#[derive(Clone)]
pub struct PerFrame {
    pending: Rc<std::cell::Cell<bool>>,
    widget: glib::WeakRef<gtk::Widget>,
    f: Rc<dyn Fn()>,
}

impl PerFrame {
    pub fn new(widget: &impl IsA<gtk::Widget>, f: impl Fn() + 'static) -> Self {
        Self {
            pending: Rc::default(),
            widget: widget.upcast_ref::<gtk::Widget>().downgrade(),
            f: Rc::new(f),
        }
    }

    pub fn request(&self) {
        if self.pending.replace(true) {
            return;
        }
        let (pending, f) = (self.pending.clone(), self.f.clone());
        let run = move || {
            pending.set(false);
            f();
        };
        match self.widget.upgrade().filter(|w| w.is_mapped()) {
            Some(w) => {
                w.add_tick_callback(move |_, _| {
                    run();
                    glib::ControlFlow::Break
                });
            }
            None => {
                glib::idle_add_local_once(run);
            }
        }
    }
}

/// An empty-state message.
pub fn empty(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(limits::clip_text(text))
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .max_width_chars(60)
        .lines(4)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .justify(gtk::Justification::Center)
        .css_classes(["tm-empty"])
        .margin_top(48)
        .margin_bottom(48)
        .build()
}
