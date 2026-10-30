//! The worker's own HTTP client (Spotify metadata, yt-dlp and deno
//! downloads). Every request goes through the egress proxy, so it gets the
//! same address checks as yt-dlp, redirects included.

use std::io::{self, Read, Write};
use std::time::Duration;

use ureq::Agent;
use ureq::http::Response;

pub const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";

#[derive(Clone)]
pub struct Http {
    agent: Agent,
    /// Same proxy, but redirects are returned instead of followed.
    no_redirect: Agent,
}

fn agent(proxy_url: &str, redirects: u32) -> Result<Agent, String> {
    let proxy = ureq::Proxy::new(proxy_url).map_err(|e| format!("proxy: {e}"))?;
    let config = Agent::config_builder()
        .proxy(Some(proxy))
        .max_redirects(redirects)
        .max_redirects_will_error(false)
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .timeout_recv_body(Some(Duration::from_secs(600)))
        .user_agent(USER_AGENT)
        .build();
    Ok(Agent::new_with_config(config))
}

pub type Body = Response<ureq::Body>;

fn status_error(url: &str, resp: &Body) -> String {
    format!("{url}: HTTP {}", resp.status().as_u16())
}

impl Http {
    pub fn new(proxy_url: &str) -> Result<Self, String> {
        Ok(Self {
            agent: agent(proxy_url, 5)?,
            no_redirect: agent(proxy_url, 0)?,
        })
    }

    /// GET with extra headers; fails on non-2xx.
    pub fn get(&self, url: &str, headers: &[(&str, &str)]) -> Result<Body, String> {
        let mut req = self.agent.get(url);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let resp = req.call().map_err(|e| format!("{url}: {e}"))?;
        if !resp.status().is_success() {
            return Err(status_error(url, &resp));
        }
        Ok(resp)
    }

    /// GET returning the body as text, at most `limit` bytes.
    pub fn text(&self, url: &str, headers: &[(&str, &str)], limit: u64) -> Result<String, String> {
        self.get(url, headers)?
            .into_body()
            .with_config()
            .limit(limit)
            .read_to_string()
            .map_err(|e| format!("{url}: {e}"))
    }

    /// GET returning the body as text whatever the status (APIs that
    /// explain errors in the body).
    pub fn text_any_status(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        limit: u64,
    ) -> Result<String, String> {
        let mut req = self.agent.get(url);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let resp = req.call().map_err(|e| format!("{url}: {e}"))?;
        let status = resp.status().as_u16();
        let text = resp
            .into_body()
            .with_config()
            .limit(limit)
            .read_to_string()
            .map_err(|e| format!("{url}: {e}"))?;
        if status == 429 {
            return Err(format!("{url}: rate limited (HTTP 429); try again later").to_owned());
        }
        Ok(text)
    }

    /// The `Location` of a redirect response, without following it.
    pub fn redirect_location(&self, url: &str) -> Result<String, String> {
        let resp = self
            .no_redirect
            .get(url)
            .call()
            .map_err(|e| format!("{url}: {e}"))?;
        if !resp.status().is_redirection() {
            return Err(status_error(url, &resp));
        }
        resp.headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
            .ok_or_else(|| format!("{url}: redirect without a location"))
    }

    /// Streams the body of `url` into `out`, refusing more than `limit`
    /// bytes; returns the byte count.
    pub fn download(&self, url: &str, out: &mut impl Write, limit: u64) -> Result<u64, String> {
        let resp = self.get(url, &[])?;
        let mut reader = resp.into_body().into_reader().take(limit + 1);
        let n = io::copy(&mut reader, out).map_err(|e| format!("{url}: {e}"))?;
        if n > limit {
            return Err(format!("{url}: larger than {limit} bytes"));
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::Proxy;

    /// The worker's own client goes through the proxy, which refuses
    /// loopback, so even a direct request for a local service fails.
    #[test]
    fn own_requests_are_checked_by_the_proxy() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let proxy = Proxy::start().unwrap();
        let http = Http::new(&proxy.url()).unwrap();
        let err = http
            .text(&format!("http://127.0.0.1:{port}/"), &[], 1000)
            .unwrap_err();
        assert!(err.contains("403"), "{err}");
        let err = http
            .text(&format!("https://127.0.0.1:{port}/"), &[], 1000)
            .unwrap_err();
        assert!(!err.is_empty());
        assert_eq!(proxy.take_refusals().len(), 2);
    }
}
