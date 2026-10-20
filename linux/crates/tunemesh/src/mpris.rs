//! MPRIS (`org.mpris.MediaPlayer2.tunemesh`): media keys and desktop media
//! controls.

use std::hash::{Hash, Hasher};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use mpris_server::{LoopStatus, Metadata, PlaybackStatus, Player as MprisPlayer, Time, TrackId};

use crate::app::{APP_ID, App};
use crate::covers;
use crate::player::{Change, Repeat};

fn track_id(path: &str) -> TrackId {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    TrackId::try_from(format!(
        "/io/github/brainage04/TuneMesh/Track/T{:016x}",
        h.finish()
    ))
    .unwrap_or(TrackId::NO_TRACK)
}

/// Text for other apps (desktop shells show it): cut to the display limit.
fn clip(s: &str) -> String {
    tunemesh_core::limits::clip_line(s).into_owned()
}

fn metadata(app: &App) -> Metadata {
    let lib = app.store.library();
    let Some(path) = app.player.current_path() else {
        return Metadata::new();
    };
    let Some(t) = lib.track(&path) else {
        let mut m = Metadata::new();
        m.set_trackid(Some(track_id(&path)));
        m.set_title(Some(clip(path.rsplit('/').next().unwrap_or(&path))));
        return m;
    };
    let root = app.store.root();
    let mut b = Metadata::builder()
        .trackid(track_id(&path))
        .title(clip(&t.title))
        .artist([clip(&t.artist)])
        .album(clip(&t.album))
        .length(Time::from_millis(t.duration_ms as i64))
        .url(
            glib::filename_to_uri(root.join(&t.path), None)
                .map(|u| u.to_string())
                .unwrap_or_default(),
        );
    if let Some(aa) = &t.album_artist {
        b = b.album_artist([clip(aa)]);
    }
    if let Some(n) = t.track_number {
        b = b.track_number(n as i32);
    }
    if let Some(file) = covers::art_file(&root, t)
        && let Ok(uri) = glib::filename_to_uri(&file, None)
    {
        b = b.art_url(uri.to_string());
    }
    b.build()
}

fn status(app: &App) -> PlaybackStatus {
    if !app.player.has_track() {
        PlaybackStatus::Stopped
    } else if app.player.is_playing() {
        PlaybackStatus::Playing
    } else {
        PlaybackStatus::Paused
    }
}

fn loop_status(r: Repeat) -> LoopStatus {
    match r {
        Repeat::Off => LoopStatus::None,
        Repeat::All => LoopStatus::Playlist,
        Repeat::One => LoopStatus::Track,
    }
}

/// Registers the MPRIS service and keeps it in sync with the player.
pub fn start(app: &Rc<App>) {
    let app = app.clone();
    glib::spawn_future_local(async move {
        let built = MprisPlayer::builder("tunemesh")
            .identity("TuneMesh")
            .desktop_entry(APP_ID)
            // No URI schemes or MIME types: OpenUri is not supported (the
            // app only plays files of its library), and MPRIS clients read
            // empty lists as "do not call OpenUri".
            .can_raise(true)
            .can_quit(true)
            .can_play(true)
            .can_pause(true)
            .can_seek(true)
            .can_go_next(true)
            .can_go_previous(true)
            .can_control(true)
            .playback_status(status(&app))
            .loop_status(loop_status(app.player.repeat()))
            .shuffle(app.player.shuffle())
            .volume(app.player.volume() / 100.0)
            .metadata(metadata(&app))
            .build()
            .await;
        let mpris = match built {
            Ok(p) => Rc::new(p),
            Err(e) => {
                eprintln!("tunemesh: MPRIS unavailable: {e}");
                return;
            }
        };
        mpris.set_position(Time::from_micros((app.player.position() * 1e6) as i64));

        let a = app.clone();
        mpris.connect_play_pause(move |_| a.player.toggle());
        let a = app.clone();
        mpris.connect_play(move |_| a.player.play());
        let a = app.clone();
        mpris.connect_pause(move |_| a.player.pause());
        let a = app.clone();
        mpris.connect_stop(move |_| a.player.pause());
        let a = app.clone();
        mpris.connect_next(move |_| a.player.next());
        let a = app.clone();
        mpris.connect_previous(move |_| a.player.previous());
        let a = app.clone();
        mpris.connect_seek(move |_, offset| {
            let target = a.player.position() + offset.as_micros() as f64 / 1e6;
            if target >= a.player.duration() && a.player.duration() > 0.0 {
                a.player.next();
            } else {
                a.player.seek(target);
            }
        });
        let a = app.clone();
        mpris.connect_set_position(move |_, id, pos| {
            if a.player.current_path().map(|p| track_id(&p)).as_ref() == Some(id) {
                a.player.seek(pos.as_micros() as f64 / 1e6);
            }
        });
        let a = app.clone();
        mpris.connect_set_loop_status(move |_, s| {
            a.player.set_repeat(match s {
                LoopStatus::None => Repeat::Off,
                LoopStatus::Playlist => Repeat::All,
                LoopStatus::Track => Repeat::One,
            })
        });
        let a = app.clone();
        mpris.connect_set_shuffle(move |_, on| a.player.set_shuffle(on));
        let a = app.clone();
        mpris.connect_set_volume(move |_, v| a.player.set_volume(v * 100.0));
        let a = app.clone();
        mpris.connect_raise(move |_| a.present());
        let a = app.clone();
        mpris.connect_quit(move |_| a.gtk_app.activate_action("quit", None));

        let weak_app = Rc::downgrade(&app);
        let m = mpris.clone();
        app.player.connect(move |c| {
            let Some(app) = weak_app.upgrade() else {
                return;
            };
            let m = m.clone();
            match c {
                Change::Position => {
                    m.set_position(Time::from_micros((app.player.position() * 1e6) as i64));
                }
                Change::Seeked => {
                    let t = Time::from_micros((app.player.position() * 1e6) as i64);
                    m.set_position(t);
                    glib::spawn_future_local(async move {
                        let _ = m.seeked(t).await;
                    });
                }
                Change::Track => {
                    let md = metadata(&app);
                    let pos = Time::from_micros((app.player.position() * 1e6) as i64);
                    m.set_position(pos);
                    let st = status(&app);
                    glib::spawn_future_local(async move {
                        let _ = m.set_metadata(md).await;
                        let _ = m.set_playback_status(st).await;
                    });
                }
                Change::Playing => {
                    let st = status(&app);
                    m.set_position(Time::from_micros((app.player.position() * 1e6) as i64));
                    glib::spawn_future_local(async move {
                        let _ = m.set_playback_status(st).await;
                    });
                }
                Change::Options | Change::Queue => {
                    let (l, s, v) = (
                        loop_status(app.player.repeat()),
                        app.player.shuffle(),
                        app.player.volume() / 100.0,
                    );
                    glib::spawn_future_local(async move {
                        if m.loop_status() != l {
                            let _ = m.set_loop_status(l).await;
                        }
                        if m.shuffle() != s {
                            let _ = m.set_shuffle(s).await;
                        }
                        if (m.volume() - v).abs() > 1e-6 {
                            let _ = m.set_volume(v).await;
                        }
                    });
                }
            }
        });
        mpris.run().await;
    });
}
