//! `~/.config/tunemesh/config.toml` and the per-device state/cache paths.

use std::fs;
use std::path::PathBuf;

use gtk::glib;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub library_path: PathBuf,
    /// "This device downloads songs": run `tunemesh-worker` as a child.
    #[serde(default)]
    pub downloader: bool,
    /// Whether the setup assistant was finished or skipped. A config file
    /// written before the assistant existed counts as set up.
    #[serde(default = "yes")]
    pub setup_done: bool,
}

fn yes() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            library_path: default_library(),
            downloader: false,
            setup_done: false,
        }
    }
}

pub fn default_library() -> PathBuf {
    real_home().join("Music").join("TuneMesh")
}

/// The user's home folder. Snap points `$HOME` at a per-snap folder and
/// keeps the real one in `$SNAP_REAL_HOME`.
pub fn real_home() -> PathBuf {
    std::env::var_os("SNAP_REAL_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(glib::home_dir)
}

pub fn config_file() -> PathBuf {
    config_base().join("tunemesh").join("config.toml")
}

/// Per-device state (queue, position, volume): never inside the library.
pub fn state_dir() -> PathBuf {
    state_base().join("tunemesh")
}

pub fn cache_dir() -> PathBuf {
    cache_base().join("tunemesh")
}

#[cfg(not(test))]
fn config_base() -> PathBuf {
    glib::user_config_dir()
}

#[cfg(not(test))]
fn state_base() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| glib::home_dir().join(".local").join("state"))
}

#[cfg(not(test))]
fn cache_base() -> PathBuf {
    glib::user_cache_dir()
}

/// Unit tests never touch the developer's real config, state or cache:
/// each test process gets its own folder under the temporary directory.
#[cfg(test)]
fn test_home() -> PathBuf {
    std::env::temp_dir().join(format!("tunemesh-test-home-{}", std::process::id()))
}

#[cfg(test)]
fn config_base() -> PathBuf {
    test_home().join("config")
}

#[cfg(test)]
fn state_base() -> PathBuf {
    test_home().join("state")
}

#[cfg(test)]
fn cache_base() -> PathBuf {
    test_home().join("cache")
}

/// Where the built-in downloader assembles downloads (contract: created 0700).
pub fn worker_dir() -> PathBuf {
    cache_dir().join("worker")
}

/// Expands a leading `~/` to the real home folder.
pub fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => real_home().join(rest),
        None if path == "~" => real_home(),
        None => PathBuf::from(path),
    }
}

impl Config {
    pub fn load() -> Self {
        match fs::read_to_string(config_file()) {
            Ok(text) => match toml::from_str::<Config>(&text) {
                Ok(mut c) => {
                    c.library_path = expand_home(&c.library_path.to_string_lossy());
                    c
                }
                Err(e) => {
                    eprintln!(
                        "tunemesh: ignoring invalid {}: {e}",
                        config_file().display()
                    );
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let text = toml::to_string(self).map_err(std::io::Error::other)?;
        tunemesh_core::atomic::write(&config_file(), text.as_bytes())
    }
}

/// This device's name: shown as `requestedBy`, and the name the worker
/// writes into `.tunemesh/worker.json` (both use the host name).
pub fn device_name() -> String {
    glib::host_name().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_config_counts_as_set_up() {
        let c: Config = toml::from_str("library_path = \"/srv/music\"").unwrap();
        assert!(c.setup_done);
        assert!(!c.downloader);
        assert!(!Config::default().setup_done);
    }

    #[test]
    fn new_keys_round_trip() {
        let c = Config {
            library_path: "/srv/music".into(),
            downloader: true,
            setup_done: true,
        };
        let back: Config = toml::from_str(&toml::to_string(&c).unwrap()).unwrap();
        assert!(back.downloader && back.setup_done);
    }
}
