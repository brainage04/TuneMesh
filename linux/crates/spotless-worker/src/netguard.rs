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
        // 192.0.0.0/24: IETF protocol assignments. IANA marks the PCP and
        // TURN anycast addresses 192.0.0.9 and .10 globally reachable, but
        // the nearest server (the local router, the ISP's NAT) answers
        // them, so they are refused like their IPv6 twins.
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 88 && c == 99) // 192.88.99.0/24: deprecated 6to4 relay anycast
        || (a == 198 && (b & 0xfe) == 18) // 198.18.0.0/15: benchmarking
        || a >= 240)
}

/// The IPv4 address in the last 32 bits of `ip`.
fn low_v4(ip: Ipv6Addr) -> Ipv4Addr {
    let o = ip.octets();
    Ipv4Addr::new(o[12], o[13], o[14], o[15])
}

/// IPv6 addresses are public unless they are in a block the IANA special
/// purpose registry marks as not globally reachable, or reach a non-public
/// IPv4 address through a translation or tunnel prefix.
fn is_public_v6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();
    let in_net = |net: [u16; 8], prefix: u32| {
        let mask = u128::MAX.checked_shl(128 - prefix).unwrap_or(0);
        u128::from(ip) & mask == u128::from(Ipv6Addr::from(net)) & mask
    };
    // ::ffff:0:0/96 (IPv4-mapped), 64:ff9b::/96 (NAT64) and
    // ::ffff:0:0:0/96 (SIIT) reach the embedded IPv4 address.
    if ip.to_ipv4_mapped().is_some()
        || in_net([0x64, 0xff9b, 0, 0, 0, 0, 0, 0], 96)
        || in_net([0, 0, 0, 0, 0xffff, 0, 0, 0], 96)
    {
        return is_public_v4(low_v4(ip));
    }
    // 2002::/16 (6to4) tunnels to the IPv4 address in bits 16-47.
    if s[0] == 0x2002 {
        let o = ip.octets();
        return is_public_v4(Ipv4Addr::new(o[2], o[3], o[4], o[5]));
    }
    // Globally reachable blocks inside 2001::/23 (IETF protocol
    // assignments), which is otherwise not. Not the PCP, TURN and DNS-SD
    // anycast addresses 2001:1::1 to ::3: like 192.0.0.9 and .10, the
    // nearest server answers them.
    let global_in_2001 = [
        ([0x2001, 3, 0, 0, 0, 0, 0, 0], 32),  // AMT
        ([0x2001, 4, 0x112, 0, 0, 0, 0, 0], 48), // AS112-v6
        ([0x2001, 0x20, 0, 0, 0, 0, 0, 0], 28), // ORCHIDv2
        ([0x2001, 0x30, 0, 0, 0, 0, 0, 0], 28), // drone remote ID
    ];
    if in_net([0x2001, 0, 0, 0, 0, 0, 0, 0], 23) {
        return global_in_2001.iter().any(|(n, p)| in_net(*n, *p));
    }
    let not_global = [
        ([0, 0, 0, 0, 0, 0, 0, 0], 96), // ::/96: unspecified, loopback, IPv4-compatible
        ([0x64, 0xff9b, 1, 0, 0, 0, 0, 0], 48), // local-use NAT64 (RFC 8215)
        ([0x100, 0, 0, 0, 0, 0, 0, 0], 64), // discard-only
        ([0x100, 0, 0, 1, 0, 0, 0, 0], 64), // dummy prefix
        ([0x2001, 0xdb8, 0, 0, 0, 0, 0, 0], 32), // documentation
        ([0x3fff, 0, 0, 0, 0, 0, 0, 0], 20), // documentation
        ([0x5f00, 0, 0, 0, 0, 0, 0, 0], 16), // SRv6 SIDs
        ([0xfc00, 0, 0, 0, 0, 0, 0, 0], 7), // unique local, Tailscale
        ([0xfe80, 0, 0, 0, 0, 0, 0, 0], 10), // link-local
        ([0xfec0, 0, 0, 0, 0, 0, 0, 0], 10), // site-local
        ([0xff00, 0, 0, 0, 0, 0, 0, 0], 8), // multicast
    ];
    !not_global.iter().any(|(n, p)| in_net(*n, *p))
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

    pub fn new(addr: IpAddr, prefix: u8) -> Self {
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
    let mut out = interface_networks();
    let read = |p: &str| std::fs::read_to_string(p).unwrap_or_default();
    out.extend(routes_v4(&read("/proc/net/route")));
    out.extend(routes_v6(&read("/proc/net/ipv6_route")));
    out
}

/// Interface addresses with their netmasks. A global IPv6 address with a
/// /128 mask (DHCPv6) counts with its /64: IPv6 subnets are /64, and the
/// router may announce the prefix without an on-link route.
fn interface_networks() -> Vec<Net> {
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
                        out.push(Net::new(ip, if prefix == 128 { 64 } else { prefix }));
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

/// Normalises IPv6 addresses that reach an IPv4 address (IPv4-mapped
/// `::ffff:0:0/96`, NAT64 `64:ff9b::/96`, SIIT `::ffff:0:0:0/96`, 6to4
/// `2002::/16`) to that address, so they compare equal to it.
pub fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => {
            let s = v6.segments();
            let o = v6.octets();
            if v6.to_ipv4_mapped().is_some()
                || s[..6] == [0x64, 0xff9b, 0, 0, 0, 0]
                || s[..6] == [0, 0, 0, 0, 0xffff, 0]
            {
                IpAddr::V4(low_v4(v6))
            } else if s[0] == 0x2002 {
                IpAddr::V4(Ipv4Addr::new(o[2], o[3], o[4], o[5]))
            } else {
                ip
            }
        }
        v4 => v4,
    }
}

/// `RTF_GATEWAY` in the kernel's route flags.
const RTF_GATEWAY: u32 = 0x2;

/// On-link networks from `/proc/net/route`: routes without a gateway,
/// other than on `lo`.
pub fn routes_v4(table: &str) -> Vec<Net> {
    let mut out = Vec::new();
    for line in table.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        let [iface, dest, gateway, flags, _, _, _, mask, ..] = f[..] else {
            continue;
        };
        let hex = |s: &str| u32::from_str_radix(s, 16).ok();
        let (Some(dest), Some(gateway), Some(flags), Some(mask)) =
            (hex(dest), hex(gateway), hex(flags), hex(mask))
        else {
            continue;
        };
        if iface == "lo" || gateway != 0 || flags & RTF_GATEWAY != 0 {
            continue;
        }
        // The file holds the addresses in host byte order of a big-endian
        // value read as little-endian: the bytes are in memory order.
        let addr = Ipv4Addr::from(dest.to_le_bytes());
        out.push(Net::new(IpAddr::V4(addr), mask.count_ones() as u8));
    }
    out
}

/// On-link networks from `/proc/net/ipv6_route`: routes without a next hop,
/// other than on `lo`.
pub fn routes_v6(table: &str) -> Vec<Net> {
    let mut out = Vec::new();
    for line in table.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        let [dest, prefix, _, _, next_hop, _, _, _, flags, iface] = f[..] else {
            continue;
        };
        let (Ok(dest), Ok(prefix), Ok(next_hop), Ok(flags)) = (
            u128::from_str_radix(dest, 16),
            u8::from_str_radix(prefix, 16),
            u128::from_str_radix(next_hop, 16),
            u32::from_str_radix(flags, 16),
        ) else {
            continue;
        };
        if iface == "lo" || next_hop != 0 || flags & RTF_GATEWAY != 0 {
            continue;
        }
        out.push(Net::new(IpAddr::V6(Ipv6Addr::from(dest)), prefix));
    }
    out
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
pub fn host_port(url: &str) -> Option<(String, u16)> {
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

    /// Routes without a gateway are on-link networks; routes through a
    /// gateway, `lo` and the default route are not.
    #[test]
    fn reads_on_link_routes() {
        let v4 = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
                  eth0\t00000000\t0102A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
                  eth0\t0002A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n\
                  wg0\t000010AC\t00000000\t0001\t0\t0\t0\t0000F0FF\t0\t0\t0\n\
                  eth0\t0000000A\t0102A8C0\t0003\t0\t0\t0\t000000FF\t0\t0\t0\n";
        assert_eq!(
            routes_v4(v4),
            [
                Net::new(ip("192.168.2.0"), 24),
                Net::new(ip("172.16.0.0"), 12)
            ]
        );
        // 2001:db8:1::/64 on eth0 (from a DHCPv6 network whose address is a
        // /128), a gateway route, the default route, and lo.
        let v6 = "\
            20010db8000100000000000000000000 40 00000000000000000000000000000000 00 00000000000000000000000000000000 00000100 00000001 00000000 00000001     eth0
            20010db8000200000000000000000000 30 00000000000000000000000000000000 00 fe800000000000000000000000000001 00000400 00000001 00000000 00000003     eth0
            00000000000000000000000000000000 00 00000000000000000000000000000000 00 fe800000000000000000000000000001 00000400 00000001 00000000 00000003     eth0
            00000000000000000000000000000001 80 00000000000000000000000000000000 00 00000000000000000000000000000000 00000000 00000001 00000000 80200001       lo
            200106c0000000000000000000000000 00 00000000000000000000000000000000 00 00000000000000000000000000000000 00000400 00000001 00000000 00000001     eth0
        ";
        let nets = routes_v6(v6);
        assert_eq!(nets[0], Net::new(ip("2001:db8:1::"), 64));
        // A gateway-less route with too short a prefix counts as one address.
        assert_eq!(nets[1], Net::host(ip("2001:6c0::")));
        assert_eq!(nets.len(), 2);
        // A neighbour on that network is refused; it is not public anyway
        // here (documentation range), so check membership directly.
        assert!(nets[0].contains(ip("2001:db8:1::abcd")));
        assert!(!nets[0].contains(ip("2001:db8:2::1")));
    }

    /// NAT64 addresses reach the embedded IPv4 address, so they match the
    /// host's own IPv4 networks.
    #[test]
    fn nat64_matches_own_ipv4_networks() {
        let own = [Net::new(ip("11.22.33.44"), 24)];
        assert_eq!(canonical(ip("64:ff9b::b16:212c")), ip("11.22.33.44"));
        assert_eq!(
            refusal(ip("64:ff9b::b16:2101"), &own),
            Some("on one of this computer's own networks")
        );
        assert!(own[0].contains(ip("64:ff9b::b16:21fe")));
        assert_eq!(refusal(ip("64:ff9b::808:808"), &own), None);
    }

    /// R1-opus-4: translation and tunnel prefixes, and the IANA blocks
    /// that are not globally reachable.
    #[test]
    fn classifies_special_ipv6_blocks() {
        for refused in [
            "64:ff9b:1::a00:1", // local-use NAT64, private IPv4
            "64:ff9b:1::c0a8:101",
            "64:ff9b:1:abcd::808:808", // local-use NAT64, even to a public IPv4
            "64:ff9b::a00:1",          // well-known NAT64, private IPv4
            "2002:a00:1::1",           // 6to4 to 10.0.0.1
            "2002:7f00:1::1",          // 6to4 to 127.0.0.1
            "::a00:1",                 // IPv4-compatible
            "::808:808",
            "::ffff:0:a00:1", // SIIT, private IPv4
            "100::1",         // discard-only
            "100:0:0:1::1",   // dummy prefix
            "2001::1",        // Teredo
            "2001:2::1",      // benchmarking
            "2001:10::1",     // ORCHID (deprecated)
            "2001:db8::1",
            "3fff::1",
            "5f00::1",
            "fd7a:115c:a1e0::1",
            "fe80::1",
            "ff02::1",
        ] {
            assert!(!is_public(ip(refused)), "{refused}");
        }
        for allowed in [
            "2a00:1450:4001:80b::200e",
            "64:ff9b::808:808", // well-known NAT64 to 8.8.8.8
            "2002:808:808::1",  // 6to4 to 8.8.8.8
            "2001:4:112::1",    // AS112-v6
            "2001:20::1",       // ORCHIDv2
            "2001:3::1", // AMT
        ] {
            assert!(is_public(ip(allowed)), "{allowed}");
        }
        // 6to4 to one of the host's own IPv4 networks.
        let own = [Net::new(ip("11.22.33.44"), 24)];
        assert!(refusal(ip("2002:b16:2101::1"), &own).is_some());
        assert_eq!(canonical(ip("2002:b16:212c::1")), ip("11.22.33.44"));
    }

    /// R4D-2: the IANA special-purpose registries' "globally reachable"
    /// column: 192.88.99.0/24 (deprecated 6to4 relay anycast) is not.
    /// R5-Opus-8: the PCP, TURN and DNS-SD anycast addresses are, but the
    /// nearest server answers them (the local router or the ISP's), so
    /// they are refused in both families.
    #[test]
    fn follows_the_iana_registries() {
        for refused in [
            "192.88.99.1",
            "192.88.99.2",
            "2002:c058:6301::1",
            "192.0.0.9",
            "192.0.0.10",
            "2001:1::1",
            "2001:1::2",
            "2001:1::3",
        ] {
            assert!(!is_public(ip(refused)), "{refused}");
        }
        for allowed in ["192.88.98.1", "192.88.100.1"] {
            assert!(is_public(ip(allowed)), "{allowed}");
        }
    }
}
