//! Keeps request URLs off the worker's own network. Any synced device can
//! write a request, and yt-dlp's generic extractor fetches whatever URL it is
//! given, so a URL whose host is (or resolves to) a loopback, private,
//! link-local, CGNAT/Tailscale or other non-public address is refused before
//! a downloader sees it. The systemd unit's IPAddressDeny covers what this
//! check cannot: redirects and DNS answers that change after the check.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.is_multicast()
        || a == 0
        || (a == 100 && (b & 0xc0) == 64) // 100.64.0.0/10: CGNAT, Tailscale
        || (a == 192 && b == 0 && c == 0) // 192.0.0.0/24: IETF protocol assignments
        || (a == 198 && (b & 0xfe) == 18) // 198.18.0.0/15: benchmarking
        || a >= 240)
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_public_v4(v4);
    }
    let s = ip.segments();
    // 64:ff9b::/96 (NAT64) reaches the embedded IPv4 address.
    if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return is_public_v4(Ipv4Addr::new((s[6] >> 8) as u8, s[6] as u8, (s[7] >> 8) as u8, s[7] as u8));
    }
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (s[0] & 0xfe00) == 0xfc00 // fc00::/7: unique local, Tailscale
        || (s[0] & 0xffc0) == 0xfe80 // fe80::/10: link-local
        || (s[0] & 0xffc0) == 0xfec0 // fec0::/10: site-local
        || (s[0] == 0x2001 && s[1] == 0x0db8)) // documentation
}

/// Whether `ip` is a globally reachable address.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => is_public_v6(v6),
    }
}

/// The host and port of an `http(s)://` URL; IPv6 literals lose their brackets.
fn host_port(url: &str) -> Option<(String, u16)> {
    let (scheme, rest) = url.split_once("://")?;
    let default_port = match scheme.to_ascii_lowercase().as_str() {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit('@').next()?;
    let (host, port) = if let Some(v6) = host_port.strip_prefix('[') {
        let (host, after) = v6.split_once(']')?;
        (host, after.strip_prefix(':'))
    } else {
        match host_port.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (host_port, None),
        }
    };
    let port = match port {
        Some(p) => p.parse().ok()?,
        None => default_port,
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    (!host.is_empty()).then_some((host, port))
}

fn system_resolve(host: &str, port: u16) -> io::Result<Vec<IpAddr>> {
    Ok((host, port).to_socket_addrs()?.map(|a| a.ip()).collect())
}

/// Refuses `url` unless it is an http(s) URL whose host resolves only to
/// public addresses.
pub fn check_url(url: &str) -> Result<(), String> {
    check_url_with(url, system_resolve)
}

pub fn check_url_with(url: &str, resolve: impl Fn(&str, u16) -> io::Result<Vec<IpAddr>>) -> Result<(), String> {
    let (host, port) = host_port(url).ok_or_else(|| format!("Not a web link: {url}"))?;
    let addresses = match host.parse::<IpAddr>() {
        Ok(ip) => vec![ip],
        Err(_) => resolve(&host, port).map_err(|e| format!("Could not resolve {host}: {e}"))?,
    };
    if addresses.is_empty() {
        return Err(format!("Could not resolve {host}"));
    }
    match addresses.iter().find(|ip| !is_public(**ip)) {
        Some(ip) => Err(format!("Refusing {host}: it is a local or private network address ({ip})")),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn classifies_addresses() {
        for public in ["1.1.1.1", "142.250.70.78", "100.63.255.255", "100.128.0.1", "2a00:1450:4006::200e", "64:ff9b::808:808"] {
            assert!(is_public(ip(public)), "{public}");
        }
        for private in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "100.100.100.100",
            "100.66.141.2",
            "0.0.0.0",
            "224.0.0.1",
            "255.255.255.255",
            "198.18.0.1",
            "::1",
            "::",
            "fd7a:115c:a1e0::53",
            "fe80::1",
            "::ffff:127.0.0.1",
            "64:ff9b::7f00:1",
            "2001:db8::1",
        ] {
            assert!(!is_public(ip(private)), "{private}");
        }
    }

    #[test]
    fn parses_hosts() {
        assert_eq!(host_port("https://www.youtube.com/watch?v=x"), Some(("www.youtube.com".into(), 443)));
        assert_eq!(host_port("HTTP://user:pw@Example.COM.:8080/a"), Some(("example.com".into(), 8080)));
        assert_eq!(host_port("http://[::1]:8384/"), Some(("::1".into(), 8384)));
        assert_eq!(host_port("http://[fe80::1]"), Some(("fe80::1".into(), 80)));
        assert_eq!(host_port("ftp://example.com/x"), None);
        assert_eq!(host_port("https:///x"), None);
    }

    #[test]
    fn refuses_private_hosts_and_resolutions() {
        let resolve = |host: &str, _port: u16| -> io::Result<Vec<IpAddr>> {
            match host {
                "www.youtube.com" => Ok(vec![ip("142.250.70.78"), ip("2a00:1450:4006::200e")]),
                "thomas-pc.tail0f6d2e.ts.net" => Ok(vec![ip("100.82.1.2")]),
                "rebind.example" => Ok(vec![ip("93.184.216.34"), ip("127.0.0.1")]),
                _ => Err(io::Error::new(io::ErrorKind::NotFound, "no such host")),
            }
        };
        assert!(check_url_with("https://www.youtube.com/watch?v=x", resolve).is_ok());
        for refused in [
            "http://thomas-pc.tail0f6d2e.ts.net:8384/",
            "https://rebind.example/a",
            "http://127.0.0.1:8384/rest/system/config",
            "http://[::1]/",
            "http://169.254.169.254/latest/meta-data/",
            "https://nonexistent.invalid/",
            "file:///etc/passwd",
        ] {
            assert!(check_url_with(refused, resolve).is_err(), "{refused}");
        }
    }
}
