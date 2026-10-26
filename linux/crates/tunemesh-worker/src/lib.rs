//! The TuneMesh download worker's modules. The `tunemesh-worker` binary
//! (`main.rs`) runs them. With the `fuzzing` feature the parsers and filters
//! are public too, so the fuzz targets in `linux/fuzz` can call them
//! directly; without it only what the binary needs is. Not a stable API.

/// Declares an item `pub` with the `fuzzing` feature (for `linux/fuzz`) and
/// `pub(crate)` without it.
macro_rules! fuzz_pub {
    ($(#[$m:meta])* $kw:ident $($rest:tt)*) => {
        #[cfg(feature = "fuzzing")]
        $(#[$m])* pub $kw $($rest)*
        #[cfg(not(feature = "fuzzing"))]
        $(#[$m])* pub(crate) $kw $($rest)*
    };
}

/// Declares modules public with the `fuzzing` feature, private without it.
macro_rules! fuzz_pub_mod {
    ($($name:ident),*) => {
        $(
            #[cfg(feature = "fuzzing")]
            pub mod $name;
            #[cfg(not(feature = "fuzzing"))]
            mod $name;
        )*
    };
}

pub mod config;
pub mod ffshim;
pub mod http;
pub mod job;
pub mod leader;
pub mod proxy;
pub mod rundir;
pub mod selftest;
pub mod signals;
pub mod tools;
fuzz_pub_mod!(child, netguard, spotify, ytdlp);

// Defined after the modules, which import it by path (`use crate::log`).
macro_rules! log {
    ($($arg:tt)*) => { eprintln!($($arg)*) };
}
pub(crate) use log;
