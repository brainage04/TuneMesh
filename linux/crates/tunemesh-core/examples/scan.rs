//! Prints the tracks `library::scan` finds below a directory, one path per
//! line. Built with the release profile it checks that a file the tag
//! reader panics on does not end the process
//! (`cargo run --release --example scan -- <dir>`).

use std::collections::HashMap;

fn main() {
    let Some(root) = std::env::args_os().nth(1) else {
        eprintln!("usage: scan <library root>");
        std::process::exit(2);
    };
    for track in tunemesh_core::library::scan(root.as_ref(), &HashMap::new()) {
        println!("{}", track.path);
    }
}
