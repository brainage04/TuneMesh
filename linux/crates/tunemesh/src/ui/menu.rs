//! Track context menus and the small dialogs they open.

use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use tunemesh_core::LIKED_SONGS;

use crate::app::{App, Route};
use crate::store::EntryRef;
use crate::ui::widgets::plural;

/// What a context menu acts on.
pub struct MenuCtx {
    /// Library paths of the tracks (available ones only).
    pub paths: Vec<String>,
    /// Playlist name and the entries as shown, when opened inside a playlist.
    pub playlist: Option<(String, Vec<EntryRef>)>,
    /// Queue index when opened from the queue panel.
    pub queue_index: Option<usize>,
}

fn action(group: &gio::SimpleActionGroup, name: &str, f: impl Fn() + 'static) {
    let a = gio::SimpleAction::new(name, None);
    a.connect_activate(move |_, _| f());
    group.add_action(&a);
}

/// Shows a context menu for tracks at (`x`, `y`) in `parent`.
pub fn popup(app: &Rc<App>, parent: &gtk::Widget, x: f64, y: f64, ctx: MenuCtx) {
    let paths = Rc::new(ctx.paths);
    let menu = gio::Menu::new();
    let group = gio::SimpleActionGroup::new();

    if let Some(index) = ctx.queue_index {
        let section = gio::Menu::new();
        section.append(Some("Remove from queue"), Some("tm.remove-queue"));
        menu.append_section(None, &section);
        let a = app.clone();
        action(&group, "remove-queue", move || a.player.remove_at(index));
    }

    if !paths.is_empty() {
        let section = gio::Menu::new();
        section.append(Some("Play next"), Some("tm.play-next"));
        section.append(Some("Add to queue"), Some("tm.add-queue"));
        menu.append_section(None, &section);
        let (a, p) = (app.clone(), paths.clone());
        action(&group, "play-next", move || {
            a.player.play_next(&p);
            a.toast(&format!(
                "{} will play next",
                plural(p.len(), "song", "songs")
            ));
        });
        let (a, p) = (app.clone(), paths.clone());
        action(&group, "add-queue", move || {
            a.player.add_to_queue(&p);
            a.toast(&format!(
                "Added {} to queue",
                plural(p.len(), "song", "songs")
            ));
        });

        let section = gio::Menu::new();
        let sub = gio::Menu::new();
        let create = gio::Menu::new();
        create.append(Some("New playlist…"), Some("tm.new-playlist"));
        sub.append_section(None, &create);
        let lists = gio::Menu::new();
        for (i, name) in app.store.playlist_names().into_iter().enumerate() {
            let action_name = format!("add-pl-{i}");
            // Menu labels treat `_` as a mnemonic marker; double it so a
            // name like "lo_fi" shows as written.
            lists.append(
                Some(&tunemesh_core::limits::clip_line(&name).replace('_', "__")),
                Some(&format!("tm.{action_name}")),
            );
            let (a, p) = (app.clone(), paths.clone());
            action(&group, &action_name, move || {
                let (a2, n) = (a.clone(), name.clone());
                a.store.add_to_playlist(&name, &p, move |r| match r {
                    Ok(()) => a2.toast(&format!("Added to {n}")),
                    Err(e) => a2.error("Could not add to playlist", e),
                });
            });
        }
        sub.append_section(None, &lists);
        section.append_submenu(Some("Add to playlist"), &sub);
        let (a, p) = (app.clone(), paths.clone());
        action(&group, "new-playlist", move || {
            let (a2, p2) = (a.clone(), p.clone());
            ask_text(&a, "New playlist", "", "Create", move |name| {
                let a3 = a2.clone();
                let p3 = p2.clone();
                a2.store.create_playlist(&name, move |r| match r {
                    Ok(name) => {
                        let a4 = a3.clone();
                        a3.store
                            .add_to_playlist(&name.clone(), &p3, move |r| match r {
                                Ok(()) => a4.toast(&format!("Added to {name}")),
                                Err(e) => a4.error("Could not add to playlist", e),
                            });
                    }
                    Err(e) => a3.error("Could not create playlist", e),
                });
            });
        });

        if let Some((name, positions)) = &ctx.playlist
            && name != LIKED_SONGS
        {
            section.append(Some("Remove from this playlist"), Some("tm.remove-pl"));
            let (a, n, pos) = (app.clone(), name.clone(), positions.clone());
            action(&group, "remove-pl", move || {
                a.store
                    .remove_from_playlist(&n, &pos, a.on_error("Could not update playlist"));
            });
        }
        let all_liked = paths.iter().all(|p| app.store.is_liked(p));
        if all_liked {
            section.append(Some("Remove from Liked Songs"), Some("tm.unlike"));
        } else {
            section.append(Some("Save to Liked Songs"), Some("tm.like"));
        }
        for (id, like) in [("like", true), ("unlike", false)] {
            let (a, p) = (app.clone(), paths.clone());
            action(&group, id, move || {
                a.store
                    .set_liked_paths(&p, like, a.on_error("Could not update Liked Songs"));
            });
        }
        menu.append_section(None, &section);

        if paths.len() == 1 {
            let lib = app.store.library();
            if let Some(t) = lib.track(&paths[0]) {
                let section = gio::Menu::new();
                section.append(Some("Go to album"), Some("tm.go-album"));
                section.append(Some("Go to artist"), Some("tm.go-artist"));
                menu.append_section(None, &section);
                let album = lib.album_of(t).map(|i| lib.albums[i].key());
                let artist = t.listed_artist().to_owned();
                let a = app.clone();
                action(&group, "go-album", move || {
                    if let Some(k) = &album {
                        a.navigate(Route::Album(k.clone()));
                    }
                });
                let a = app.clone();
                action(&group, "go-artist", move || {
                    a.navigate(Route::Artist(artist.clone()))
                });
            }
        }

        let section = gio::Menu::new();
        section.append(Some("Delete from library…"), Some("tm.delete"));
        menu.append_section(None, &section);
        let (a, p) = (app.clone(), paths.clone());
        action(&group, "delete", move || confirm_delete(&a, (*p).clone()));
    } else if let Some((name, positions)) = &ctx.playlist {
        // Unavailable entries can only be removed.
        let section = gio::Menu::new();
        section.append(Some("Remove from this playlist"), Some("tm.remove-pl"));
        menu.append_section(None, &section);
        let (a, n, pos) = (app.clone(), name.clone(), positions.clone());
        action(&group, "remove-pl", move || {
            a.store
                .remove_from_playlist(&n, &pos, a.on_error("Could not update playlist"));
        });
    }

    if menu.n_items() == 0 {
        return;
    }
    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.insert_action_group("tm", Some(&group));
    popover.set_parent(parent);
    popover.set_has_arrow(false);
    popover.set_halign(gtk::Align::Start);
    popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    popover.connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
    popover.popup();
}

/// "Delete from library" with a warning that it deletes everywhere.
pub fn confirm_delete(app: &Rc<App>, paths: Vec<String>) {
    let lib = app.store.library();
    let heading = match paths.as_slice() {
        [one] => format!(
            "Delete “{}”?",
            lib.track(one).map_or(one.as_str(), |t| t.title.as_str())
        ),
        many => format!("Delete {} songs?", many.len()),
    };
    let a = app.clone();
    confirm(
        app,
        &heading,
        "The file is removed from the library folder, so it disappears from every device that syncs it. This cannot be undone here.",
        "Delete",
        move || {
            a.player.remove_paths(&paths);
            match a.store.delete_tracks(&paths) {
                Ok(()) => a.toast(&format!("Deleted {}", plural(paths.len(), "song", "songs"))),
                Err(e) => a.error("Could not delete", e),
            }
        },
    );
}

/// A destructive confirmation dialog.
pub fn confirm(
    app: &App,
    heading: &str,
    body: &str,
    confirm_label: &str,
    on_confirm: impl Fn() + 'static,
) {
    let dialog = adw::AlertDialog::new(Some(heading), Some(body));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("confirm", confirm_label);
    dialog.set_response_appearance("confirm", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.connect_response(None, move |_, response| {
        if response == "confirm" {
            on_confirm();
        }
    });
    dialog.present(Some(&app.window));
}

/// Asks for a line of text (playlist names).
pub fn ask_text(
    app: &App,
    heading: &str,
    initial: &str,
    ok_label: &str,
    on_ok: impl Fn(String) + 'static,
) {
    let dialog = adw::AlertDialog::new(Some(heading), None);
    let entry = gtk::Entry::builder()
        .text(initial)
        .activates_default(true)
        .placeholder_text("Name")
        .build();
    dialog.set_extra_child(Some(&entry));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("ok", ok_label);
    dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("ok"));
    dialog.set_close_response("cancel");
    dialog.set_response_enabled("ok", !initial.trim().is_empty());
    let d = dialog.clone();
    entry.connect_changed(move |e| d.set_response_enabled("ok", !e.text().trim().is_empty()));
    let e = entry.clone();
    dialog.connect_response(None, move |_, response| {
        let text = e.text().trim().to_owned();
        if response == "ok" && !text.is_empty() {
            on_ok(text);
        }
    });
    dialog.present(Some(&app.window));
    entry.grab_focus();
}
