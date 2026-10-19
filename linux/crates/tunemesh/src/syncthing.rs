//! A small client for Syncthing's REST API, used by the setup assistant to
//! share the library folder. All calls block; run them off the main thread.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use crate::config;

/// Folder ID every TuneMesh device uses for the library.
pub const FOLDER_ID: &str = "tunemesh";
pub const FOLDER_LABEL: &str = "TuneMesh";
pub const DEFAULT_ADDRESS: &str = "http://127.0.0.1:8384";

/// Where to reach Syncthing's REST API.
#[derive(Debug, Clone, PartialEq)]
pub struct Endpoint {
    /// `http(s)://host:port`, no trailing slash.
    pub base: String,
    pub api_key: String,
}

/// The `config.xml` files Syncthing may use, most likely first. Syncthing
/// keeps using `~/.config/syncthing` when it already holds a configuration,
/// otherwise it uses `~/.local/state/syncthing`. Inside Flatpak the host's
/// XDG folders come from `HOST_XDG_*`; under Snap `SNAP_REAL_HOME` is the
/// real home folder.
pub fn config_candidates() -> Vec<PathBuf> {
    let home = config::real_home();
    let env_dir = |name: &str| std::env::var_os(name).map(PathBuf::from).filter(|p| p.is_absolute());
    let mut dirs: Vec<PathBuf> = Vec::new();
    for d in [
        env_dir("HOST_XDG_CONFIG_HOME"),
        env_dir("XDG_CONFIG_HOME"),
        Some(home.join(".config")),
        env_dir("HOST_XDG_STATE_HOME"),
        env_dir("XDG_STATE_HOME"),
        Some(home.join(".local").join("state")),
    ]
    .into_iter()
    .flatten()
    {
        let file = d.join("syncthing").join("config.xml");
        if !dirs.contains(&file) {
            dirs.push(file);
        }
    }
    dirs
}

/// Reads the GUI address and API key from a Syncthing `config.xml`.
pub fn endpoint_from_config(xml: &str) -> Option<Endpoint> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let gui = doc.root_element().children().find(|n| n.has_tag_name("gui"))?;
    let text = |tag: &str| {
        gui.children()
            .find(|n| n.has_tag_name(tag))
            .and_then(|n| n.text())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let api_key = text("apikey")?.to_owned();
    let tls = gui.attribute("tls").is_some_and(|v| v == "true");
    let address = text("address").unwrap_or("127.0.0.1:8384");
    let base = normalize_address(address, tls)?;
    Some(Endpoint { base, api_key })
}

/// Turns a GUI address (`127.0.0.1:8384`, `0.0.0.0:8384`, `[::]:8384`,
/// `http://host:port/`) into a base URL. Unix sockets are not supported.
pub fn normalize_address(address: &str, tls: bool) -> Option<String> {
    let address = address.trim().trim_end_matches('/');
    if address.is_empty() || address.starts_with("unix") || address.starts_with('/') {
        return None;
    }
    let (scheme, rest) = match address.split_once("://") {
        Some((s @ ("http" | "https"), r)) => (s, r),
        Some(_) => return None,
        None => (if tls { "https" } else { "http" }, address),
    };
    // Listening on every address: talk to it over loopback.
    let rest = if let Some(port) = rest.strip_prefix("0.0.0.0:") {
        format!("127.0.0.1:{port}")
    } else if let Some(port) = rest.strip_prefix("[::]:") {
        format!("[::1]:{port}")
    } else if let Some(port) = rest.strip_prefix(':') {
        format!("127.0.0.1:{port}")
    } else {
        rest.to_owned()
    };
    Some(format!("{scheme}://{rest}"))
}

fn is_loopback(base: &str) -> bool {
    let host = base.split_once("://").map_or(base, |(_, r)| r);
    host.starts_with("127.") || host.starts_with("localhost") || host.starts_with("[::1]")
}

#[derive(Debug)]
pub enum Error {
    /// Nothing answers at the address.
    Unreachable(String),
    /// Syncthing answered but refused the API key.
    Forbidden,
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unreachable(e) => write!(f, "Syncthing is not reachable: {e}"),
            Error::Forbidden => write!(f, "Syncthing refused the API key"),
            Error::Other(e) => f.write_str(e),
        }
    }
}

pub struct Client {
    ep: Endpoint,
    agent: ureq::Agent,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderDevice {
    #[serde(rename = "deviceID")]
    pub device_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    pub path: String,
    #[serde(default)]
    pub devices: Vec<FolderDevice>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Device {
    #[serde(rename = "deviceID")]
    pub device_id: String,
    #[serde(default)]
    pub name: String,
}

/// What the setup assistant shows once connected.
#[derive(Debug, Clone)]
pub struct Overview {
    pub my_id: String,
    pub version: String,
    pub folder: Option<Folder>,
    /// Other devices the TuneMesh folder is shared with, with their names.
    pub shared_with: Vec<Device>,
}

impl Client {
    pub fn new(ep: Endpoint) -> Self {
        // Syncthing's GUI certificate is self-signed; skip verification only
        // when talking to this machine.
        let tls = ureq::tls::TlsConfig::builder().disable_verification(is_loopback(&ep.base)).build();
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(5)))
            .http_status_as_error(false)
            .tls_config(tls)
            .build()
            .into();
        Self { ep, agent }
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.ep
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.ep.base)
    }

    /// Sends a request; `Ok(None)` for 404.
    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Option<Value>, Error> {
        let url = self.url(path);
        let key = self.ep.api_key.as_str();
        let result = match (method, body) {
            ("GET", _) => self.agent.get(&url).header("X-API-Key", key).call(),
            ("PUT", Some(b)) => self.agent.put(&url).header("X-API-Key", key).send_json(b),
            ("PATCH", Some(b)) => self.agent.patch(&url).header("X-API-Key", key).send_json(b),
            _ => unreachable!("unsupported request {method} {path}"),
        };
        let mut resp = result.map_err(|e| match e {
            ureq::Error::Io(_) | ureq::Error::ConnectionFailed | ureq::Error::HostNotFound | ureq::Error::Timeout(_) => {
                Error::Unreachable(e.to_string())
            }
            other => Error::Other(other.to_string()),
        })?;
        let status = resp.status().as_u16();
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        match status {
            200..=299 => Ok(Some(if text.trim().is_empty() { Value::Null } else { serde_json::from_str(&text).unwrap_or(Value::Null) })),
            404 => Ok(None),
            401 | 403 => Err(Error::Forbidden),
            _ => Err(Error::Other(format!("Syncthing answered {status} to {method} {path}: {}", text.trim()))),
        }
    }

    fn get<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<Option<T>, Error> {
        match self.call("GET", path, None)? {
            Some(v) => serde_json::from_value(v).map(Some).map_err(|e| Error::Other(format!("Unexpected answer from {path}: {e}"))),
            None => Ok(None),
        }
    }

    pub fn my_id(&self) -> Result<String, Error> {
        let v: Value = self.get("/rest/system/status")?.unwrap_or_default();
        v["myID"].as_str().map(str::to_owned).ok_or_else(|| Error::Other("Syncthing did not report its device ID".into()))
    }

    pub fn overview(&self) -> Result<Overview, Error> {
        let my_id = self.my_id()?;
        let v: Value = self.get("/rest/system/version")?.unwrap_or_default();
        let version = v["version"].as_str().unwrap_or_default().to_owned();
        let folder: Option<Folder> = self.get(&format!("/rest/config/folders/{FOLDER_ID}"))?;
        let devices: Vec<Device> = self.get("/rest/config/devices")?.unwrap_or_default();
        let shared_with = folder
            .as_ref()
            .map(|f| {
                f.devices
                    .iter()
                    .filter(|d| d.device_id != my_id)
                    .map(|d| {
                        devices
                            .iter()
                            .find(|x| x.device_id == d.device_id)
                            .cloned()
                            .unwrap_or(Device { device_id: d.device_id.clone(), name: String::new() })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Overview { my_id, version, folder, shared_with })
    }

    /// Adds the `tunemesh` folder at `path`, shared with no other device yet.
    pub fn create_folder(&self, path: &Path, my_id: &str) -> Result<(), Error> {
        let body = json!({
            "id": FOLDER_ID,
            "label": FOLDER_LABEL,
            "path": path.to_string_lossy(),
            "type": "sendreceive",
            "devices": [{ "deviceID": my_id }],
        });
        self.call("PUT", &format!("/rest/config/folders/{FOLDER_ID}"), Some(&body)).map(drop)
    }

    /// Checks and normalises a device ID the user typed.
    pub fn check_device_id(&self, id: &str) -> Result<String, Error> {
        let id: String = id.chars().filter(|c| !c.is_whitespace()).collect();
        let q: String = id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
        let v: Value = self.get(&format!("/rest/svc/deviceid?id={q}"))?.unwrap_or_default();
        match (v["id"].as_str(), v["error"].as_str()) {
            (Some(norm), _) => Ok(norm.to_owned()),
            (_, Some(e)) => Err(Error::Other(format!("That is not a valid device ID ({e})"))),
            _ => Err(Error::Other("That is not a valid device ID".into())),
        }
    }

    /// Adds `device` (if new) and shares the `tunemesh` folder with it.
    pub fn share_with(&self, device_id: &str, name: &str) -> Result<String, Error> {
        let id = self.check_device_id(device_id)?;
        if id == self.my_id()? {
            return Err(Error::Other("That is this device's own ID; enter the ID shown on the other device".into()));
        }
        if self.get::<Value>(&format!("/rest/config/devices/{id}"))?.is_none() {
            let mut body = json!({ "deviceID": id });
            if !name.trim().is_empty() {
                body["name"] = json!(name.trim());
            }
            self.call("PUT", &format!("/rest/config/devices/{id}"), Some(&body))?;
        }
        let folder: Value = self
            .get(&format!("/rest/config/folders/{FOLDER_ID}"))?
            .ok_or_else(|| Error::Other("Syncthing has no TuneMesh folder yet".into()))?;
        let mut devices = folder["devices"].as_array().cloned().unwrap_or_default();
        if !devices.iter().any(|d| d["deviceID"] == json!(id)) {
            devices.push(json!({ "deviceID": id }));
            self.call("PATCH", &format!("/rest/config/folders/{FOLDER_ID}"), Some(&json!({ "devices": devices })))?;
        }
        Ok(id)
    }
}

/// The result of looking for a running Syncthing.
#[derive(Debug)]
pub enum Probe {
    Connected(Client, Overview),
    /// A configuration exists (or Syncthing is installed) but nothing answers.
    NotRunning { installed: bool, config: Option<PathBuf> },
    NotInstalled,
    /// Syncthing runs but its API key could not be read or was refused.
    NeedKey { reason: String },
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").field("base", &self.ep.base).finish_non_exhaustive()
    }
}

/// Finds Syncthing: `manual` (address and key pasted by the user) if given,
/// else every readable `config.xml` in turn.
pub fn probe(manual: Option<Endpoint>) -> Probe {
    let installed = gtk::glib::find_program_in_path("syncthing").is_some();
    let mut configs: Vec<(Option<PathBuf>, Endpoint)> = Vec::new();
    let mut found_config = None;
    match manual {
        Some(ep) => configs.push((None, ep)),
        None => {
            for path in config_candidates() {
                let Ok(xml) = std::fs::read_to_string(&path) else { continue };
                found_config.get_or_insert_with(|| path.clone());
                if let Some(ep) = endpoint_from_config(&xml) {
                    configs.push((Some(path), ep));
                }
            }
        }
    }
    let mut last = None;
    for (path, ep) in configs {
        let client = Client::new(ep);
        match client.overview() {
            Ok(o) => return Probe::Connected(client, o),
            Err(Error::Unreachable(_)) => last = Some(Probe::NotRunning { installed, config: path }),
            Err(e) => last = Some(Probe::NeedKey { reason: e.to_string() }),
        }
    }
    match last {
        Some(p) => p,
        None if found_config.is_some() => Probe::NeedKey { reason: "The Syncthing configuration has no API key this app can read.".into() },
        None if installed => Probe::NotRunning { installed, config: None },
        None => Probe::NotInstalled,
    }
}

/// Writes `<root>/.stignore` with the TuneMesh rules when it is missing
/// (contract: "`.stignore`").
pub fn ensure_stignore(root: &Path) -> std::io::Result<()> {
    let path = root.join(".stignore");
    if std::fs::symlink_metadata(&path).is_ok() {
        return Ok(());
    }
    tunemesh_core::atomic::write(&path, b"// TuneMesh\n(?d).~tunemesh-*\n")
}

/// Install hints, the user's distribution first (from `/etc/os-release`).
pub fn install_hints() -> Vec<(&'static str, &'static str)> {
    let hints = [
        ("debian ubuntu", "Debian, Ubuntu, Mint: sudo apt install syncthing"),
        ("fedora", "Fedora: sudo dnf install syncthing"),
        ("arch", "Arch, Manjaro: sudo pacman -S syncthing"),
        ("opensuse suse", "openSUSE: sudo zypper install syncthing"),
        ("nixos", "NixOS: services.syncthing.enable = true; (or Home Manager's services.syncthing)"),
    ];
    let os = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let ids: Vec<String> = os
        .lines()
        .filter_map(|l| l.strip_prefix("ID=").or_else(|| l.strip_prefix("ID_LIKE=")))
        .flat_map(|v| v.trim_matches('"').split_whitespace().map(str::to_owned).collect::<Vec<_>>())
        .collect();
    let mut v: Vec<(&str, &str)> = hints.to_vec();
    v.sort_by_key(|(keys, _)| !keys.split(' ').any(|k| ids.iter().any(|i| i == k)));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_gui_settings() {
        let xml = r#"<configuration version="37">
  <folder id="x" path="/tmp"></folder>
  <gui enabled="true" tls="false" debugging="false">
    <address>0.0.0.0:18384</address>
    <apikey>abc123</apikey>
    <theme>default</theme>
  </gui>
</configuration>"#;
        let ep = endpoint_from_config(xml).unwrap();
        assert_eq!(ep.base, "http://127.0.0.1:18384");
        assert_eq!(ep.api_key, "abc123");
        let tls = xml.replace(r#"tls="false""#, r#"tls="true""#);
        assert_eq!(endpoint_from_config(&tls).unwrap().base, "https://127.0.0.1:18384");
        assert!(endpoint_from_config("<configuration><gui><address>x</address></gui></configuration>").is_none());
    }

    #[test]
    fn addresses() {
        assert_eq!(normalize_address("[::]:8384", false).unwrap(), "http://[::1]:8384");
        assert_eq!(normalize_address("https://example.ts.net:8384/", false).unwrap(), "https://example.ts.net:8384");
        assert!(normalize_address("unix:///run/st.sock", false).is_none());
        assert!(normalize_address("ftp://x", false).is_none());
        assert!(is_loopback("https://127.0.0.1:8384"));
        assert!(!is_loopback("https://192.0.2.4:8384"));
    }
}
