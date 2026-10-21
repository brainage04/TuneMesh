//! Generates a hostile library (`tunemesh_core::hostile`) and optionally
//! checks how a client copes with it:
//!
//! ```text
//! cargo run --release -p tunemesh-core --features hostile --example hostile_library -- \
//!     <dir> [--small] [--album-tracks N] [--files N] [--statuses N] [--requests N] \
//!     [--playlists N] [--small-playlists N] [--no-huge] [--check]
//! ```
//!
//! `<dir>` must not exist yet. `--check` then scans it, builds the library,
//! summarises the playlists and lists the downloads, printing times and the
//! peak resident memory. The full library takes about 1.2 GB of disk.

use std::path::PathBuf;
use std::process::ExitCode;

use tunemesh_core::hostile::{self, Options};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut dir = None;
    let mut o = Options::full();
    let mut check = false;
    while let Some(a) = args.next() {
        let mut num = |what: &str| -> usize {
            args.next()
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| panic!("{what} needs a number"))
        };
        match a.as_str() {
            "--small" => o = Options::small(),
            "--album-tracks" => o.album_tracks = num(&a),
            "--files" => o.files = num(&a),
            "--statuses" => o.statuses = num(&a),
            "--requests" => o.requests = num(&a),
            "--playlists" => o.big_playlists = num(&a),
            "--small-playlists" => o.small_playlists = num(&a),
            "--no-huge" => o.huge = false,
            "--check" => check = true,
            "-h" | "--help" => {
                println!(
                    "usage: hostile_library <dir> [--small] [--album-tracks N] [--files N] [--statuses N] [--requests N] [--playlists N] [--small-playlists N] [--no-huge] [--check]"
                );
                return ExitCode::SUCCESS;
            }
            _ if dir.is_none() && !a.starts_with('-') => dir = Some(PathBuf::from(a)),
            _ => {
                eprintln!("hostile_library: unknown argument {a:?}");
                return ExitCode::from(2);
            }
        }
    }
    let Some(dir) = dir else {
        eprintln!("hostile_library: give the directory to create");
        return ExitCode::from(2);
    };
    if dir.exists() {
        eprintln!("hostile_library: {} already exists", dir.display());
        return ExitCode::from(2);
    }
    let start = std::time::Instant::now();
    if let Err(e) = hostile::generate(&dir, &o) {
        eprintln!("hostile_library: {e}");
        return ExitCode::FAILURE;
    }
    println!(
        "generated {} in {:.1?} ({o:?})",
        dir.display(),
        start.elapsed()
    );
    if check {
        let r = hostile::check(&dir);
        println!(
            "check: {} tracks, {} albums, {} playlists ({} too large), {} downloads listed\n\
             scan {:.2?}, library {:.2?}, playlists {:.2?}, downloads {:.2?}; peak RSS {} MiB",
            r.tracks,
            r.albums,
            r.playlists,
            r.too_large,
            r.downloads,
            r.scan,
            r.library,
            r.playlists_time,
            r.downloads_time,
            r.peak_rss_kib / 1024
        );
    }
    ExitCode::SUCCESS
}
