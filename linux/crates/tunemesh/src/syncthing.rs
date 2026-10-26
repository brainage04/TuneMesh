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
    let env_dir = |name: &str| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
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
    let gui = doc
        .root_element()
        .children()
        .find(|n| n.has_tag_name("gui"))?;
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

/// The host of a base URL (`scheme://[userinfo@]host[:port][/...]`),
/// without brackets around an IPv6 literal.
fn url_host(base: &str) -> Option<&str> {
    let rest = base.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if let Some(v6) = host_port.strip_prefix('[') {
        let (h, after) = v6.split_once(']')?;
        return (after.is_empty() || after.starts_with(':')).then_some(h);
    }
    Some(host_port.split_once(':').map_or(host_port, |(h, _)| h))
}

/// Whether `base` names this machine: a loopback IP literal or exactly
/// `localhost`. Only then is Syncthing's self-signed certificate accepted.
fn is_loopback(base: &str) -> bool {
    match url_host(base) {
        Some(h) => match h.parse::<std::net::IpAddr>() {
            Ok(ip) => ip.is_loopback(),
            Err(_) => h.eq_ignore_ascii_case("localhost"),
        },
        None => false,
    }
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
        let tls = ureq::tls::TlsConfig::builder()
            .disable_verification(is_loopback(&ep.base))
            .build();
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(5)))
            .http_status_as_error(false)
            // The API key travels in a header: never follow a redirect,
            // which could carry it to another origin.
            .max_redirects(0)
            // Never send the key through a proxy from HTTP(S)_PROXY or
            // ALL_PROXY: Syncthing is local, and TLS checks may be off.
            .proxy(None)
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
            ureq::Error::Io(_)
            | ureq::Error::ConnectionFailed
            | ureq::Error::HostNotFound
            | ureq::Error::Timeout(_) => Error::Unreachable(e.to_string()),
            other => Error::Other(other.to_string()),
        })?;
        let status = resp.status().as_u16();
        // A body cut short (connection closed early) is a failure, never an
        // empty success.
        let text = resp.body_mut().read_to_string().map_err(|e| {
            Error::Other(format!(
                "Syncthing's answer to {method} {path} was cut short: {e}"
            ))
        })?;
        match status {
            200..=299 if text.trim().is_empty() => Ok(Some(Value::Null)),
            200..=299 => serde_json::from_str(&text)
                .map(Some)
                .map_err(|e| Error::Other(format!("Unexpected answer to {method} {path}: {e}"))),
            300..=399 => Err(Error::Other(format!(
                "Syncthing answered {status} (a redirect) to {method} {path}; check the address"
            ))),
            // Only a lookup can find nothing; a write that gets 404 failed.
            404 if method == "GET" => Ok(None),
            401 | 403 => Err(Error::Forbidden),
            _ => Err(Error::Other(format!(
                "Syncthing answered {status} to {method} {path}: {}",
                text.trim()
            ))),
        }
    }

    fn get<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<Option<T>, Error> {
        match self.call("GET", path, None)? {
            Some(v) => serde_json::from_value(v)
                .map(Some)
                .map_err(|e| Error::Other(format!("Unexpected answer from {path}: {e}"))),
            None => Ok(None),
        }
    }

    pub fn my_id(&self) -> Result<String, Error> {
        let v: Value = self.get("/rest/system/status")?.unwrap_or_default();
        v["myID"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| Error::Other("Syncthing did not report its device ID".into()))
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
                            .unwrap_or(Device {
                                device_id: d.device_id.clone(),
                                name: String::new(),
                            })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Overview {
            my_id,
            version,
            folder,
            shared_with,
        })
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
        self.call(
            "PUT",
            &format!("/rest/config/folders/{FOLDER_ID}"),
            Some(&body),
        )
        .map(drop)
    }

    /// Checks and normalises a device ID the user typed.
    pub fn check_device_id(&self, id: &str) -> Result<String, Error> {
        let id: String = id.chars().filter(|c| !c.is_whitespace()).collect();
        let q: String = id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        let v: Value = self
            .get(&format!("/rest/svc/deviceid?id={q}"))?
            .unwrap_or_default();
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
            return Err(Error::Other(
                "That is this device's own ID; enter the ID shown on the other device".into(),
            ));
        }
        if self
            .get::<Value>(&format!("/rest/config/devices/{id}"))?
            .is_none()
        {
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
            self.call(
                "PATCH",
                &format!("/rest/config/folders/{FOLDER_ID}"),
                Some(&json!({ "devices": devices })),
            )?;
        }
        Ok(id)
    }
}

/// The result of looking for a running Syncthing.
#[derive(Debug)]
pub enum Probe {
    Connected(Client, Overview),
    /// A configuration exists (or Syncthing is installed) but nothing answers.
    NotRunning {
        installed: bool,
        config: Option<PathBuf>,
        address: Option<String>,
    },
    NotInstalled,
    /// Syncthing runs but its API key could not be read or was refused.
    NeedKey {
        reason: String,
        address: Option<String>,
    },
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("base", &self.ep.base)
            .finish_non_exhaustive()
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
                let Ok(xml) = std::fs::read_to_string(&path) else {
                    continue;
                };
                found_config.get_or_insert_with(|| path.clone());
                if let Some(ep) = endpoint_from_config(&xml) {
                    configs.push((Some(path), ep));
                }
            }
        }
    }
    // Without a connection, report what the most likely configuration says.
    let mut first = None;
    for (path, ep) in configs {
        let address = Some(ep.base.clone());
        let client = Client::new(ep);
        let result = match client.overview() {
            Ok(o) => return Probe::Connected(client, o),
            Err(Error::Unreachable(_)) => Probe::NotRunning {
                installed,
                config: path,
                address,
            },
            Err(e) => Probe::NeedKey {
                reason: e.to_string(),
                address,
            },
        };
        first.get_or_insert(result);
    }
    match first {
        Some(p) => p,
        None if found_config.is_some() => Probe::NeedKey {
            reason: "The Syncthing configuration has no API key this app can read".into(),
            address: None,
        },
        None if installed => Probe::NotRunning {
            installed,
            config: None,
            address: None,
        },
        None => Probe::NotInstalled,
    }
}

/// This device's Syncthing device ID, from the first Syncthing that answers
/// at an address and key in a readable `config.xml`; `None` when none does.
/// Blocks (each try is limited to the client's timeout): call it off the
/// main loop.
#[cfg(not(test))]
pub fn local_device_id() -> Option<String> {
    device_id_from(config_candidates())
}

fn device_id_from(configs: impl IntoIterator<Item = PathBuf>) -> Option<String> {
    configs
        .into_iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .filter_map(|xml| endpoint_from_config(&xml))
        .find_map(|ep| Client::new(ep).my_id().ok())
}

/// Install hints, the user's distribution first (from `/etc/os-release`).
pub fn install_hints() -> Vec<(&'static str, &'static str)> {
    let hints = [
        (
            "debian ubuntu",
            "Debian, Ubuntu, Mint: sudo apt install syncthing",
        ),
        ("fedora", "Fedora: sudo dnf install syncthing"),
        ("arch", "Arch, Manjaro: sudo pacman -S syncthing"),
        ("opensuse suse", "openSUSE: sudo zypper install syncthing"),
        (
            "nixos",
            "NixOS: services.syncthing.enable = true; (or Home Manager's services.syncthing)",
        ),
    ];
    let os = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let ids: Vec<String> = os
        .lines()
        .filter_map(|l| l.strip_prefix("ID=").or_else(|| l.strip_prefix("ID_LIKE=")))
        .flat_map(|v| {
            v.trim_matches('"')
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
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
        assert_eq!(
            endpoint_from_config(&tls).unwrap().base,
            "https://127.0.0.1:18384"
        );
        assert!(
            endpoint_from_config("<configuration><gui><address>x</address></gui></configuration>")
                .is_none()
        );
    }

    #[test]
    fn addresses() {
        assert_eq!(
            normalize_address("[::]:8384", false).unwrap(),
            "http://[::1]:8384"
        );
        assert_eq!(
            normalize_address("https://example.ts.net:8384/", false).unwrap(),
            "https://example.ts.net:8384"
        );
        assert!(normalize_address("unix:///run/st.sock", false).is_none());
        assert!(normalize_address("ftp://x", false).is_none());
        assert!(is_loopback("https://127.0.0.1:8384"));
        assert!(is_loopback("http://[::1]:8384"));
        assert!(is_loopback("http://localhost:8384"));
        assert!(is_loopback("https://LOCALHOST"));
        assert!(!is_loopback("https://127.0.0.1.attacker.invalid"));
        assert!(!is_loopback("https://localhost.attacker.invalid"));
        assert!(!is_loopback("https://127.0.0.1.attacker.invalid:8384/rest"));
        assert!(!is_loopback("https://127.0.0.1@attacker.invalid"));
        assert!(!is_loopback("https://[::1].attacker.invalid"));
        assert!(!is_loopback("https://192.0.2.4:8384"));
    }

    /// Serves one canned HTTP response per connection; counts connections
    /// and records the request headers it saw.
    fn serve(response: String) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                log.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf[..n]).into_owned());
                let _ = s.write_all(response.as_bytes());
            }
        });
        (addr, seen)
    }

    #[test]
    fn redirects_are_not_followed_and_keep_the_key_home() {
        let body = r#"{"myID":"X"}"#;
        let (other, other_seen) = serve(format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ));
        let (first, first_seen) = serve(format!(
            "HTTP/1.1 302 Found\r\nLocation: {other}/rest/system/status\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        ));
        let client = Client::new(Endpoint {
            base: first,
            api_key: "secret-key".into(),
        });
        let r = client.my_id();
        assert!(matches!(r, Err(Error::Other(_))), "{r:?}");
        assert_eq!(first_seen.lock().unwrap().len(), 1);
        assert!(
            first_seen.lock().unwrap()[0]
                .to_ascii_lowercase()
                .contains("x-api-key: secret-key")
        );
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            other_seen.lock().unwrap().is_empty(),
            "the redirect target was contacted"
        );
    }

    #[test]
    fn proxy_settings_from_the_environment_are_ignored() {
        let body = r#"{"myID":"DIRECT"}"#;
        let (proxy, proxy_seen) = serve(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 15\r\nConnection: close\r\n\r\n{\"myID\":\"PROXY\"}"
                .to_string(),
        );
        let (direct, _) = serve(format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ));
        // SAFETY: no other test reads these variables.
        unsafe {
            for k in [
                "HTTP_PROXY",
                "http_proxy",
                "HTTPS_PROXY",
                "https_proxy",
                "ALL_PROXY",
                "all_proxy",
            ] {
                std::env::set_var(k, &proxy);
            }
            std::env::remove_var("NO_PROXY");
            std::env::remove_var("no_proxy");
        }
        let client = Client::new(Endpoint {
            base: direct,
            api_key: "secret-key".into(),
        });
        let id = client.my_id();
        unsafe {
            for k in [
                "HTTP_PROXY",
                "http_proxy",
                "HTTPS_PROXY",
                "https_proxy",
                "ALL_PROXY",
                "all_proxy",
            ] {
                std::env::remove_var(k);
            }
        }
        assert_eq!(id.unwrap(), "DIRECT");
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            proxy_seen.lock().unwrap().is_empty(),
            "the proxy was contacted"
        );
    }

    /// A fake Syncthing answering each request line ("PUT /rest/...") with
    /// the response `route` gives.
    fn fake(route: impl Fn(&str) -> String + Send + 'static) -> Client {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                let mut buf = vec![0u8; 16384];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                let line = req.lines().next().unwrap_or("").to_owned();
                let _ = s.write_all(route(&line).as_bytes());
            }
        });
        Client::new(Endpoint {
            base,
            api_key: "k".into(),
        })
    }

    fn ok(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    const NOT_FOUND: &str =
        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    /// Promises 100 bytes, sends 7 and closes.
    const CUT_SHORT: &str = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\npartial";

    /// GET answers of a normal Syncthing; writes go to `write`.
    fn syncthing_with_writes(write: &'static str) -> Client {
        fake(move |line| {
            let path = line.split(' ').nth(1).unwrap_or("");
            if line.starts_with("PUT ") || line.starts_with("PATCH ") {
                return write.to_owned();
            }
            match path {
                "/rest/system/status" => ok(r#"{"myID":"LOCAL"}"#),
                p if p.starts_with("/rest/svc/deviceid") => ok(r#"{"id":"REMOTE"}"#),
                "/rest/config/folders/tunemesh" => {
                    ok(r#"{"id":"tunemesh","path":"/x","devices":[{"deviceID":"LOCAL"}]}"#)
                }
                _ => NOT_FOUND.to_owned(),
            }
        })
    }

    #[test]
    fn rejected_writes_are_errors() {
        let c = syncthing_with_writes(NOT_FOUND);
        assert!(c.create_folder(Path::new("/x"), "LOCAL").is_err());
        assert!(c.share_with("REMOTE", "phone").is_err());
    }

    #[test]
    fn answers_cut_short_are_errors() {
        let c = syncthing_with_writes(CUT_SHORT);
        assert!(c.create_folder(Path::new("/x"), "LOCAL").is_err());
        assert!(c.share_with("REMOTE", "phone").is_err());
        let c = fake(|_| CUT_SHORT.to_owned());
        assert!(c.my_id().is_err());
    }

    #[test]
    fn empty_successful_writes_still_succeed() {
        let c = syncthing_with_writes(
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        c.create_folder(Path::new("/x"), "LOCAL").unwrap();
        assert_eq!(c.share_with("REMOTE", "phone").unwrap(), "REMOTE");
    }

    /// The device ID comes from the first configured Syncthing that
    /// answers; a missing or dead one is skipped.
    #[test]
    fn device_id_from_the_first_syncthing_that_answers() {
        let live = fake(|_| ok(r#"{"myID":"ABCDEFG-HIJKLMN"}"#));
        let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let dead_addr = dead.local_addr().unwrap();
        drop(dead);
        let dir = std::env::temp_dir().join(format!("tunemesh-stid-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = |name: &str, address: &str| {
            let path = dir.join(name);
            std::fs::write(
                &path,
                format!(
                    "<configuration><gui tls=\"false\"><address>{address}</address><apikey>k</apikey></gui></configuration>"
                ),
            )
            .unwrap();
            path
        };
        let live_addr = live
            .endpoint()
            .base
            .trim_start_matches("http://")
            .to_owned();
        let found = device_id_from([
            dir.join("missing.xml"),
            config("dead.xml", &dead_addr.to_string()),
            config("live.xml", &live_addr),
        ]);
        assert_eq!(found.as_deref(), Some("ABCDEFG-HIJKLMN"));
        assert_eq!(device_id_from([dir.join("dead.xml")]), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
