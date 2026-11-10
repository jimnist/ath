//! The HTTP transport under the Athletic.net data source.
//!
//! `Fetcher` is a thin seam over "send a [`Request`] (a GET, or a POST with a
//! JSON body) → status + body text" so the data source (URL building,
//! Cloudflare-challenge detection, retry, JSON parsing) can be tested with a
//! stub instead of the network.

use std::fmt;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Serialize;

/// The HTTP methods Athletic.net's API needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    /// Always with a JSON body (e.g. `tfRankings/GetRankings`).
    Post,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
        }
    }
}

/// One request to send. Also what the response cache is keyed by (method, URL
/// and body; never the extra headers).
///
/// The fields are private so a request can only be built through [`Request::get`]
/// or [`Request::post_json`]: a POST always carries its JSON body.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Request {
    method: Method,
    url: String,
    /// The JSON body of a POST, exactly as sent; `None` for a GET.
    body: Option<String>,
    /// Extra headers beyond what every transport sends (`accept`, and
    /// `content-type` on a POST), such as `GetNavInfo`'s `anettokens`. They
    /// carry per-call tokens, not what is asked for, so they are not part of
    /// the cache key and their values are never printed.
    headers: Vec<(String, String)>,
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Header values are tokens: show only their names.
        let names: Vec<&str> = self.headers.iter().map(|(n, _)| n.as_str()).collect();
        f.debug_struct("Request")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("body", &self.body)
            .field("headers", &names)
            .finish()
    }
}

impl Request {
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: Method::Get,
            url: url.into(),
            body: None,
            headers: Vec::new(),
        }
    }

    /// A POST whose body is `body` serialized as JSON.
    pub fn post_json<T: Serialize + ?Sized>(url: impl Into<String>, body: &T) -> Result<Self> {
        Ok(Self {
            method: Method::Post,
            url: url.into(),
            body: Some(serde_json::to_string(body).context("serializing POST body")?),
            headers: Vec::new(),
        })
    }

    /// The same request with one more header; the name is lower-cased.
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers
            .push((name.into().to_ascii_lowercase(), value.into()));
        self
    }

    /// The extra headers, as `(lower-case name, value)` pairs.
    pub fn headers(&self) -> impl Iterator<Item = (&str, &str)> {
        self.headers.iter().map(|(n, v)| (n.as_str(), v.as_str()))
    }

    pub fn method(&self) -> Method {
        self.method
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// The JSON body of a POST, exactly as sent; `None` for a GET.
    pub fn body(&self) -> Option<&str> {
        self.body.as_deref()
    }
}

impl fmt::Display for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.method.as_str(), self.url)
    }
}

/// A raw HTTP response.
#[derive(Debug, Clone)]
pub struct FetchResponse {
    pub status: u16,
    pub body: String,
}

impl FetchResponse {
    /// True if this looks like a Cloudflare bot challenge ("Just a moment...")
    /// rather than a JSON payload.
    ///
    /// A challenge is a Cloudflare signal - a 403/429/503, or the interstitial
    /// marker text - together with a non-JSON body. A plain 404/500 (bad id,
    /// server error) is deliberately NOT a challenge, so it isn't retried or
    /// misreported as an expired cookie; it surfaces as a normal parse error.
    pub fn looks_like_challenge(&self) -> bool {
        let trimmed = self.body.trim_start();
        let json_ish = trimmed.starts_with('{') || trimmed.starts_with('[');
        let cloudflare_signal = matches!(self.status, 403 | 429 | 503)
            || self.body.contains("Just a moment")
            || self.body.contains("challenges.cloudflare");
        cloudflare_signal && !json_ish
    }

    /// True if a successful (2xx) answer carries no JSON: an empty body or an
    /// HTML page. Athletic.net does this when it throttles a burst of requests
    /// (seen live as an empty 200; HANDOFF.md describes HTML instead of JSON),
    /// so it is transient. Error statuses never count: a 404/500 is a real
    /// answer and fails at once.
    pub fn looks_throttled(&self) -> bool {
        let trimmed = self.body.trim_start();
        let json_ish = trimmed.starts_with('{') || trimmed.starts_with('[');
        (200..300).contains(&self.status) && !json_ish
    }
}

/// Sends raw HTTP requests. Abstracted so the data source is testable without a
/// network or real Cloudflare.
#[async_trait]
pub trait Fetcher: Send + Sync {
    async fn fetch(&self, req: &Request) -> Result<FetchResponse>;

    /// What the user should do when Athletic.net keeps answering with a
    /// Cloudflare challenge despite backing off. It depends on the transport:
    /// a cookie can be refreshed, a browser tab has to pass the check itself.
    fn challenge_advice(&self) -> &'static str;
}

/// The cookie transport's [`Fetcher::challenge_advice`].
pub const COOKIE_CHALLENGE_ADVICE: &str = "Your session cookie is likely expired or bound to a \
     different User-Agent/IP - paste a fresh one via --cookie / ATH_COOKIE / --cookie-file (and \
     make sure --user-agent matches the browser it came from).";

/// A default Chrome-like User-Agent. The `cf_clearance` cookie is bound to the
/// UA that solved the challenge, so callers should pass the UA their browser
/// used; this is only the fallback.
pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
     AppleWebKit/537.36 (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36";

/// The real `reqwest` fetcher: sends the session cookie + a matching User-Agent.
pub struct ReqwestFetcher {
    client: reqwest::Client,
    cookie: String,
    user_agent: String,
}

impl ReqwestFetcher {
    pub fn new(cookie: String, user_agent: String) -> Result<Self> {
        let client = reqwest::Client::builder()
            .build()
            .context("building HTTP client")?;
        Ok(Self::with_client(client, cookie, user_agent))
    }

    /// A fetcher over a caller-built client (tests use one that skips any
    /// configured proxy, so their localhost server is reached directly).
    fn with_client(client: reqwest::Client, cookie: String, user_agent: String) -> Self {
        Self {
            client,
            cookie,
            user_agent,
        }
    }
}

#[async_trait]
impl Fetcher for ReqwestFetcher {
    async fn fetch(&self, req: &Request) -> Result<FetchResponse> {
        let builder = match req.method {
            Method::Get => self.client.get(&req.url),
            Method::Post => self
                .client
                .post(&req.url)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(req.body.clone().unwrap_or_default()),
        };
        let builder = req
            .headers()
            .fold(builder, |b, (name, value)| b.header(name, value));
        let resp = builder
            .header(reqwest::header::COOKIE, &self.cookie)
            .header(reqwest::header::USER_AGENT, &self.user_agent)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .with_context(|| req.to_string())?;
        let status = resp.status().as_u16();
        let body = resp
            .text()
            .await
            .with_context(|| format!("reading response body of {req}"))?;
        Ok(FetchResponse { status, body })
    }

    fn challenge_advice(&self) -> &'static str {
        COOKIE_CHALLENGE_ADVICE
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

    use super::*;

    #[test]
    fn throttled_means_a_2xx_without_json() {
        let resp = |status, body: &str| FetchResponse {
            status,
            body: body.to_string(),
        };
        assert!(resp(200, "").looks_throttled());
        assert!(resp(200, "  <html>blocked</html>").looks_throttled());
        assert!(!resp(200, "{\"ok\":true}").looks_throttled());
        assert!(!resp(200, "[]").looks_throttled());
        assert!(!resp(404, "").looks_throttled(), "a 404 is a real answer");
        assert!(!resp(500, "<html>error</html>").looks_throttled());
    }

    /// Serve exactly one HTTP/1.1 exchange on localhost: answer with `status`
    /// and `body`, and hand back the raw request (head and body) as received.
    /// A fetcher that ignores proxy settings, so the tests' localhost server is
    /// reached even where HTTP_PROXY or a system proxy is configured.
    fn local_fetcher(cookie: String, user_agent: String) -> ReqwestFetcher {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        ReqwestFetcher::with_client(client, cookie, user_agent)
    }

    async fn one_shot_server(
        status: &'static str,
        body: &'static str,
    ) -> (String, JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            // Read the head, then as many body bytes as Content-Length says.
            loop {
                let n = sock.read(&mut chunk).await.unwrap();
                buf.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buf).to_string();
                if let Some(end) = text.find("\r\n\r\n") {
                    let len = header(&text, "content-length")
                        .and_then(|v| v.parse::<usize>().ok())
                        .unwrap_or(0);
                    if buf.len() >= end + 4 + len {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let reply = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            sock.write_all(reply.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&buf).to_string()
        });
        (base, handle)
    }

    /// A header's value from a raw HTTP/1.1 message.
    fn header<'a>(raw: &'a str, name: &str) -> Option<&'a str> {
        raw.split("\r\n\r\n").next()?.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case(name).then(|| v.trim())
        })
    }

    #[tokio::test]
    async fn post_sends_json_body_cookie_and_user_agent() {
        let (base, server) = one_shot_server("200 OK", r#"{"ok":true}"#).await;
        let fetcher = local_fetcher("cf_clearance=abc".into(), "TestUA/1.0".into());
        let req = Request::post_json(
            format!("{base}/api/v1/tfRankings/GetRankings"),
            &serde_json::json!({"eventShort": "400m"}),
        )
        .unwrap();

        let resp = fetcher.fetch(&req).await.unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, r#"{"ok":true}"#);
        assert!(!resp.looks_like_challenge());

        let raw = server.await.unwrap();
        assert!(
            raw.starts_with("POST /api/v1/tfRankings/GetRankings HTTP/1.1\r\n"),
            "{raw}"
        );
        assert_eq!(header(&raw, "cookie"), Some("cf_clearance=abc"));
        assert_eq!(header(&raw, "user-agent"), Some("TestUA/1.0"));
        assert_eq!(header(&raw, "content-type"), Some("application/json"));
        assert!(
            raw.ends_with(r#"{"eventShort":"400m"}"#),
            "body sent: {raw}"
        );
    }

    #[tokio::test]
    async fn get_sends_cookie_and_user_agent_without_a_body() {
        let (base, server) = one_shot_server("200 OK", "{}").await;
        let fetcher = local_fetcher("c=1".into(), "TestUA/2.0".into());
        fetcher
            .fetch(&Request::get(format!("{base}/api/v1/x")))
            .await
            .unwrap();
        let raw = server.await.unwrap();
        assert!(raw.starts_with("GET /api/v1/x HTTP/1.1\r\n"), "{raw}");
        assert_eq!(header(&raw, "cookie"), Some("c=1"));
        assert_eq!(header(&raw, "user-agent"), Some("TestUA/2.0"));
        assert_eq!(header(&raw, "content-type"), None);
    }

    #[tokio::test]
    async fn get_sends_extra_headers() {
        let (base, server) = one_shot_server("200 OK", "{}").await;
        let fetcher = local_fetcher("c=1".into(), "UA".into());
        let req = Request::get(format!("{base}/api/v1/nav")).with_header("AnetTokens", "tok");
        fetcher.fetch(&req).await.unwrap();
        let raw = server.await.unwrap();
        assert_eq!(header(&raw, "anettokens"), Some("tok"));
        assert_eq!(header(&raw, "cookie"), Some("c=1"));
    }

    #[test]
    fn printing_a_request_never_shows_header_values() {
        let req = Request::get("https://x/").with_header("anettokens", "secret-token");
        let printed = format!("{req:?} {req}");
        assert!(printed.contains("anettokens"), "{printed}");
        assert!(!printed.contains("secret-token"), "{printed}");
    }

    #[test]
    fn cookie_transport_advises_a_fresh_cookie() {
        let advice = local_fetcher("c=1".into(), "UA".into()).challenge_advice();
        assert!(advice.contains("--cookie"), "{advice}");
    }

    #[tokio::test]
    async fn challenge_on_a_post_is_detected() {
        let (base, server) = one_shot_server(
            "403 Forbidden",
            "<!DOCTYPE html><title>Just a moment...</title>",
        )
        .await;
        let fetcher = local_fetcher("c=1".into(), "UA".into());
        let req = Request::post_json(base, &serde_json::json!({})).unwrap();
        let resp = fetcher.fetch(&req).await.unwrap();
        server.await.unwrap();
        assert_eq!(resp.status, 403);
        assert!(resp.looks_like_challenge());
    }
}
