//! The persistent bottom bar: current track, transport, seek, queue, volume.

use std::cell::Cell;
use std::rc::{Rc, Weak};

use gtk::prelude::*;
use tunemesh_core::limits;

use crate::app::{App, Route};
use crate::covers::Source;
use crate::player::{Change, Repeat};
use crate::store::StoreChange;
use crate::ui::widgets::{fmt_duration, icon_button, label};

pub struct NowPlaying {
    pub widget: gtk::CenterBox,
    app: Weak<App>,
    cover: gtk::Image,
    title: gtk::Label,
    title_btn: gtk::Button,
    artist: gtk::Label,
    artist_btn: gtk::Button,
    heart: gtk::Button,
    shuffle: gtk::Button,
    prev: gtk::Button,
    play: gtk::Button,
    next: gtk::Button,
    repeat: gtk::Button,
    pos_label: gtk::Label,
    dur_label: gtk::Label,
    seek: gtk::Scale,
    queue_btn: gtk::Button,
    volume_btn: gtk::Button,
    pub volume: gtk::Scale,
    last_volume: Cell<f64>,
}

impl NowPlaying {
    pub fn new(app: &Rc<App>) -> Rc<Self> {
        // The controls stay centred; the sides give up width first in a
        // narrow window.
        let widget = gtk::CenterBox::builder()
            .css_classes(["tm-nowplaying"])
            .build();

        // Left: track info.
        let left = gtk::Box::builder()
            .spacing(12)
            .width_request(150)
            .margin_end(16)
            .valign(gtk::Align::Center)
            .build();
        let cover = crate::ui::widgets::cover_image(56);
        left.append(&cover);
        let text = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .valign(gtk::Align::Center)
            .spacing(2)
            .build();
        let title = label("", &["title"]);
        title.set_max_width_chars(28);
        let title_btn = gtk::Button::builder()
            .child(&title)
            .css_classes(["tm-link", "flat"])
            .halign(gtk::Align::Start)
            .build();
        let artist = label("", &["artist"]);
        artist.set_max_width_chars(32);
        let artist_btn = gtk::Button::builder()
            .child(&artist)
            .css_classes(["tm-link", "flat"])
            .halign(gtk::Align::Start)
            .build();
        text.append(&title_btn);
        text.append(&artist_btn);
        left.append(&text);
        let heart = icon_button("tm-heart-outline-symbolic", "Save to Liked Songs");
        heart.add_css_class("tm-heart");
        heart.set_valign(gtk::Align::Center);
        left.append(&heart);
        widget.set_start_widget(Some(&left));

        // Centre: transport and seek bar.
        let center = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .valign(gtk::Align::Center)
            .spacing(4)
            .build();
        let controls = gtk::Box::builder()
            .spacing(16)
            .halign(gtk::Align::Center)
            .build();
        let shuffle = icon_button("media-playlist-shuffle-symbolic", "Enable shuffle");
        let prev = icon_button("media-skip-backward-symbolic", "Previous");
        let play = gtk::Button::builder()
            .icon_name("media-playback-start-symbolic")
            .tooltip_text("Play")
            .css_classes(["tm-play-circle"])
            .valign(gtk::Align::Center)
            .build();
        let next = icon_button("media-skip-forward-symbolic", "Next");
        let repeat = icon_button("media-playlist-repeat-symbolic", "Enable repeat");
        for b in [&shuffle, &prev, &play, &next, &repeat] {
            controls.append(b);
        }
        center.append(&controls);
        let seek_row = gtk::Box::builder().spacing(8).build();
        let pos_label = gtk::Label::builder()
            .label("0:00")
            .width_chars(5)
            .xalign(1.0)
            .css_classes(["time"])
            .build();
        let seek = gtk::Scale::builder()
            .orientation(gtk::Orientation::Horizontal)
            .adjustment(&gtk::Adjustment::new(0.0, 0.0, 1.0, 1.0, 10.0, 0.0))
            .draw_value(false)
            .width_request(160)
            .hexpand(true)
            .css_classes(["tm-seek"])
            .build();
        let dur_label = gtk::Label::builder()
            .label("0:00")
            .width_chars(5)
            .xalign(0.0)
            .css_classes(["time"])
            .build();
        seek_row.append(&pos_label);
        seek_row.append(&seek);
        seek_row.append(&dur_label);
        center.append(&seek_row);
        let clamp = adw::Clamp::builder()
            .maximum_size(640)
            .tightening_threshold(640)
            .child(&center)
            .build();
        clamp.set_size_request(220, -1);
        clamp.set_hexpand(true);
        widget.set_center_widget(Some(&clamp));

        // Right: queue and volume.
        let right = gtk::Box::builder()
            .spacing(8)
            .width_request(110)
            .margin_start(16)
            .halign(gtk::Align::End)
            .valign(gtk::Align::Center)
            .build();
        let queue_btn = icon_button("view-list-ordered-symbolic", "Queue");
        let volume_btn = icon_button("audio-volume-high-symbolic", "Mute");
        let volume = gtk::Scale::builder()
            .orientation(gtk::Orientation::Horizontal)
            .adjustment(&gtk::Adjustment::new(
                app.player.volume(),
                0.0,
                100.0,
                1.0,
                10.0,
                0.0,
            ))
            .draw_value(false)
            .width_request(70)
            .css_classes(["tm-volume"])
            .build();
        let spacer = gtk::Box::builder().hexpand(true).build();
        right.append(&spacer);
        right.append(&queue_btn);
        right.append(&volume_btn);
        right.append(&volume);
        widget.set_end_widget(Some(&right));

        let np = Rc::new(Self {
            widget,
            app: Rc::downgrade(app),
            cover,
            title,
            title_btn,
            artist,
            artist_btn,
            heart,
            shuffle,
            prev,
            play,
            next,
            repeat,
            pos_label,
            dur_label,
            seek,
            queue_btn,
            volume_btn,
            volume,
            last_volume: Cell::new(70.0),
        });
        np.connect(app);
        np.update_track();
        np.update_playing();
        np.update_position();
        np.update_options();
        np
    }

    fn connect(self: &Rc<Self>, app: &Rc<App>) {
        let a = app.clone();
        self.play.connect_clicked(move |_| a.player.toggle());
        let a = app.clone();
        self.prev.connect_clicked(move |_| a.player.previous());
        let a = app.clone();
        self.next.connect_clicked(move |_| a.player.next());
        let a = app.clone();
        self.shuffle
            .connect_clicked(move |_| a.player.set_shuffle(!a.player.shuffle()));
        let a = app.clone();
        self.repeat
            .connect_clicked(move |_| a.player.cycle_repeat());
        let a = app.clone();
        self.queue_btn.connect_clicked(move |_| a.toggle_queue());
        let me = Rc::downgrade(self);
        app.queue_split.connect_show_sidebar_notify(move |_| {
            if let Some(me) = me.upgrade() {
                me.update_options();
            }
        });
        let a = app.clone();
        self.seek.connect_change_value(move |_, _, v| {
            a.player.seek(v);
            gtk::glib::Propagation::Proceed
        });
        let a = app.clone();
        self.volume
            .connect_value_changed(move |s| a.player.set_volume(s.value()));
        let (a, me) = (app.clone(), Rc::downgrade(self));
        self.volume_btn.connect_clicked(move |_| {
            let Some(me) = me.upgrade() else { return };
            let v = a.player.volume();
            if v > 0.0 {
                me.last_volume.set(v);
                me.volume.set_value(0.0);
            } else {
                me.volume.set_value(me.last_volume.get().max(10.0));
            }
        });
        let a = app.clone();
        self.heart.connect_clicked(move |_| {
            if let Some(p) = a.player.current_path()
                && let Err(e) = a.store.toggle_like(&p)
            {
                a.error("Could not update Liked Songs", e);
            }
        });
        let a = app.clone();
        self.title_btn.connect_clicked(move |_| {
            let lib = a.store.library();
            if let Some(t) = a.player.current_path().and_then(|p| lib.track(&p).cloned())
                && let Some(i) = lib.album_of(&t)
            {
                a.navigate(Route::Album(lib.albums[i].key()));
            }
        });
        let a = app.clone();
        self.artist_btn.connect_clicked(move |_| {
            let lib = a.store.library();
            if let Some(t) = a.player.current_path().and_then(|p| lib.track(&p).cloned()) {
                a.navigate(Route::Artist(t.listed_artist().to_owned()));
            }
        });

        let me = self.clone();
        app.player.connect(move |c| match c {
            Change::Track => {
                me.update_track();
                me.update_position();
            }
            Change::Playing => me.update_playing(),
            Change::Position | Change::Seeked => me.update_position(),
            Change::Options | Change::Queue => me.update_options(),
        });
        let me = Rc::downgrade(self);
        app.store.connect(move |c| {
            if let Some(me) = me.upgrade()
                && matches!(c, StoreChange::Liked | StoreChange::Library)
            {
                me.update_track();
            }
        });
    }

    fn update_track(&self) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let lib = app.store.library();
        let path = app.player.current_path();
        let track = path.as_deref().and_then(|p| lib.track(p));
        let has = path.is_some();
        self.cover.set_visible(has);
        self.heart.set_visible(track.is_some());
        match track {
            Some(t) => {
                self.title.set_label(&limits::clip_line(&t.title));
                self.artist.set_label(&limits::clip_line(&t.artist));
                app.covers.apply(&self.cover, Source::for_track(t), 56);
                let liked = app.store.is_liked(&t.path);
                self.heart.set_icon_name(if liked {
                    "tm-heart-filled-symbolic"
                } else {
                    "tm-heart-outline-symbolic"
                });
                self.heart.set_tooltip_text(Some(if liked {
                    "Remove from Liked Songs"
                } else {
                    "Save to Liked Songs"
                }));
                if liked {
                    self.heart.add_css_class("liked")
                } else {
                    self.heart.remove_css_class("liked")
                }
                app.window.set_title(Some(&limits::clip_line(&format!(
                    "{} • {}",
                    t.title, t.artist
                ))));
            }
            None => {
                self.title.set_label(
                    path.as_deref()
                        .map(|p| p.rsplit('/').next().unwrap_or(p))
                        .unwrap_or(""),
                );
                self.artist.set_label(if has { "Unavailable" } else { "" });
                app.window.set_title(Some("TuneMesh"));
            }
        }
        let enabled = has;
        for b in [&self.play, &self.prev, &self.next] {
            b.set_sensitive(enabled);
        }
        self.seek.set_sensitive(enabled);
    }

    fn update_playing(&self) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let playing = app.player.is_playing();
        self.play.set_icon_name(if playing {
            "media-playback-pause-symbolic"
        } else {
            "media-playback-start-symbolic"
        });
        self.play
            .set_tooltip_text(Some(if playing { "Pause" } else { "Play" }));
    }

    fn update_position(&self) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let mut dur = app.player.duration();
        if dur <= 0.0 {
            let lib = app.store.library();
            dur = app
                .player
                .current_path()
                .and_then(|p| lib.track(&p).map(|t| t.duration_ms as f64 / 1000.0))
                .unwrap_or(0.0);
        }
        let pos = app.player.position().min(dur.max(0.0));
        let adj = self.seek.adjustment();
        adj.set_upper(dur.max(1.0));
        adj.set_value(pos);
        self.pos_label.set_label(&fmt_duration(pos as u64));
        self.dur_label.set_label(&fmt_duration(dur.round() as u64));
    }

    fn update_options(&self) {
        let Some(app) = self.app.upgrade() else {
            return;
        };
        let p = &app.player;
        if p.shuffle() {
            self.shuffle.add_css_class("on");
            self.shuffle.set_tooltip_text(Some("Disable shuffle"));
        } else {
            self.shuffle.remove_css_class("on");
            self.shuffle.set_tooltip_text(Some("Enable shuffle"));
        }
        match p.repeat() {
            Repeat::Off => {
                self.repeat.remove_css_class("on");
                self.repeat.set_icon_name("media-playlist-repeat-symbolic");
                self.repeat.set_tooltip_text(Some("Enable repeat"));
            }
            Repeat::All => {
                self.repeat.add_css_class("on");
                self.repeat.set_icon_name("media-playlist-repeat-symbolic");
                self.repeat.set_tooltip_text(Some("Enable repeat one"));
            }
            Repeat::One => {
                self.repeat.add_css_class("on");
                self.repeat
                    .set_icon_name("media-playlist-repeat-song-symbolic");
                self.repeat.set_tooltip_text(Some("Disable repeat"));
            }
        }
        let v = p.volume();
        if (self.volume.value() - v).abs() > 0.5 {
            self.volume.set_value(v);
        }
        self.volume_btn.set_icon_name(match v {
            v if v <= 0.0 => "audio-volume-muted-symbolic",
            v if v < 34.0 => "audio-volume-low-symbolic",
            v if v < 67.0 => "audio-volume-medium-symbolic",
            _ => "audio-volume-high-symbolic",
        });
        let revealed = app.queue_shown();
        if revealed {
            self.queue_btn.add_css_class("on")
        } else {
            self.queue_btn.remove_css_class("on")
        }
    }
}
