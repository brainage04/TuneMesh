//! Keeps downloads off the worker's own network. Any synced device can
//! write a request, and yt-dlp's generic extractor fetches whatever URL it is
//! given, so an address that is loopback, private, link-local,
//! CGNAT/Tailscale, otherwise non-public, or one of this host's own interface
//! addresses is refused.
//!
//! Request URLs are checked up front for a clear error message; the real
//! enforcement is the egress proxy (`proxy.rs`), which applies the same
//! check to every connection yt-dlp and the worker make, including
//! redirects, manifests, fragments and thumbnails, and connects to the
//! address it checked (so a DNS answer that changes afterwards does not
//! matter). What it cannot see: the host's public IPv4 address behind NAT
//! (reachable through hairpin NAT), which only a firewall can block.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};

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
        return is_public_v4(Ipv4Addr::new(
            (s[6] >> 8) as u8,
            s[6] as u8,
            (s[7] >> 8) as u8,
            s[7] as u8,
        ));
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

/// A network of this host: an interface address and its on-link prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Net {
    pub addr: IpAddr,
    pub prefix: u8,
}

/// Prefixes shorter than these are treated as the single address: an
/// interface configured with a huge or default-route netmask (some VPNs)
/// must not make the whole internet "local".
const MIN_PREFIX_V4: u8 = 8;
const MIN_PREFIX_V6: u8 = 32;

impl Net {
    /// Just this one address.
    #[cfg(test)]
    pub fn host(addr: IpAddr) -> Self {
        let addr = canonical(addr);
        let prefix = if addr.is_ipv4() { 32 } else { 128 };
        Self { addr, prefix }
    }

    fn new(addr: IpAddr, prefix: u8) -> Self {
        let addr = canonical(addr);
        let (min, max) = if addr.is_ipv4() {
            (MIN_PREFIX_V4, 32)
        } else {
            (MIN_PREFIX_V6, 128)
        };
        let prefix = if prefix < min { max } else { prefix.min(max) };
        Self { addr, prefix }
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, canonical(ip)) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = u32::MAX
                    .checked_shl(32 - u32::from(self.prefix))
                    .unwrap_or(0);
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.prefix))
                    .unwrap_or(0);
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

fn prefix_len(bytes: &[u8]) -> u8 {
    bytes.iter().map(|b| b.count_ones() as u8).sum()
}

/// The addresses and on-link prefixes of this host's network interfaces
/// (any scope). Neighbours on these networks may have globally routable
/// addresses (a home router's or NAS's IPv6 address) that a firewall hides
/// from the internet but not from this host.
pub fn own_networks() -> Vec<Net> {
    let mut out = Vec::new();
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `head` with a list we only read and then free.
    unsafe {
        if libc::getifaddrs(&mut head) != 0 {
            return out;
        }
        let mut cur = head;
        while !cur.is_null() {
            let addr = (*cur).ifa_addr;
            let mask = (*cur).ifa_netmask;
            if !addr.is_null() {
                match i32::from((*addr).sa_family) {
                    libc::AF_INET => {
                        let sin = &*(addr as *const libc::sockaddr_in);
                        let ip = IpAddr::V4(Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr)));
                        let prefix = if mask.is_null() {
                            32
                        } else {
                            let m = &*(mask as *const libc::sockaddr_in);
                            prefix_len(&m.sin_addr.s_addr.to_ne_bytes())
                        };
                        out.push(Net::new(ip, prefix));
                    }
                    libc::AF_INET6 => {
                        let sin6 = &*(addr as *const libc::sockaddr_in6);
                        let ip = IpAddr::V6(Ipv6Addr::from(sin6.sin6_addr.s6_addr));
                        let prefix = if mask.is_null() {
                            128
                        } else {
                            let m = &*(mask as *const libc::sockaddr_in6);
                            prefix_len(&m.sin6_addr.s6_addr)
                        };
                        out.push(Net::new(ip, prefix));
                    }
                    _ => {}
                }
            }
            cur = (*cur).ifa_next;
        }
        libc::freeifaddrs(head);
    }
    out
}

/// Normalises IPv4-mapped IPv6 addresses so they compare equal to IPv4.
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

/// Why `ip` may not be reached, if it may not (for the local log only).
pub fn refusal(ip: IpAddr, own: &[Net]) -> Option<&'static str> {
    if !is_public(ip) {
        Some("a local or private network address")
    } else if own.iter().any(|n| n.contains(ip)) {
        Some("on one of this computer's own networks")
    } else {
        None
    }
}

/// The message requests see for every refused or unresolvable host. It is
/// the same in each case and names no address: status files sync to every
/// device, and must not tell them what internal names resolve to.
fn refused_message(host: &str) -> String {
    format!("Refusing {host}: it does not resolve to a public internet address")
}

/// Resolves `host` once and returns its addresses if every one of them may
/// be reached. Callers connect to these addresses, never to the name again.
pub fn resolve_checked(host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
    resolve_checked_with(host, port, &own_networks(), system_resolve)
}

pub fn resolve_checked_with(
    host: &str,
    port: u16,
    own: &[Net],
    resolve: impl Fn(&str, u16) -> io::Result<Vec<IpAddr>>,
) -> Result<Vec<SocketAddr>, String> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let literal = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(&host);
    if host.is_empty() {
        return Err("Empty host name".to_owned());
    }
    let addresses = match literal.parse::<IpAddr>() {
        Ok(ip) => vec![ip],
        Err(_) => match resolve(&host, port) {
            Ok(a) => a,
            Err(e) => {
                crate::log!("netguard: could not resolve {host}: {e}");
                return Err(refused_message(&host));
            }
        },
    };
    if addresses.is_empty() {
        crate::log!("netguard: {host} has no addresses");
        return Err(refused_message(&host));
    }
    for ip in &addresses {
        if let Some(why) = refusal(*ip, own) {
            crate::log!("netguard: refusing {host}: {ip} is {why}");
            return Err(refused_message(&host));
        }
    }
    Ok(addresses
        .into_iter()
        .map(|ip| SocketAddr::new(ip, port))
        .collect())
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
/// addresses that may be reached.
pub fn check_url(url: &str) -> Result<(), String> {
    check_url_with(url, &own_networks(), system_resolve)
}

pub fn check_url_with(
    url: &str,
    own: &[Net],
    resolve: impl Fn(&str, u16) -> io::Result<Vec<IpAddr>>,
) -> Result<(), String> {
    let (host, port) = host_port(url).ok_or_else(|| format!("Not a web link: {url}"))?;
    resolve_checked_with(&host, port, own, resolve).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn classifies_addresses() {
        for public in [
            "1.1.1.1",
            "142.250.70.78",
            "100.63.255.255",
            "100.128.0.1",
            "2a00:1450:4006::200e",
            "64:ff9b::808:808",
        ] {
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
            "100.127.255.254",
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
        assert_eq!(
            host_port("https://www.youtube.com/watch?v=x"),
            Some(("www.youtube.com".into(), 443))
        );
        assert_eq!(
            host_port("HTTP://user:pw@Example.COM.:8080/a"),
            Some(("example.com".into(), 8080))
        );
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
                "laptop.example.ts.net" => Ok(vec![ip("100.64.0.7")]),
                "own.example" => Ok(vec![ip("11.22.33.44")]),
                "rebind.example" => Ok(vec![ip("93.184.216.34"), ip("127.0.0.1")]),
                _ => Err(io::Error::new(io::ErrorKind::NotFound, "no such host")),
            }
        };
        // 11.22.33.44 stands in for a public address of this host.
        let own = [Net::host(ip("11.22.33.44")), Net::host(ip("::1"))];
        assert!(check_url_with("https://www.youtube.com/watch?v=x", &own, resolve).is_ok());
        for refused in [
            "http://laptop.example.ts.net:8384/",
            "http://own.example:8384/",
            "http://[::ffff:11.22.33.44]/",
            "https://rebind.example/a",
            "http://127.0.0.1:8384/rest/system/config",
            "http://[::1]/",
            "http://169.254.169.254/latest/meta-data/",
            "https://nonexistent.invalid/",
            "file:///etc/passwd",
        ] {
            assert!(check_url_with(refused, &own, resolve).is_err(), "{refused}");
        }
    }

    #[test]
    fn lists_own_networks() {
        let own = own_networks();
        assert!(
            own.iter().any(|n| n.addr.is_loopback() && n.prefix == 8),
            "{own:?}"
        );
    }

    /// Review item 1: a LAN neighbour's global IPv6 address (the router at
    /// `<prefix>::1`, a NAS) is refused like the host's own address.
    #[test]
    fn refuses_neighbours_on_own_networks() {
        let own = [
            Net::new(ip("2a02:1234:5678:9a00::10"), 64),
            Net::new(ip("11.22.33.44"), 24),
            // A VPN interface with a default-route netmask counts as itself only.
            Net::new(ip("12.0.0.5"), 0),
        ];
        let resolve = |host: &str, _: u16| -> io::Result<Vec<IpAddr>> {
            match host {
                "nas.example" => Ok(vec![ip("2a02:1234:5678:9a00:211:32ff:fe12:3456")]),
                _ => Ok(vec![ip("142.250.70.78")]),
            }
        };
        for refused in [
            "http://[2a02:1234:5678:9a00::1]/",
            "http://nas.example/",
            "http://11.22.33.1/",
            "http://[::ffff:11.22.33.200]/",
            "http://12.0.0.5/",
        ] {
            assert!(check_url_with(refused, &own, resolve).is_err(), "{refused}");
        }
        for allowed in [
            "http://[2a02:1234:5678:9a01::1]/",
            "http://11.22.34.1/",
            "http://12.0.0.6/",
            "https://www.youtube.com/",
        ] {
            assert!(check_url_with(allowed, &own, resolve).is_ok(), "{allowed}");
        }
    }

    /// Review item 11: the message (it ends up in synced status files) is
    /// the same for private, own and unresolvable hosts and names no address.
    #[test]
    fn refusals_do_not_reveal_addresses() {
        let resolve = |host: &str, _: u16| -> io::Result<Vec<IpAddr>> {
            match host {
                "nas.lan" => Ok(vec![ip("192.168.1.23")]),
                _ => Err(io::Error::new(io::ErrorKind::NotFound, "no such host")),
            }
        };
        let a = check_url_with("http://nas.lan/", &[], resolve).unwrap_err();
        let b = check_url_with("http://nothing.lan/", &[], resolve).unwrap_err();
        assert!(!a.contains("192.168"), "{a}");
        assert_eq!(a.replace("nas.lan", "x"), b.replace("nothing.lan", "x"));
    }
}
