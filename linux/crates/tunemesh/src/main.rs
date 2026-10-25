//! TuneMesh desktop app: a GTK 4 + libadwaita player for a Syncthing music
//! library, playing through libmpv.

mod app;
mod config;
mod covers;
mod downloader;
mod mpris;
mod mpv;
mod player;
mod store;
mod syncthing;
mod ui;

#[cfg(test)]
mod gui_tests;

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};

use crate::app::{APP_ID, App};
use crate::config::{Config, expand_home};

const USAGE: &str = "usage: tunemesh [--library <folder>] [--setup]

Options:
  --library <folder>  use this library folder for this session (the saved
                      setting in ~/.config/tunemesh/config.toml is unchanged)
  --setup             open the setup assistant
  --version           print the version and exit
  -h, --help          print this help and exit";

fn main() -> glib::ExitCode {
    // GLib, mpv and the helper threads would otherwise each get a malloc
    // arena of their own; two arenas keep resident memory down at no
    // measurable cost for an app this size.
    // SAFETY: called before any other thread exists.
    unsafe { libc::mallopt(libc::M_ARENA_MAX, 2) };
    tunemesh_core::fsx::raise_fd_limit();
    let mut library = None;
    let mut setup = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--library" => match args.next() {
                Some(p) => library = Some(expand_home(&p)),
                None => {
                    eprintln!("{USAGE}");
                    return glib::ExitCode::FAILURE;
                }
            },
            "--setup" => setup = true,
            "--version" => {
                println!("tunemesh {}", env!("CARGO_PKG_VERSION"));
                return glib::ExitCode::SUCCESS;
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return glib::ExitCode::SUCCESS;
            }
            other => {
                eprintln!("tunemesh: unknown argument {other:?}\n{USAGE}");
                return glib::ExitCode::FAILURE;
            }
        }
    }

    gio::resources_register_include!("tunemesh.gresource").expect("register resources");
    let application = adw::Application::builder().application_id(APP_ID).build();
    let instance: Rc<RefCell<Option<Rc<App>>>> = Rc::new(RefCell::new(None));

    application.connect_startup(|_| app::load_style());

    let inst = instance.clone();
    application.connect_activate(move |gtk_app| {
        if let Some(app) = inst.borrow().as_ref() {
            app.present();
            return;
        }
        let mut config = Config::load();
        let first_run = !config.setup_done;
        if let Some(path) = &library {
            config.library_path = path.clone();
        }
        let app = App::new(gtk_app, config);
        mpris::start(&app);
        // Logging out or `systemctl stop` sends SIGTERM: save the queue and
        // position and stop the downloader first, like quitting does.
        for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            let a = app.clone();
            glib_unix::unix_signal_add_local_once(signal, move || a.quit());
        }
        app.present();
        if setup || first_run {
            ui::setup::open(&app);
        }
        *inst.borrow_mut() = Some(app);
    });

    let inst = instance.clone();
    application.connect_shutdown(move |_| {
        // Whatever ended the main loop, never leave the worker behind.
        if let Some(app) = inst.borrow().as_ref() {
            app.downloader.stop_blocking();
        }
    });

    // Arguments were handled above; GApplication must not see them.
    application.run_with_args(&[std::env::args().next().unwrap_or_default()])
}
