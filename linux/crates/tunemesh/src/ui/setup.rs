//! The setup assistant: library folder, Syncthing sharing, downloader.
//! Shown on first run and from Settings; every step can be skipped.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::app::App;
use crate::config::{self, expand_home};
use crate::syncthing::{self, Client, Endpoint, Overview, Probe};
use crate::ui::downloads;

struct Setup {
    app: Rc<App>,
    dialog: adw::Dialog,
    nav: adw::NavigationView,
    toasts: adw::ToastOverlay,
    sync_body: gtk::Box,
    /// The endpoint that last answered, reused for later calls.
    endpoint: RefCell<Option<Endpoint>>,
    busy: Cell<bool>,
}

/// Opens the assistant over the main window.
pub fn open(app: &Rc<App>) {
    let dialog = adw::Dialog::builder()
        .title("Set Up TuneMesh")
        .content_width(620)
        .content_height(720)
        .build();
    let nav = adw::NavigationView::new();
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&nav));
    dialog.set_child(Some(&toasts));
    let s = Rc::new(Setup {
        app: app.clone(),
        dialog: dialog.clone(),
        nav: nav.clone(),
        toasts,
        sync_body: gtk::Box::new(gtk::Orientation::Vertical, 18),
        endpoint: RefCell::new(None),
        busy: Cell::new(false),
    });
    nav.add(&s.library_page());
    let a = app.clone();
    dialog.connect_closed(move |_| {
        if !a.config.borrow().setup_done {
            let mut c = a.config.borrow_mut();
            c.setup_done = true;
            if let Err(e) = c.save() {
                eprintln!("tunemesh: saving settings: {e}");
            }
        }
    });
    dialog.present(Some(&app.window));
}

fn body_text(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .build()
}

fn dim_text(text: &str) -> gtk::Label {
    let l = body_text(text);
    l.add_css_class("dim-label");
    l
}

/// A copyable command line.
fn command_row(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .selectable(true)
        .css_classes(["monospace"])
        .build()
}

fn pill(label: &str, suggested: bool) -> gtk::Button {
    let b = gtk::Button::builder()
        .label(label)
        .css_classes(["pill"])
        .build();
    if suggested {
        b.add_css_class("suggested-action");
    }
    b
}

/// A page: header bar, scrolling body, and a bottom row of buttons.
fn page(tag: &str, title: &str, body: &gtk::Box, buttons: &[&gtk::Button]) -> adw::NavigationPage {
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    // The dialog has a fixed width; an AdwClamp here lays out too narrow on
    // libadwaita 1.5.
    body.set_margin_top(12);
    body.set_margin_bottom(24);
    body.set_margin_start(48);
    body.set_margin_end(48);
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(body)
        .build();
    view.set_content(Some(&scroller));
    let bar = gtk::Box::builder()
        .spacing(12)
        .halign(gtk::Align::End)
        .margin_top(12)
        .margin_bottom(12)
        .margin_end(18)
        .build();
    for b in buttons {
        bar.append(*b);
    }
    view.add_bottom_bar(&bar);
    adw::NavigationPage::builder()
        .tag(tag)
        .title(title)
        .child(&view)
        .build()
}

fn heading(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .css_classes(["title-2"])
        .build()
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// A QR code of `text` as a texture, `scale` pixels per module.
fn qr_texture(text: &str, scale: usize) -> Option<gdk::MemoryTexture> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let modules = code.width();
    let colors = code.to_colors();
    let quiet = 4;
    let size = (modules + 2 * quiet) * scale;
    let mut rgba = vec![255u8; size * size * 4];
    for (i, c) in colors.iter().enumerate() {
        if *c != qrcode::Color::Dark {
            continue;
        }
        let (mx, my) = (i % modules + quiet, i / modules + quiet);
        for y in my * scale..(my + 1) * scale {
            for x in mx * scale..(mx + 1) * scale {
                let p = (y * size + x) * 4;
                rgba[p..p + 3].fill(0);
            }
        }
    }
    let bytes = glib::Bytes::from_owned(rgba);
    Some(gdk::MemoryTexture::new(
        size as i32,
        size as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        size * 4,
    ))
}

impl Setup {
    fn toast(&self, text: &str) {
        let t = adw::Toast::new(text);
        // Toasts are markup by default; messages carry server and file text.
        t.set_use_markup(false);
        t.set_timeout(5);
        self.toasts.add_toast(t);
    }

    fn skip_button(self: &Rc<Self>, label: &str) -> gtk::Button {
        let b = pill(label, false);
        let d = self.dialog.clone();
        b.connect_clicked(move |_| {
            d.close();
        });
        b
    }

    // ---- step 1: library folder ------------------------------------------

    fn library_page(self: &Rc<Self>) -> adw::NavigationPage {
        let body = gtk::Box::new(gtk::Orientation::Vertical, 18);
        body.append(&heading("Welcome to TuneMesh"));
        body.append(&body_text(
            "TuneMesh plays the music in one folder and keeps it, your playlists and likes in sync between your computers and phones with Syncthing. Choose where that folder lives on this computer.",
        ));
        let group = adw::PreferencesGroup::builder()
            .title("Library folder")
            .build();
        let entry = adw::EntryRow::builder()
            .title("Folder")
            .text(self.app.store.root().to_string_lossy())
            .build();
        let choose = gtk::Button::builder()
            .icon_name("folder-open-symbolic")
            .tooltip_text("Choose…")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        entry.add_suffix(&choose);
        group.add(&entry);
        body.append(&group);
        body.append(&dim_text(&format!(
            "The default is {}. An existing folder works too, for example one Syncthing already shares; it is created if it does not exist.",
            config::default_library().display()
        )));

        let (s, e) = (self.clone(), entry.clone());
        choose.connect_clicked(move |_| {
            let dialog = gtk::FileDialog::builder()
                .title("Choose the library folder")
                .modal(true)
                .build();
            let current = expand_home(e.text().trim());
            let start = if current.is_dir() {
                current
            } else {
                config::real_home()
            };
            dialog.set_initial_folder(Some(&gio::File::for_path(start)));
            let e = e.clone();
            dialog.select_folder(Some(&s.app.window), None::<&gio::Cancellable>, move |res| {
                if let Ok(folder) = res
                    && let Some(path) = folder.path()
                {
                    e.set_text(&path.to_string_lossy());
                }
            });
        });

        let next = pill("Continue", true);
        let (s, e) = (self.clone(), entry.clone());
        next.connect_clicked(move |_| {
            let path = expand_home(e.text().trim());
            if path.as_os_str().is_empty() || !path.is_absolute() {
                s.toast("Enter the full path of a folder");
                return;
            }
            if let Err(err) = std::fs::create_dir_all(&path) {
                s.toast(&format!("Could not create {}: {err}", path.display()));
                return;
            }
            if !same_dir(&path, &s.app.store.root()) {
                s.app.set_library_path(path);
                s.app.refresh();
            }
            s.nav.push(&s.syncthing_page());
            s.probe();
        });
        page(
            "library",
            "Library",
            &body,
            &[&self.skip_button("Skip Setup"), &next],
        )
    }

    // ---- step 2: Syncthing -------------------------------------------------

    fn syncthing_page(self: &Rc<Self>) -> adw::NavigationPage {
        let body = gtk::Box::new(gtk::Orientation::Vertical, 18);
        body.append(&heading("Sync with Syncthing"));
        body.append(&body_text(
            "Syncthing copies the library folder between your devices. TuneMesh can set up the folder in the Syncthing running on this computer and share it with your other devices.",
        ));
        // Going back and forward again builds a new page around the section.
        if let Some(old) = self.sync_body.parent().and_downcast::<gtk::Box>() {
            old.remove(&self.sync_body);
        }
        body.append(&self.sync_body);
        let next = pill("Continue", true);
        let s = self.clone();
        next.connect_clicked(move |_| s.nav.push(&s.downloads_page()));
        let skip = pill("Skip", false);
        let s = self.clone();
        skip.connect_clicked(move |_| s.nav.push(&s.downloads_page()));
        page("syncthing", "Syncthing", &body, &[&skip, &next])
    }

    fn clear_sync_body(&self) {
        while let Some(c) = self.sync_body.first_child() {
            self.sync_body.remove(&c);
        }
    }

    /// Looks for Syncthing off the main thread and shows the result.
    fn probe(self: &Rc<Self>) {
        let manual = self.endpoint.borrow().clone();
        self.run(
            "Looking for Syncthing…",
            move || syncthing::probe(manual),
            |s, probe| s.show_probe(probe),
        );
    }

    /// Runs `work` on a worker thread with a spinner in the Syncthing
    /// section, then hands the result to `done` on the main thread.
    fn run<T: Send + 'static>(
        self: &Rc<Self>,
        what: &str,
        work: impl FnOnce() -> T + Send + 'static,
        done: impl FnOnce(&Rc<Self>, T) + 'static,
    ) {
        if self.busy.replace(true) {
            return;
        }
        self.clear_sync_body();
        let row = gtk::Box::builder().spacing(12).build();
        let spinner = gtk::Spinner::builder().spinning(true).build();
        row.append(&spinner);
        row.append(&dim_text(what));
        self.sync_body.append(&row);
        let s = self.clone();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(work).await;
            s.busy.set(false);
            match result {
                Ok(v) => done(&s, v),
                Err(_) => {
                    s.clear_sync_body();
                    s.sync_body.append(&dim_text(
                        "Something went wrong while talking to Syncthing.",
                    ));
                    s.sync_body.append(&s.retry_button());
                }
            }
        });
    }

    fn retry_button(self: &Rc<Self>) -> gtk::Button {
        let b = gtk::Button::builder()
            .label("Check Again")
            .halign(gtk::Align::Start)
            .css_classes(["pill"])
            .build();
        let s = self.clone();
        b.connect_clicked(move |_| s.probe());
        b
    }

    fn show_probe(self: &Rc<Self>, probe: Probe) {
        self.clear_sync_body();
        let b = &self.sync_body;
        match probe {
            Probe::Connected(client, overview) => {
                *self.endpoint.borrow_mut() = Some(client.endpoint().clone());
                self.show_connected(client, overview);
            }
            Probe::NotInstalled => {
                let group = adw::PreferencesGroup::builder()
                    .title("Syncthing is not installed")
                    .description("Install it from your distribution's packages, then start it for your user:")
                    .build();
                b.append(&group);
                for (_, hint) in syncthing::install_hints() {
                    b.append(&command_row(hint));
                }
                b.append(&command_row(
                    "systemctl --user enable --now syncthing.service",
                ));
                b.append(&dim_text("Syncthing for Android (for example Syncthing-Fork from F-Droid) syncs the folder to your phone."));
                b.append(&self.retry_button());
                b.append(&self.manual_form(false, None));
            }
            Probe::NotRunning {
                installed,
                config,
                address,
            } => {
                let group = adw::PreferencesGroup::builder()
                    .title("Syncthing is not running")
                    .description(if installed {
                        "Syncthing is installed but does not answer. Start it, for example with:"
                    } else {
                        "Found a Syncthing configuration, but nothing answers at its address. Start Syncthing, for example with:"
                    })
                    .build();
                b.append(&group);
                b.append(&command_row(
                    "systemctl --user enable --now syncthing.service",
                ));
                b.append(&dim_text("or open Syncthing from your application menu."));
                if let Some(p) = config {
                    b.append(&dim_text(&format!("Configuration: {}", p.display())));
                }
                b.append(&self.retry_button());
                b.append(&self.manual_form(false, address));
            }
            Probe::NeedKey { reason, address } => {
                let group = adw::PreferencesGroup::builder()
                    .title("Connect to Syncthing")
                    // Group descriptions are Pango markup; `reason` may hold
                    // text from the Syncthing server.
                    .description(format!(
                        "{}. Open the Syncthing web interface (usually http://127.0.0.1:8384), go to Actions → Settings → General and copy the API key here.",
                        glib::markup_escape_text(&reason)
                    ))
                    .build();
                b.append(&group);
                b.append(&self.manual_form(true, address));
                b.append(&self.retry_button());
            }
        }
    }

    /// Address and API key typed by the user; `address` is the one found in
    /// Syncthing's configuration, if any.
    fn manual_form(self: &Rc<Self>, expanded: bool, address: Option<String>) -> gtk::Widget {
        let group = adw::PreferencesGroup::new();
        let current = self
            .endpoint
            .borrow()
            .as_ref()
            .map(|e| e.base.clone())
            .or(address);
        let address = adw::EntryRow::builder()
            .title("Syncthing address")
            .text(current.as_deref().unwrap_or(syncthing::DEFAULT_ADDRESS))
            .build();
        let key = adw::PasswordEntryRow::builder().title("API key").build();
        group.add(&address);
        group.add(&key);
        let connect = gtk::Button::builder()
            .label("Connect")
            .halign(gtk::Align::Start)
            .css_classes(["pill", "suggested-action"])
            .build();
        let box_ = gtk::Box::new(gtk::Orientation::Vertical, 12);
        box_.append(&group);
        box_.append(&connect);
        let s = self.clone();
        connect.connect_clicked(move |_| {
            let Some(base) = syncthing::normalize_address(&address.text(), false) else {
                s.toast("Enter an address like http://127.0.0.1:8384");
                return;
            };
            let api_key = key.text().trim().to_owned();
            if api_key.is_empty() {
                s.toast("Paste the API key from Syncthing's settings");
                return;
            }
            *s.endpoint.borrow_mut() = Some(Endpoint { base, api_key });
            s.probe();
        });
        if expanded {
            return box_.upcast();
        }
        let expander = gtk::Expander::builder()
            .label("Syncthing runs elsewhere or uses another address")
            .child(&box_)
            .build();
        expander.upcast()
    }

    fn show_connected(self: &Rc<Self>, client: Client, o: Overview) {
        let b = &self.sync_body;
        let root = self.app.store.root();

        let folder_group = adw::PreferencesGroup::builder()
            .title("Library folder")
            .description(format!(
                "Connected to Syncthing {} at {}.",
                glib::markup_escape_text(&o.version),
                glib::markup_escape_text(&client.endpoint().base)
            ))
            .build();
        // Rows show paths and names from Syncthing: plain text, not markup.
        let row = adw::ActionRow::builder()
            .title(format!("Folder ID “{}”", syncthing::FOLDER_ID))
            .use_markup(false)
            .build();
        match &o.folder {
            None => {
                row.set_subtitle(&format!(
                    "Not set up yet. Share {} under this ID so your other devices can join it.",
                    root.display()
                ));
                let share = gtk::Button::builder()
                    .label("Share Folder")
                    .valign(gtk::Align::Center)
                    .css_classes(["suggested-action"])
                    .build();
                let (s, ep, id) = (self.clone(), client.endpoint().clone(), o.my_id.clone());
                share.connect_clicked(move |_| {
                    let (root, ep2, id) = (s.app.store.root(), ep.clone(), id.clone());
                    s.run(
                        "Adding the folder to Syncthing…",
                        move || {
                            let c = Client::new(ep2.clone());
                            let r = tunemesh_core::stignore::ensure(&root)
                                .map_err(|e| format!("Could not write .stignore: {e}"))
                                .and_then(|_| {
                                    c.create_folder(&root, &id).map_err(|e| e.to_string())
                                });
                            (r, syncthing::probe(Some(ep2)))
                        },
                        |s, (r, probe)| {
                            if let Err(e) = r {
                                s.toast(&e);
                            }
                            s.show_probe(probe);
                        },
                    );
                });
                row.add_suffix(&share);
            }
            Some(f) if same_dir(Path::new(&f.path), &root) => {
                row.set_subtitle(&format!("Shared from {}", f.path));
                row.add_suffix(&gtk::Image::from_icon_name("object-select-symbolic"));
            }
            Some(f) => {
                row.set_subtitle(&format!(
                    "Syncthing already shares this folder from {}, not from your library folder {}.",
                    f.path,
                    root.display()
                ));
                let use_it = gtk::Button::builder()
                    .label("Use That Folder")
                    .valign(gtk::Align::Center)
                    .build();
                let (s, path) = (self.clone(), PathBuf::from(&f.path));
                use_it.connect_clicked(move |_| {
                    if !path.is_dir() {
                        s.toast(&format!(
                            "{} does not exist on this computer yet",
                            path.display()
                        ));
                        return;
                    }
                    s.app.set_library_path(path.clone());
                    s.app.refresh();
                    s.probe();
                });
                row.add_suffix(&use_it);
            }
        }
        folder_group.add(&row);
        b.append(&folder_group);

        // This device's ID, to enter on the other devices.
        let me = adw::PreferencesGroup::builder()
            .title("This device")
            .description("On your other devices, add this device in Syncthing (scan the QR code with Syncthing on your phone), then accept the TuneMesh folder there.")
            .build();
        let id_row = gtk::Box::builder().spacing(12).build();
        let id_label = gtk::Label::builder()
            .label(&o.my_id)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .selectable(true)
            .xalign(0.0)
            .hexpand(true)
            .css_classes(["monospace"])
            .build();
        let copy = gtk::Button::builder()
            .icon_name("edit-copy-symbolic")
            .tooltip_text("Copy device ID")
            .valign(gtk::Align::Center)
            .build();
        let (s, id) = (self.clone(), o.my_id.clone());
        copy.connect_clicked(move |btn| {
            btn.clipboard().set_text(&id);
            s.toast("Device ID copied");
        });
        id_row.append(&id_label);
        id_row.append(&copy);
        let me_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        me_box.append(&me);
        me_box.append(&id_row);
        if let Some(tex) = qr_texture(&o.my_id, 4) {
            let pic = gtk::Picture::builder()
                .paintable(&tex)
                .can_shrink(false)
                .halign(gtk::Align::Center)
                .css_classes(["tm-qr"])
                .build();
            pic.set_tooltip_text(Some("This device's Syncthing ID"));
            me_box.append(&pic);
        }
        b.append(&me_box);

        // Other devices.
        let others = adw::PreferencesGroup::builder()
            .title("Other devices")
            .description("Add another device by its Syncthing device ID (Actions → Show ID in its Syncthing) to share the library with it.")
            .build();
        for d in &o.shared_with {
            let title = if d.name.is_empty() {
                "Unnamed device".to_owned()
            } else {
                d.name.clone()
            };
            let r = adw::ActionRow::builder()
                .use_markup(false)
                .title(title)
                .subtitle(&d.device_id)
                .subtitle_selectable(true)
                .build();
            r.add_prefix(&gtk::Image::from_icon_name("emblem-shared-symbolic"));
            others.add(&r);
        }
        let new_id = adw::EntryRow::builder().title("Device ID").build();
        let new_name = adw::EntryRow::builder().title("Name (optional)").build();
        others.add(&new_id);
        others.add(&new_name);
        let add = gtk::Button::builder()
            .label("Share With This Device")
            .halign(gtk::Align::Start)
            .css_classes(["pill"])
            .build();
        add.set_sensitive(o.folder.is_some());
        if o.folder.is_none() {
            add.set_tooltip_text(Some("Share the folder first"));
        }
        let (s, ep) = (self.clone(), client.endpoint().clone());
        add.connect_clicked(move |_| {
            let (id, name) = (new_id.text().to_string(), new_name.text().to_string());
            if id.trim().is_empty() {
                s.toast("Enter the other device's ID");
                return;
            }
            let ep = ep.clone();
            s.run(
                "Sharing the folder…",
                move || {
                    let r = Client::new(ep.clone())
                        .share_with(&id, &name)
                        .map_err(|e| e.to_string());
                    (r, syncthing::probe(Some(ep)))
                },
                |s, (r, probe)| {
                    match r {
                        Ok(_) => s.toast("Shared. Accept the folder on the other device."),
                        Err(e) => s.toast(&e),
                    }
                    s.show_probe(probe);
                },
            );
        });
        let others_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        others_box.append(&others);
        others_box.append(&add);
        b.append(&others_box);
    }

    // ---- step 3: downloads ---------------------------------------------------

    fn downloads_page(self: &Rc<Self>) -> adw::NavigationPage {
        let body = gtk::Box::new(gtk::Orientation::Vertical, 18);
        body.append(&heading("Downloads"));
        body.append(&body_text(
            "Any device can ask for songs by link or search text. One device per library does the downloading: it fetches the audio and the songs then sync everywhere. A computer that is often on is a good choice.",
        ));
        let group = adw::PreferencesGroup::new();
        let row = adw::SwitchRow::builder()
            .title("This device downloads songs")
            .subtitle("Runs the TuneMesh downloader while the app is open. Needs ffmpeg; yt-dlp is fetched and kept up to date automatically.")
            .active(self.app.config.borrow().downloader)
            .build();
        group.add(&row);
        body.append(&group);
        // Updates itself when the downloader starts, fails or hands over.
        body.append(&downloads::status_label(&self.app));
        let s = self.clone();
        row.connect_active_notify(move |r| s.app.set_downloader(r.is_active()));
        let next = pill("Continue", true);
        let s = self.clone();
        next.connect_clicked(move |_| s.nav.push(&s.done_page()));
        let skip = pill("Skip", false);
        let s = self.clone();
        skip.connect_clicked(move |_| s.nav.push(&s.done_page()));
        page("downloads", "Downloads", &body, &[&skip, &next])
    }

    // ---- step 4: done ------------------------------------------------------------

    fn done_page(self: &Rc<Self>) -> adw::NavigationPage {
        let body = gtk::Box::new(gtk::Orientation::Vertical, 18);
        body.append(&heading("All set"));
        body.append(&body_text(&format!(
            "Your library is {}. Songs you add there, or that arrive from your other devices, appear in TuneMesh within a second. You can run this assistant again from Settings.",
            self.app.store.root().display()
        )));
        let finish = pill("Start Listening", true);
        let d = self.dialog.clone();
        finish.connect_clicked(move |_| {
            d.close();
        });
        page("done", "Done", &body, &[&finish])
    }
}
