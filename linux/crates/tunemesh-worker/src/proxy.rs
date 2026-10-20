//! The worker's filtering egress proxy.
//!
//! yt-dlp follows redirects, reads manifests, fetches fragments and
//! thumbnails, and an extractor may find further URLs inside a page: none of
//! that can be checked from outside before it happens. So every network
//! connection yt-dlp (and its ffmpeg) and the worker's own HTTP client make
//! goes through this proxy, which listens on 127.0.0.1 on a random port and
//! requires a per-run credential. For each `CONNECT host:port` or plain
//! `GET http://…` it resolves the host once, refuses the connection when any
//! address is not public or belongs to this host (`netguard`), and otherwise
//! connects to the checked address itself.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::{log, netguard};

const MAX_CONNECTIONS: usize = 64;
const MAX_HEAD_BYTES: usize = 32 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// A tunnel with no traffic in either direction for this long is closed.
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);

type Checker = dyn Fn(&str, u16) -> Result<Vec<SocketAddr>, String> + Send + Sync;

pub struct Proxy {
    port: u16,
    user: String,
    password: String,
    refusals: Arc<Mutex<Vec<String>>>,
}

struct Shared {
    expected_auth: String,
    check: Box<Checker>,
    active: AtomicUsize,
    refusals: Arc<Mutex<Vec<String>>>,
}

/// 128 random bits from the kernel, as hex.
fn random_token() -> String {
    let mut buf = [0u8; 16];
    // SAFETY: getrandom writes at most `buf.len()` bytes into `buf`.
    let n = unsafe { libc::getrandom(buf.as_mut_ptr().cast(), buf.len(), 0) };
    assert_eq!(n, buf.len() as isize, "getrandom failed");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn base64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

impl Proxy {
    /// Starts the proxy with the real `netguard` check.
    pub fn start() -> io::Result<Self> {
        Self::start_with(Box::new(netguard::resolve_checked))
    }

    pub fn start_with(check: Box<Checker>) -> io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let user = "tunemesh".to_owned();
        let password = random_token();
        let refusals = Arc::new(Mutex::new(Vec::new()));
        let shared = Arc::new(Shared {
            expected_auth: format!("Basic {}", base64(format!("{user}:{password}").as_bytes())),
            check,
            active: AtomicUsize::new(0),
            refusals: refusals.clone(),
        });
        thread::Builder::new()
            .name("egress-proxy".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    if shared.active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                        shared.active.fetch_sub(1, Ordering::SeqCst);
                        let _ = respond(&stream, 503, "Too many connections");
                        continue;
                    }
                    let shared = shared.clone();
                    let spawned =
                        thread::Builder::new()
                            .name("egress-conn".into())
                            .spawn(move || {
                                let _ = handle(stream, &shared);
                                shared.active.fetch_sub(1, Ordering::SeqCst);
                            });
                    if spawned.is_err() {
                        log!("proxy: could not start a connection thread");
                    }
                }
            })?;
        Ok(Self {
            port,
            user,
            password,
            refusals,
        })
    }

    /// `http://user:password@127.0.0.1:port`, for `--proxy` and the
    /// `http_proxy`/`https_proxy` variables.
    pub fn url(&self) -> String {
        format!(
            "http://{}:{}@127.0.0.1:{}",
            self.user, self.password, self.port
        )
    }

    /// Refusals since the last call, oldest first.
    pub fn take_refusals(&self) -> Vec<String> {
        std::mem::take(&mut *self.refusals.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

fn respond(mut stream: &TcpStream, code: u16, reason: &str) -> io::Result<()> {
    let text = match code {
        403 => "Forbidden",
        407 => "Proxy Authentication Required",
        503 => "Service Unavailable",
        502 => "Bad Gateway",
        _ => "Bad Request",
    };
    let body = format!("{reason}\n");
    let auth = if code == 407 {
        "Proxy-Authenticate: Basic realm=\"tunemesh\"\r\n"
    } else {
        ""
    };
    write!(
        stream,
        "HTTP/1.1 {code} {text}\r\n{auth}Content-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()
}

/// Splits `host:port` (IPv6 literals in brackets) with a default port.
fn split_host_port(authority: &str, default_port: u16) -> Option<(String, u16)> {
    let authority = authority.rsplit('@').next()?;
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (h, after) = rest.split_once(']')?;
        (h, after.strip_prefix(':'))
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        }
    };
    let port = match port {
        Some(p) => p.parse().ok()?,
        None => default_port,
    };
    (!host.is_empty()).then(|| (host.to_owned(), port))
}

struct Head {
    method: String,
    target: String,
    version: String,
    headers: Vec<(String, String)>,
}

/// The lines of a request or response head, without the blank line.
fn read_head_lines(reader: &mut BufReader<TcpStream>) -> io::Result<Option<Vec<String>>> {
    let mut lines = Vec::new();
    let mut total = 0;
    loop {
        let mut line = String::new();
        let n = reader
            .by_ref()
            .take((MAX_HEAD_BYTES - total + 1) as u64)
            .read_line(&mut line)?;
        if n == 0 {
            return Ok(None);
        }
        total += n;
        if total > MAX_HEAD_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request head too large",
            ));
        }
        let line = line.trim_end_matches(['\r', '\n']).to_owned();
        if line.is_empty() {
            break;
        }
        lines.push(line);
    }
    Ok(Some(lines))
}

fn read_head(reader: &mut BufReader<TcpStream>) -> io::Result<Option<Head>> {
    let Some(lines) = read_head_lines(reader)? else {
        return Ok(None);
    };
    let Some(first) = lines.first() else {
        return Ok(None);
    };
    let mut parts = first.split(' ');
    let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad request line",
        ));
    };
    let headers = lines[1..]
        .iter()
        .filter_map(|l| {
            l.split_once(':')
                .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        })
        .collect();
    Ok(Some(Head {
        method: method.to_owned(),
        target: target.to_owned(),
        version: version.to_owned(),
        headers,
    }))
}

fn connect(addrs: &[SocketAddr]) -> io::Result<TcpStream> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, "no address");
    for addr in addrs {
        match TcpStream::connect_timeout(addr, CONNECT_TIMEOUT) {
            Ok(s) => return Ok(s),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Last traffic in either direction of one connection, shared by the
/// threads copying each direction.
struct Activity {
    start: Instant,
    last_ms: AtomicU64,
    idle: Duration,
}

impl Activity {
    fn new(idle: Duration) -> Arc<Self> {
        Arc::new(Self {
            start: Instant::now(),
            last_ms: AtomicU64::new(0),
            idle,
        })
    }

    fn touch(&self) {
        self.last_ms
            .store(self.start.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    fn idle(&self) -> bool {
        let last = Duration::from_millis(self.last_ms.load(Ordering::Relaxed));
        self.start.elapsed().saturating_sub(last) >= self.idle
    }
}

/// Copies `from` to `to` until `from` ends, at most `limit` bytes, or the
/// connection (both directions together) has been idle too long.
fn pump(
    mut from: &TcpStream,
    mut to: &TcpStream,
    activity: &Activity,
    mut limit: Option<u64>,
) -> io::Result<()> {
    from.set_read_timeout(Some(activity.idle.min(Duration::from_secs(1))))?;
    let mut buf = [0u8; 16 * 1024];
    loop {
        if limit == Some(0) {
            return Ok(());
        }
        let want = limit.map_or(buf.len(), |l| buf.len().min(l as usize));
        match from.read(&mut buf[..want]) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                to.write_all(&buf[..n])?;
                activity.touch();
                limit = limit.map(|l| l - n as u64);
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                if activity.idle() {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "idle"));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

/// A CONNECT tunnel: copies both directions until either side closes, or
/// until no traffic has passed in either direction for `idle`.
fn splice(client: TcpStream, upstream: TcpStream, idle: Duration) -> io::Result<()> {
    let activity = Activity::new(idle);
    let (c, u, a) = (client.try_clone()?, upstream.try_clone()?, activity.clone());
    let up = thread::Builder::new()
        .name("egress-up".into())
        .spawn(move || {
            let result = pump(&c, &u, &a, None);
            if result.is_err() {
                let _ = c.shutdown(Shutdown::Both);
                let _ = u.shutdown(Shutdown::Both);
            } else {
                // The client is done sending; the answer may still come.
                let _ = u.shutdown(Shutdown::Write);
            }
        })?;
    let _ = pump(&upstream, &client, &activity, None);
    let _ = client.shutdown(Shutdown::Both);
    let _ = upstream.shutdown(Shutdown::Both);
    let _ = up.join();
    Ok(())
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// One plain-HTTP exchange: the request body (by `Content-Length`) goes
/// upstream, then exactly one response comes back with `Connection: close`.
/// Nothing else the client sends on this connection is forwarded, so a
/// kept-alive client connection can never send a second request to the
/// first request's host.
fn exchange(
    client: TcpStream,
    mut client_reader: BufReader<TcpStream>,
    upstream: TcpStream,
    request_len: u64,
    idle: Duration,
) -> io::Result<()> {
    let activity = Activity::new(idle);
    // Request body: what the head reader already buffered, then the rest.
    let buffered = client_reader.buffer().len() as u64;
    let take = buffered.min(request_len);
    (&upstream).write_all(&client_reader.buffer()[..take as usize])?;
    client_reader.consume(take as usize);
    pump(&client, &upstream, &activity, Some(request_len - take))?;
    // Bytes beyond the body are not forwarded; the client gets
    // `Connection: close` and must open a new connection.
    let watcher = {
        let (c, u) = (client.try_clone()?, upstream.try_clone()?);
        thread::Builder::new()
            .name("egress-watch".into())
            .spawn(move || {
                // Until the client closes (it read the answer, or gave up).
                let mut buf = [0u8; 4096];
                loop {
                    match (&c).read(&mut buf) {
                        Ok(0) => break,
                        Ok(_) => {}
                        Err(e)
                            if matches!(
                                e.kind(),
                                io::ErrorKind::WouldBlock
                                    | io::ErrorKind::TimedOut
                                    | io::ErrorKind::Interrupted
                            ) => {}
                        Err(_) => break,
                    }
                }
                let _ = u.shutdown(Shutdown::Both);
            })?
    };

    upstream.set_read_timeout(Some(idle))?;
    let mut up_reader = BufReader::new(upstream.try_clone()?);
    let result = (|| {
        let Some(lines) = read_head_lines(&mut up_reader)? else {
            return Ok(());
        };
        let Some((status, rest)) = lines.split_first() else {
            return Ok(());
        };
        let headers: Vec<(String, String)> = rest
            .iter()
            .filter_map(|l| {
                l.split_once(':')
                    .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
            })
            .collect();
        let mut out = format!("{status}\r\n");
        for (k, v) in &headers {
            let lower = k.to_ascii_lowercase();
            if lower == "connection" || lower == "keep-alive" || lower.starts_with("proxy-") {
                continue;
            }
            out.push_str(&format!("{k}: {v}\r\n"));
        }
        out.push_str("Connection: close\r\n\r\n");
        (&client).write_all(out.as_bytes())?;
        let chunked = header(&headers, "transfer-encoding").is_some();
        let len = if chunked {
            None
        } else {
            header(&headers, "content-length").and_then(|v| v.parse::<u64>().ok())
        };
        // Body: the buffered part, then the rest up to Content-Length (or EOF).
        let pending = up_reader.buffer().to_vec();
        let first = len.map_or(pending.len() as u64, |l| l.min(pending.len() as u64));
        (&client).write_all(&pending[..first as usize])?;
        activity.touch();
        if len != Some(first) {
            pump(&upstream, &client, &activity, len.map(|l| l - first))?;
        }
        Ok(())
    })();
    let _ = client.shutdown(Shutdown::Both);
    let _ = upstream.shutdown(Shutdown::Both);
    let _ = watcher.join();
    result
}

fn handle(stream: TcpStream, shared: &Shared) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let Some(head) = read_head(&mut reader)? else {
        return Ok(());
    };
    let authorized = head
        .headers
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("proxy-authorization") && *v == shared.expected_auth);
    if !authorized {
        return respond(&stream, 407, "Proxy authentication required");
    }

    let (host, port, origin_target) = if head.method.eq_ignore_ascii_case("CONNECT") {
        match split_host_port(&head.target, 443) {
            Some((h, p)) => (h, p, None),
            None => return respond(&stream, 400, "Bad CONNECT target"),
        }
    } else {
        let lower = head.target.to_ascii_lowercase();
        let Some(rest) = lower.strip_prefix("http://").map(|_| &head.target[7..]) else {
            return respond(
                &stream,
                400,
                "Only http:// targets or CONNECT are supported",
            );
        };
        let (authority, path) = match rest.find(['/', '?']) {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let path = if path.starts_with('?') {
            format!("/{path}")
        } else {
            path.to_owned()
        };
        match split_host_port(authority, 80) {
            Some((h, p)) => (h, p, Some(path)),
            None => return respond(&stream, 400, "Bad request target"),
        }
    };

    let addrs = match (shared.check)(&host, port) {
        Ok(a) => a,
        Err(why) => {
            log!("proxy: refused {host}:{port}: {why}");
            shared
                .refusals
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(why.clone());
            return respond(&stream, 403, &why);
        }
    };
    let mut upstream = match connect(&addrs) {
        Ok(u) => u,
        Err(e) => {
            return respond(
                &stream,
                502,
                &format!("Could not connect to {host}:{port}: {e}"),
            );
        }
    };
    stream.set_read_timeout(None)?;

    match origin_target {
        None => {
            (&stream).write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")?;
            // Bytes the client sent after the head (the TLS hello).
            upstream.write_all(reader.buffer())?;
            splice(stream, upstream, IDLE_TIMEOUT)
        }
        Some(path) => {
            if header(&head.headers, "transfer-encoding").is_some() {
                return respond(&stream, 400, "Chunked request bodies are not supported");
            }
            let body_len = header(&head.headers, "content-length")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            let mut out = format!("{} {path} {}\r\n", head.method, head.version);
            for (k, v) in &head.headers {
                let lower = k.to_ascii_lowercase();
                if lower.starts_with("proxy-") || lower == "connection" || lower == "keep-alive" {
                    continue;
                }
                out.push_str(&format!("{k}: {v}\r\n"));
            }
            // One request per connection: the next one may go elsewhere.
            out.push_str("Connection: close\r\n\r\n");
            upstream.write_all(out.as_bytes())?;
            exchange(stream, reader, upstream, body_len, IDLE_TIMEOUT)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    fn raw(proxy: &Proxy, auth: bool, request: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", proxy.port)).unwrap();
        let header = if auth {
            format!(
                "Proxy-Authorization: Basic {}\r\n",
                base64(format!("{}:{}", proxy.user, proxy.password).as_bytes())
            )
        } else {
            String::new()
        };
        let req = request.replacen("\r\n", &format!("\r\n{header}"), 1);
        s.write_all(req.as_bytes()).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out
    }

    /// An upstream HTTP server on loopback; the test checker treats
    /// `public.test` as a public name that resolves to it.
    fn upstream(response: &'static str) -> SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        thread::spawn(move || {
            for s in l.incoming().flatten() {
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                while r.read_line(&mut line).is_ok_and(|n| n > 2) {
                    line.clear();
                }
                let _ = (&s).write_all(response.as_bytes());
            }
        });
        addr
    }

    fn proxy_with_fake_public(public: SocketAddr, own: Vec<IpAddr>) -> Proxy {
        let own: Vec<netguard::Net> = own.into_iter().map(netguard::Net::host).collect();
        Proxy::start_with(Box::new(move |host: &str, port: u16| {
            netguard::resolve_checked_with(host, port, &own, |h, _| match h {
                // A public-looking name for the loopback test server.
                "public.test" => Ok(vec!["93.184.216.34".parse().unwrap()]),
                "rebind.test" => Ok(vec![
                    "93.184.216.34".parse().unwrap(),
                    "127.0.0.1".parse().unwrap(),
                ]),
                "lan.test" => Ok(vec!["192.168.1.50".parse().unwrap()]),
                _ => Err(io::Error::new(io::ErrorKind::NotFound, "no such host")),
            })
            .map(|addrs| {
                if addrs[0].ip().to_string() == "93.184.216.34" {
                    vec![public]
                } else {
                    addrs
                }
            })
        }))
        .unwrap()
    }

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn requires_the_credential() {
        let p = proxy_with_fake_public(upstream("HTTP/1.1 200 OK\r\n\r\n"), vec![]);
        let out = raw(
            &p,
            false,
            "GET http://public.test/ HTTP/1.1\r\nHost: public.test\r\n\r\n",
        );
        assert!(out.starts_with("HTTP/1.1 407"), "{out}");
    }

    #[test]
    fn forwards_public_and_refuses_private_targets() {
        let up = upstream("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
        // 11.22.33.44 stands in for a public address of this host.
        let p = proxy_with_fake_public(up, vec!["11.22.33.44".parse().unwrap()]);
        let out = raw(
            &p,
            true,
            "GET http://public.test/x HTTP/1.1\r\nHost: public.test\r\n\r\n",
        );
        assert!(
            out.starts_with("HTTP/1.1 200 OK") && out.ends_with("ok"),
            "{out}"
        );

        for target in [
            "GET http://127.0.0.1:8384/rest HTTP/1.1",
            "GET http://[::1]/ HTTP/1.1",
            "GET http://lan.test/snapshot.jpg HTTP/1.1",
            "GET http://rebind.test/ HTTP/1.1",
            "GET http://10.0.0.1/ HTTP/1.1",
            "GET http://100.100.100.100/ HTTP/1.1",
            "GET http://11.22.33.44:8080/ HTTP/1.1",
            "CONNECT 127.0.0.1:443 HTTP/1.1",
            "CONNECT 192.168.1.1:443 HTTP/1.1",
            "CONNECT [fd7a:115c:a1e0::1]:443 HTTP/1.1",
            "CONNECT 11.22.33.44:443 HTTP/1.1",
            "CONNECT localhost:443 HTTP/1.1",
        ] {
            let out = raw(&p, true, &format!("{target}\r\nHost: x\r\n\r\n"));
            assert!(out.starts_with("HTTP/1.1 403"), "{target}: {out}");
        }
        assert_eq!(p.take_refusals().len(), 12);
        assert!(p.take_refusals().is_empty());
    }

    /// The case the old up-front URL check missed: a public page that
    /// redirects to a loopback or LAN address. The client follows the
    /// redirect through the proxy, which refuses the second hop.
    #[test]
    fn refuses_redirect_targets() {
        let up = upstream(
            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:8384/rest/system/config\r\nContent-Length: 0\r\n\r\n",
        );
        let p = proxy_with_fake_public(up, vec![]);
        let first = raw(
            &p,
            true,
            "GET http://public.test/r HTTP/1.1\r\nHost: public.test\r\n\r\n",
        );
        assert!(first.starts_with("HTTP/1.1 302"), "{first}");
        let location = first
            .lines()
            .find_map(|l| l.strip_prefix("Location: "))
            .unwrap();
        let second = raw(
            &p,
            true,
            &format!("GET {location} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"),
        );
        assert!(second.starts_with("HTTP/1.1 403"), "{second}");
    }

    #[test]
    fn tunnels_connect() {
        let up = upstream("HTTP/1.1 204 No Content\r\n\r\n");
        let p = proxy_with_fake_public(up, vec![]);
        let out = raw(
            &p,
            true,
            "CONNECT public.test:443 HTTP/1.1\r\nHost: public.test:443\r\n\r\nGET / HTTP/1.1\r\n\r\n",
        );
        assert!(
            out.starts_with("HTTP/1.1 200 Connection established\r\n\r\nHTTP/1.1 204"),
            "{out}"
        );
    }

    /// Review item 7: a client keeping its proxy connection alive must not
    /// get a second request answered by the first request's host.
    #[test]
    fn plain_http_answers_one_request_per_connection() {
        // An upstream that keeps connections alive and answers every request.
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let up = l.local_addr().unwrap();
        thread::spawn(move || {
            for s in l.incoming().flatten() {
                thread::spawn(move || {
                    let mut r = BufReader::new(s.try_clone().unwrap());
                    loop {
                        let mut line = String::new();
                        let mut blank = false;
                        while r.read_line(&mut line).is_ok_and(|n| n > 0) {
                            if line == "\r\n" {
                                blank = true;
                                break;
                            }
                            line.clear();
                        }
                        if !blank
                            || (&s)
                                .write_all(b"HTTP/1.1 200 OK\r\nConnection: keep-alive\r\nContent-Length: 1\r\n\r\nA")
                                .is_err()
                        {
                            break;
                        }
                    }
                });
            }
        });
        let p = proxy_with_fake_public(up, vec![]);
        let auth = format!(
            "Proxy-Authorization: Basic {}\r\n",
            base64(format!("{}:{}", p.user, p.password).as_bytes())
        );
        let mut s = TcpStream::connect(("127.0.0.1", p.port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let two = format!(
            "GET http://public.test/a HTTP/1.1\r\nHost: public.test\r\n{auth}\r\nGET http://b.test/ HTTP/1.1\r\nHost: b.test\r\n{auth}\r\n"
        );
        s.write_all(two.as_bytes()).unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        assert_eq!(out.matches("HTTP/1.1 200").count(), 1, "{out}");
        assert!(
            out.contains("Connection: close") && !out.contains("keep-alive"),
            "{out}"
        );
        assert!(out.ends_with("\r\n\r\nA"), "{out}");
    }

    /// Review item 6: a tunnel stays open while data flows in one direction
    /// only, and closes once both are idle.
    #[test]
    fn tunnel_idle_timeout_counts_both_directions() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let server = l.local_addr().unwrap();
        thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            // Streams for 2.5 s to a client that sends nothing.
            for _ in 0..10 {
                (&s).write_all(b"x").unwrap();
                thread::sleep(Duration::from_millis(250));
            }
            // Did the proxy half-close the upstream side while streaming?
            s.set_read_timeout(Some(Duration::from_millis(100)))
                .unwrap();
            let mut b = [0u8; 1];
            let eof_early = matches!((&s).read(&mut b), Ok(0));
            tx.send(eof_early).unwrap();
        });
        let (c, proxy_side) = {
            let l2 = TcpListener::bind("127.0.0.1:0").unwrap();
            let c = TcpStream::connect(l2.local_addr().unwrap()).unwrap();
            (c, l2.accept().unwrap().0)
        };
        let upstream = TcpStream::connect(server).unwrap();
        let t = thread::spawn(move || splice(proxy_side, upstream, Duration::from_secs(1)));
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut got = 0;
        let mut b = [0u8; 16];
        let started = Instant::now();
        while let Ok(n) = (&c).read(&mut b) {
            if n == 0 {
                break;
            }
            got += n;
        }
        assert_eq!(got, 10);
        assert!(
            !rx.recv().unwrap(),
            "upstream saw EOF while it was still streaming"
        );
        // Closed about 1 s after the last byte.
        assert!(started.elapsed() < Duration::from_secs(5));
        t.join().unwrap().unwrap();
    }

    #[test]
    fn splits_authorities() {
        assert_eq!(
            split_host_port("example.com:8443", 443),
            Some(("example.com".into(), 8443))
        );
        assert_eq!(
            split_host_port("example.com", 80),
            Some(("example.com".into(), 80))
        );
        assert_eq!(split_host_port("[::1]:443", 80), Some(("::1".into(), 443)));
        assert_eq!(split_host_port("u:p@h:1", 80), Some(("h".into(), 1)));
        assert_eq!(split_host_port(":1", 80), None);
    }
}
