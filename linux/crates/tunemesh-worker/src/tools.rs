//! Finding, and when needed downloading, the programs the worker runs.
//!
//! - yt-dlp: `--yt-dlp`, else `$TUNEMESH_YTDLP`, else a managed copy at
//!   `$XDG_DATA_HOME/tunemesh/yt-dlp/yt-dlp`: the official standalone release
//!   binary for this CPU, verified against the release's `SHA2-256SUMS`,
//!   checked for updates at most once a day and replaced atomically.
//! - deno (yt-dlp's JavaScript runtime for YouTube): `$TUNEMESH_DENO`, else
//!   `deno` on `PATH`, else a managed copy at
//!   `$XDG_DATA_HOME/tunemesh/deno/deno` from the official release zip,
//!   verified against its `.sha256sum`, checked at most once a week.
//! - ffmpeg and ffprobe: `$TUNEMESH_FFMPEG_DIR`, else `PATH`.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use sha2::{Digest, Sha256};

use crate::config::{self, private_dir};
use crate::http::Http;
use crate::log;

const YTDLP_REFRESH: Duration = Duration::from_secs(24 * 3600);
const DENO_REFRESH: Duration = Duration::from_secs(7 * 24 * 3600);
/// After a failed download with no usable copy, wait this long before trying again.
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(10 * 60);
const MAX_BINARY_BYTES: u64 = 512 * 1024 * 1024;

/// What a job needs to run yt-dlp.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub yt_dlp: PathBuf,
    /// `None`: no JavaScript runtime; YouTube may fail or offer fewer formats.
    pub deno: Option<PathBuf>,
    /// The real programs; yt-dlp reaches them through `ffshim`.
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

pub struct Tools {
    yt_dlp: Option<PathBuf>,
    deno: Option<PathBuf>,
    ffmpeg_dir: Option<PathBuf>,
    data_dir: PathBuf,
    /// Last update check per managed tool in this process.
    checked: HashMap<&'static str, Instant>,
}

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The first `name` on `PATH` that is an executable file.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| is_executable(p))
}

fn is_executable(p: &Path) -> bool {
    fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

fn is_musl() -> bool {
    ["/lib/ld-musl-x86_64.so.1", "/lib/ld-musl-aarch64.so.1"]
        .iter()
        .any(|p| Path::new(p).exists())
}

/// The yt-dlp release asset for this machine.
fn ytdlp_asset() -> Option<&'static str> {
    match (std::env::consts::ARCH, is_musl()) {
        ("x86_64", false) => Some("yt-dlp_linux"),
        ("x86_64", true) => Some("yt-dlp_musllinux"),
        ("aarch64", false) => Some("yt-dlp_linux_aarch64"),
        ("aarch64", true) => Some("yt-dlp_musllinux_aarch64"),
        _ => None,
    }
}

fn deno_asset() -> Option<&'static str> {
    match (std::env::consts::ARCH, is_musl()) {
        ("x86_64", false) => Some("deno-x86_64-unknown-linux-gnu.zip"),
        ("aarch64", false) => Some("deno-aarch64-unknown-linux-gnu.zip"),
        _ => None,
    }
}

/// The tag a GitHub `releases/latest` URL redirects to.
fn latest_tag(http: &Http, repo: &str) -> Result<String, String> {
    let location = http.redirect_location(&format!("https://github.com/{repo}/releases/latest"))?;
    let tag = location
        .rsplit_once("/tag/")
        .map(|(_, t)| t.trim_end_matches('/'))
        .unwrap_or("");
    if tag.is_empty()
        || !tag
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c))
    {
        return Err(format!(
            "unexpected release redirect for {repo}: {location}"
        ));
    }
    Ok(tag.to_owned())
}

/// The checksum for `asset` in a `sha256sum`-style listing.
fn checksum_for(listing: &str, asset: &str) -> Option<String> {
    listing.lines().find_map(|l| {
        let mut parts = l.split_whitespace();
        let (sum, name) = (parts.next()?, parts.next()?);
        (name.trim_start_matches('*') == asset
            && sum.len() == 64
            && sum.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| sum.to_ascii_lowercase())
    })
}

/// A writer that hashes what passes through it.
struct Hashing<W> {
    inner: W,
    hash: Sha256,
}

impl<W: Write> Write for Hashing<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.hash.update(&buf[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A temporary file in `dir` that is removed unless kept.
struct Temp {
    path: PathBuf,
    keep: bool,
}

impl Temp {
    fn new(dir: &Path) -> io::Result<(Self, File)> {
        let path = dir.join(format!(".download-{:08x}", fastrand::u32(..)));
        let file = File::options().write(true).create_new(true).open(&path)?;
        Ok((Self { path, keep: false }, file))
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Downloads `url` into a temporary file in `dir`, checking its SHA-256.
fn fetch_verified(http: &Http, url: &str, sha256: &str, dir: &Path) -> Result<Temp, String> {
    let (temp, file) = Temp::new(dir).map_err(|e| e.to_string())?;
    let mut w = Hashing {
        inner: file,
        hash: Sha256::new(),
    };
    http.download(url, &mut w, MAX_BINARY_BYTES)?;
    let got = hex(&w.hash.finalize());
    if got != sha256 {
        return Err(format!(
            "{url}: checksum mismatch (expected {sha256}, got {got})"
        ));
    }
    w.inner.sync_all().map_err(|e| e.to_string())?;
    Ok(temp)
}

/// Makes `temp` executable and renames it to `dir/name`, recording `version`.
fn install(temp: &mut Temp, dir: &Path, name: &str, version: &str) -> Result<PathBuf, String> {
    fs::set_permissions(&temp.path, fs::Permissions::from_mode(0o755))
        .map_err(|e| e.to_string())?;
    let target = dir.join(name);
    fs::rename(&temp.path, &target).map_err(|e| e.to_string())?;
    temp.keep = true;
    tunemesh_core::atomic::write(&dir.join("version"), version.as_bytes())
        .map_err(|e| e.to_string())?;
    touch(dir);
    Ok(target)
}

fn touch(dir: &Path) {
    let _ = tunemesh_core::atomic::write(&dir.join("checked"), b"");
}

fn checked_recently(dir: &Path, every: Duration) -> bool {
    fs::metadata(dir.join("checked"))
        .and_then(|m| m.modified())
        .is_ok_and(|t| {
            SystemTime::now()
                .duration_since(t)
                .is_ok_and(|age| age < every)
        })
}

/// Reads the one file called `name` out of a zip archive (stored or deflated).
fn unzip_entry(zip: &[u8], name: &str, out: &mut impl Write, limit: u64) -> Result<(), String> {
    let u16_at = |o: usize| {
        zip.get(o..o + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]) as usize)
    };
    let u32_at = |o: usize| {
        zip.get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    };
    let bad = || "not a usable zip archive".to_owned();
    let search_from = zip.len().saturating_sub(22 + 65535);
    let eocd = (search_from..zip.len().saturating_sub(21))
        .rev()
        .find(|&i| u32_at(i) == Some(0x0605_4b50))
        .ok_or_else(bad)?;
    let count = u16_at(eocd + 10).ok_or_else(bad)?;
    let mut p = u32_at(eocd + 16).ok_or_else(bad)?;
    for _ in 0..count {
        if u32_at(p) != Some(0x0201_4b50) {
            return Err(bad());
        }
        let method = u16_at(p + 10).ok_or_else(bad)?;
        let csize = u32_at(p + 20).ok_or_else(bad)?;
        let nlen = u16_at(p + 28).ok_or_else(bad)?;
        let elen = u16_at(p + 30).ok_or_else(bad)?;
        let clen = u16_at(p + 32).ok_or_else(bad)?;
        let local = u32_at(p + 42).ok_or_else(bad)?;
        let entry = zip.get(p + 46..p + 46 + nlen).ok_or_else(bad)?;
        if entry == name.as_bytes() {
            if u32_at(local) != Some(0x0403_4b50) {
                return Err(bad());
            }
            let start = local
                + 30
                + u16_at(local + 26).ok_or_else(bad)?
                + u16_at(local + 28).ok_or_else(bad)?;
            let data = zip.get(start..start + csize).ok_or_else(bad)?;
            let mut reader: Box<dyn Read> = match method {
                0 => Box::new(data),
                8 => Box::new(flate2::read::DeflateDecoder::new(data)),
                m => return Err(format!("unsupported zip compression {m}")),
            };
            let n = io::copy(&mut (&mut reader).take(limit + 1), out).map_err(|e| e.to_string())?;
            if n > limit {
                return Err("zip entry too large".to_owned());
            }
            return Ok(());
        }
        p += 46 + nlen + elen + clen;
    }
    Err(format!("{name} not found in the archive"))
}

impl Tools {
    pub fn new(yt_dlp_flag: Option<PathBuf>) -> Self {
        Self {
            yt_dlp: yt_dlp_flag.or_else(|| env_path("TUNEMESH_YTDLP")),
            deno: env_path("TUNEMESH_DENO"),
            ffmpeg_dir: env_path("TUNEMESH_FFMPEG_DIR"),
            data_dir: config::data_home().join("tunemesh"),
            checked: HashMap::new(),
        }
    }

    /// Whether `tool` was checked in this process within `every`, or a
    /// failed attempt is too recent to retry.
    fn skip_check(&self, tool: &'static str, every: Duration) -> bool {
        self.checked.get(tool).is_some_and(|t| t.elapsed() < every)
    }

    /// The real ffmpeg and ffprobe: `$TUNEMESH_FFMPEG_DIR`, else `PATH`.
    pub fn ffmpeg(&self) -> Result<(PathBuf, PathBuf), String> {
        Ok(match &self.ffmpeg_dir {
            Some(dir) => {
                if !is_executable(&dir.join("ffmpeg")) || !is_executable(&dir.join("ffprobe")) {
                    return Err(format!(
                        "TUNEMESH_FFMPEG_DIR ({}) has no ffmpeg and ffprobe",
                        dir.display()
                    ));
                }
                (dir.join("ffmpeg"), dir.join("ffprobe"))
            }
            None => match (which("ffmpeg"), which("ffprobe")) {
                (Some(a), Some(b)) => (a, b),
                _ => {
                    return Err("ffmpeg and ffprobe are not installed (install ffmpeg or set TUNEMESH_FFMPEG_DIR)".to_owned());
                }
            },
        })
    }

    /// Finds or downloads everything; the error explains what is missing.
    pub fn resolve(&mut self, http: &Http) -> Result<Resolved, String> {
        let (ffmpeg, ffprobe) = self.ffmpeg()?;
        let yt_dlp = match &self.yt_dlp {
            Some(p) if is_executable(p) => p.clone(),
            Some(p) => return Err(format!("yt-dlp at {} is not an executable file", p.display())),
            None => self.managed_ytdlp(http).map_err(|e| {
                format!("yt-dlp is not available: {e}. Install yt-dlp and set TUNEMESH_YTDLP, or check the network")
            })?,
        };
        let deno = match &self.deno {
            Some(p) => Some(p.clone()),
            None => match which("deno") {
                Some(p) => Some(p),
                None => match self.managed_deno(http) {
                    Ok(p) => Some(p),
                    Err(e) => {
                        log!("deno is not available ({e}); YouTube downloads may fail");
                        None
                    }
                },
            },
        };
        Ok(Resolved {
            yt_dlp,
            deno,
            ffmpeg,
            ffprobe,
        })
    }

    fn managed_ytdlp(&mut self, http: &Http) -> Result<PathBuf, String> {
        let dir = self.data_dir.join("yt-dlp");
        let binary = dir.join("yt-dlp");
        let have = is_executable(&binary);
        if have
            && (checked_recently(&dir, YTDLP_REFRESH) || self.skip_check("yt-dlp", YTDLP_REFRESH))
        {
            return Ok(binary);
        }
        if !have && self.skip_check("yt-dlp", RETRY_AFTER_FAILURE) {
            return Err("the last download attempt failed; retrying in a few minutes".to_owned());
        }
        self.checked.insert("yt-dlp", Instant::now());
        let result = (|| {
            let asset = ytdlp_asset().ok_or_else(|| {
                format!(
                    "no standalone yt-dlp release for {} Linux",
                    std::env::consts::ARCH
                )
            })?;
            let dir = private_dir(&dir).map_err(|e| e.to_string())?;
            let tag = latest_tag(http, "yt-dlp/yt-dlp")?;
            let current = fs::read_to_string(dir.join("version")).unwrap_or_default();
            if have && current.trim() == tag {
                touch(&dir);
                return Ok(binary.clone());
            }
            let base = format!("https://github.com/yt-dlp/yt-dlp/releases/download/{tag}");
            let sums = http.text(&format!("{base}/SHA2-256SUMS"), &[], 64 * 1024)?;
            let sum = checksum_for(&sums, asset)
                .ok_or_else(|| format!("{asset} is not listed in SHA2-256SUMS"))?;
            let mut temp = fetch_verified(http, &format!("{base}/{asset}"), &sum, &dir)?;
            let path = install(&mut temp, &dir, "yt-dlp", &tag)?;
            log!("installed yt-dlp {tag} at {}", path.display());
            Ok(path)
        })();
        match result {
            Ok(p) => Ok(p),
            Err(e) if have => {
                log!("could not update yt-dlp ({e}); using the installed copy");
                Ok(binary)
            }
            Err(e) => Err(e),
        }
    }

    fn managed_deno(&mut self, http: &Http) -> Result<PathBuf, String> {
        let dir = self.data_dir.join("deno");
        let binary = dir.join("deno");
        let have = is_executable(&binary);
        if have && (checked_recently(&dir, DENO_REFRESH) || self.skip_check("deno", DENO_REFRESH)) {
            return Ok(binary);
        }
        if !have && self.skip_check("deno", RETRY_AFTER_FAILURE) {
            return Err("the last download attempt failed".to_owned());
        }
        self.checked.insert("deno", Instant::now());
        let result = (|| {
            let asset = deno_asset().ok_or_else(|| {
                format!(
                    "no deno release for {} Linux with this C library",
                    std::env::consts::ARCH
                )
            })?;
            let dir = private_dir(&dir).map_err(|e| e.to_string())?;
            let tag = latest_tag(http, "denoland/deno")?;
            let current = fs::read_to_string(dir.join("version")).unwrap_or_default();
            if have && current.trim() == tag {
                touch(&dir);
                return Ok(binary.clone());
            }
            let base = format!("https://github.com/denoland/deno/releases/download/{tag}");
            let listing = http.text(&format!("{base}/{asset}.sha256sum"), &[], 4096)?;
            let sum =
                checksum_for(&listing, asset).ok_or_else(|| format!("no checksum for {asset}"))?;
            let zip_temp = fetch_verified(http, &format!("{base}/{asset}"), &sum, &dir)?;
            let zip = fs::read(&zip_temp.path).map_err(|e| e.to_string())?;
            let (mut temp, mut file) = Temp::new(&dir).map_err(|e| e.to_string())?;
            unzip_entry(&zip, "deno", &mut file, MAX_BINARY_BYTES)?;
            file.sync_all().map_err(|e| e.to_string())?;
            drop(zip_temp);
            let path = install(&mut temp, &dir, "deno", &tag)?;
            log!("installed deno {tag} at {}", path.display());
            Ok(path)
        })();
        match result {
            Ok(p) => Ok(p),
            Err(e) if have => {
                log!("could not update deno ({e}); using the installed copy");
                Ok(binary)
            }
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_checksums() {
        let listing = "1fa6  yt-dlp\n58162f9bfdc27458ea47bfcb311cf47028f17d8154a8bf7d689861d46399230a  yt-dlp_linux\n\
                       b16e4dab368a816cd05d477d698a605a6ae87ccee1c8ffd38fa21d7254141fcc  yt-dlp_linux_aarch64\n";
        assert_eq!(
            checksum_for(listing, "yt-dlp_linux").as_deref(),
            Some("58162f9bfdc27458ea47bfcb311cf47028f17d8154a8bf7d689861d46399230a")
        );
        assert_eq!(checksum_for(listing, "yt-dlp"), None);
        assert_eq!(checksum_for(listing, "yt-dlp_linux.zip"), None);
    }

    #[test]
    fn unzips_stored_and_deflated_entries() {
        let deflated = make_zip(8);
        let stored = make_zip(0);
        for zip in [deflated, stored] {
            let mut out = Vec::new();
            unzip_entry(&zip, "deno", &mut out, 100).unwrap();
            assert_eq!(out, b"hello deno");
            assert!(unzip_entry(&zip, "other", &mut Vec::new(), 100).is_err());
            assert!(unzip_entry(&zip, "deno", &mut Vec::new(), 3).is_err());
        }
        assert!(unzip_entry(b"not a zip", "deno", &mut Vec::new(), 100).is_err());
    }

    /// A minimal one-entry zip archive.
    fn make_zip(method: u16) -> Vec<u8> {
        let content = b"hello deno";
        let data = if method == 8 {
            let mut e =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            e.write_all(content).unwrap();
            e.finish().unwrap()
        } else {
            content.to_vec()
        };
        let name = b"deno";
        let mut z = Vec::new();
        let local = |z: &mut Vec<u8>| {
            z.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            z.extend_from_slice(&[20, 0, 0, 0]);
            z.extend_from_slice(&method.to_le_bytes());
            z.extend_from_slice(&[0; 8]);
            z.extend_from_slice(&(data.len() as u32).to_le_bytes());
            z.extend_from_slice(&(content.len() as u32).to_le_bytes());
            z.extend_from_slice(&(name.len() as u16).to_le_bytes());
            z.extend_from_slice(&0u16.to_le_bytes());
            z.extend_from_slice(name);
            z.extend_from_slice(&data);
        };
        local(&mut z);
        let cd = z.len();
        z.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        z.extend_from_slice(&[20, 0, 20, 0, 0, 0]);
        z.extend_from_slice(&method.to_le_bytes());
        z.extend_from_slice(&[0; 8]);
        z.extend_from_slice(&(data.len() as u32).to_le_bytes());
        z.extend_from_slice(&(content.len() as u32).to_le_bytes());
        z.extend_from_slice(&(name.len() as u16).to_le_bytes());
        z.extend_from_slice(&[0; 12]);
        z.extend_from_slice(&0u32.to_le_bytes());
        z.extend_from_slice(name);
        let cd_len = z.len() - cd;
        z.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        z.extend_from_slice(&[0; 4]);
        z.extend_from_slice(&1u16.to_le_bytes());
        z.extend_from_slice(&1u16.to_le_bytes());
        z.extend_from_slice(&(cd_len as u32).to_le_bytes());
        z.extend_from_slice(&(cd as u32).to_le_bytes());
        z.extend_from_slice(&0u16.to_le_bytes());
        z
    }
}
