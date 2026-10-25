//! Small shared widgets: covers, tiles, buttons, formatting.

use std::cell::RefCell;
use std::rc::Rc;

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

/// A recyclable cover tile (album, artist, playlist).
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
    liked: gtk::Widget,
    title: gtk::Label,
    subtitle: gtk::Label,
    open: Rc<RefCell<Option<Callback>>>,
    play: Rc<RefCell<Option<Callback>>>,
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
        let liked = liked_cover(TILE_COVER);
        liked.set_visible(false);
        let stack = gtk::Box::new(gtk::Orientation::Vertical, 0);
        stack.append(&picture);
        stack.append(&liked);
        let overlay = gtk::Overlay::builder()
            .child(&stack)
            .halign(gtk::Align::Start)
            .build();
        let play_btn = play_button(false);
        play_btn.set_halign(gtk::Align::End);
        play_btn.set_valign(gtk::Align::End);
        play_btn.add_css_class("tm-tile-play");
        overlay.add_overlay(&play_btn);

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
            .cursor(&gtk::gdk::Cursor::from_name("pointer", None).expect("pointer cursor"))
            .build();
        widget.append(&overlay);
        widget.append(&title);
        widget.append(&subtitle);

        let open: Rc<RefCell<Option<Callback>>> = Rc::new(RefCell::new(None));
        let play: Rc<RefCell<Option<Callback>>> = Rc::new(RefCell::new(None));
        let click = gtk::GestureClick::new();
        let o = open.clone();
        click.connect_released(move |g, _, _, _| {
            g.set_state(gtk::EventSequenceState::Claimed);
            let cb = o.borrow().clone();
            if let Some(cb) = cb {
                cb();
            }
        });
        widget.add_controller(click);
        let p = play.clone();
        play_btn.connect_clicked(move |_| {
            let cb = p.borrow().clone();
            if let Some(cb) = cb {
                cb();
            }
        });
        let parts = Rc::new(TileParts {
            picture,
            liked,
            title,
            subtitle,
            open,
            play,
        });
        // SAFETY: only read back as `Rc<TileParts>`, in `Tile::of`.
        unsafe { widget.set_data("tm-tile", parts.clone()) };
        Self { widget, parts }
    }

    /// The tile whose widget is `widget` (made by `Tile::new`).
    pub fn of(widget: &gtk::Widget) -> Self {
        let widget = widget.clone().downcast::<gtk::Box>().expect("tile box");
        // SAFETY: set in `Tile::new` with this type.
        let parts = unsafe {
            widget
                .data::<Rc<TileParts>>("tm-tile")
                .expect("tile")
                .as_ref()
                .clone()
        };
        Self { widget, parts }
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
        match art {
            TileArt::Cover(source) => {
                self.parts.picture.set_visible(true);
                self.parts.liked.set_visible(false);
                self.parts
                    .picture
                    .set_paintable(None::<&gtk::gdk::Paintable>);
                app.covers
                    .apply(&self.parts.picture, source, TILE_COVER as u32);
            }
            TileArt::Liked => {
                self.parts.picture.set_visible(false);
                self.parts.liked.set_visible(true);
            }
        }
        let title = limits::clip_line(title);
        self.parts.title.set_label(&title);
        self.parts.title.set_tooltip_text(Some(&title));
        self.parts.subtitle.set_label(&limits::clip_line(subtitle));
        *self.parts.open.borrow_mut() = Some(Rc::new(open));
        *self.parts.play.borrow_mut() = Some(Rc::new(play));
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
