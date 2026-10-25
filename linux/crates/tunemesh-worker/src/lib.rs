//! The TuneMesh download worker's modules. The `tunemesh-worker` binary
//! (`main.rs`) runs them; the library exists so the fuzz targets in
//! `linux/fuzz` can call the parsers and filters directly. Not a stable API.

pub mod child;
pub mod config;
pub mod ffshim;
pub mod http;
pub mod job;
pub mod leader;
pub mod netguard;
pub mod proxy;
pub mod rundir;
pub mod selftest;
pub mod signals;
pub mod spotify;
pub mod tools;
pub mod ytdlp;

// Defined after the modules, which import it by path (`use crate::log`).
macro_rules! log {
    ($($arg:tt)*) => { eprintln!($($arg)*) };
}
pub(crate) use log;
