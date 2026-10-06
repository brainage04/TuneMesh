//! `~/.config/tunemesh/config.toml` and the per-device state/cache paths.

use std::fs;
use std::path::PathBuf;

use gtk::glib;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub library_path: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Self { library_path: glib::home_dir().join("Music").join("TuneMesh") }
    }
}

pub fn config_file() -> PathBuf {
    glib::user_config_dir().join("tunemesh").join("config.toml")
}

/// Per-device state (queue, position, volume): never inside the library.
pub fn state_dir() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| glib::home_dir().join(".local").join("state"));
    base.join("tunemesh")
}

pub fn cache_dir() -> PathBuf {
    glib::user_cache_dir().join("tunemesh")
}

/// Expands a leading `~/`.
pub fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => glib::home_dir().join(rest),
        None if path == "~" => glib::home_dir(),
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
                    eprintln!("tunemesh: ignoring invalid {}: {e}", config_file().display());
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

/// Device name shown as `requestedBy`.
pub fn device_name() -> String {
    glib::host_name().to_string()
}
