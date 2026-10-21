//! The TuneMesh library format (`docs/library-format.md`): folder layout,
//! track identity and metadata, playlists, download requests and status.

pub mod atomic;
pub mod fsx;
#[cfg(any(test, feature = "hostile"))]
pub mod hostile;
pub mod library;
pub mod limits;
pub mod playlist;
pub mod request;
pub mod sanitize;
pub mod search;
pub mod stignore;
pub mod worker;

pub use library::{CoverSource, Library, Track};
pub use playlist::{LIKED_SONGS, Playlist, PlaylistEntry};
pub use request::{Request, RequestState, Status};

/// Directory below the root holding playlists.
pub const PLAYLISTS_DIR: &str = "Playlists";
/// Directory below the root holding requests and status files.
pub const TUNEMESH_DIR: &str = ".tunemesh";

/// RFC 3339 UTC timestamp with second precision, as used in requests and
/// status files.
pub fn now_rfc3339() -> String {
    humantime::format_rfc3339_seconds(std::time::SystemTime::now()).to_string()
}
