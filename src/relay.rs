//! The browser relay transport (ADR 0006), the default way `ath` reaches
//! Athletic.net.
//!
//! Cloudflare blocks plain HTTP clients, so `ath` hands each request to a real
//! Athletic.net tab instead. [`RelayFetcher`] serves a tiny HTTP API on
//! `127.0.0.1`, and the tab (started from the bookmarklet `ath relay install`
//! prints, source in `relay.js`) runs a few loops, each of which:
//!
//! 1. `GET /next?tab=<tab id>&loop=<n>` - a long poll. `200` with a job
//!    `{id, method, url, headers, body}`, `204` when nothing arrived within the
//!    poll window, or `200 {"standby": true}` when another tab is serving.
//! 2. Performs the job with `fetch` in the page (same origin, the browser's own
//!    session), and
//! 3. `POST /result` with `{id, status, body}`, or `{id, error}` when the
//!    browser could not perform it at all.
//!
//! Only one tab serves at a time (the "active" tab); a second tab running the
//! loop waits on standby and takes over once the active tab goes quiet. A job
//! the tab took but lost (its loop polled again without answering, or the tab
//! was replaced) is handed out once more; requests are idempotent reads.
//!
//! Security: the server binds `127.0.0.1` only and answers only requests whose
//! `Origin` is exactly `https://www.athletic.net` (anything else, including no
//! `Origin`, gets a bare 403), so no other web page can read jobs or post
//! results. `ath` only hands out `https://www.athletic.net/` URLs, and the tab
//! refuses any URL outside its own origin. No credential leaves the browser.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::header::{self, HeaderValue};
use hyper::service::service_fn;
use hyper::{Method as HttpMethod, StatusCode};
use hyper_util::rt::TokioIo;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::{Notify, oneshot};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::{Instant, timeout, timeout_at};

use crate::http::{FetchResponse, Fetcher, Method, Request};

/// The only origin the relay talks to, and the only one it fetches for.
pub const ATHLETIC_ORIGIN: &str = "https://www.athletic.net";

/// The default `--relay-port`.
pub const DEFAULT_PORT: u16 = 8767;

/// The default `--relay-timeout`.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long `GET /next` waits for a job before answering 204.
const POLL_WAIT: Duration = Duration::from_secs(10);

/// How long a handed-out request may take before `ath` gives up on it.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(60);

/// How long the active tab must be quiet (no poll waiting, no poll or result
/// for this long) before a standby tab takes over.
const TAKEOVER_QUIET: Duration = Duration::from_secs(5);

/// How often a waiting fetch rechecks the relay's state.
const TICK: Duration = Duration::from_millis(100);

/// How many times a job is handed out before a loss fails it.
const MAX_HANDOUTS: u32 = 2;

/// The largest `/result` body accepted (a whole Profile is well under this).
const MAX_RESULT_BYTES: usize = 64 << 20;

/// The tab's loop. `__ATH_RELAY__` is replaced with the relay's base URL.
const RELAY_JS: &str = include_str!("relay.js");
const RELAY_PLACEHOLDER: &str = "__ATH_RELAY__";

/// What `encodeURIComponent` leaves alone; everything else is escaped.
const URI_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

/// What `ath` tells the user when Athletic.net keeps challenging the relay tab.
pub const RELAY_CHALLENGE_ADVICE: &str = "Athletic.net is challenging the browser tab that runs \
     the ath relay. Switch to that tab, reload it and pass any \"verify you are human\" check, \
     click the ath relay bookmarklet again, and re-run. If it keeps happening you are probably \
     rate limited: wait a few minutes first.";

/// How the relay listens and how long it waits for the tab.
#[derive(Debug, Clone)]
pub struct RelayConfig {
    /// Port on 127.0.0.1; 0 picks a free one (tests).
    pub port: u16,
    /// How long the tab may be silent while requests wait for it (it neither
    /// polls nor holds a request) before the relay gives up on it.
    pub connect_timeout: Duration,
    /// How long `GET /next` waits before answering 204.
    pub poll_wait: Duration,
    /// How long a handed-out request may take to come back.
    pub answer_timeout: Duration,
    /// How long the active tab must be quiet before a standby tab takes over.
    pub takeover_quiet: Duration,
    /// Print "waiting for the tab" / "tab connected" to stderr.
    pub announce: bool,
}

impl RelayConfig {
    pub fn new(port: u16, connect_timeout: Duration) -> Self {
        Self {
            port,
            connect_timeout,
            poll_wait: POLL_WAIT,
            answer_timeout: ANSWER_TIMEOUT,
            takeover_quiet: TAKEOVER_QUIET,
            announce: true,
        }
    }
}

/// The browser relay [`Fetcher`]: serves the relay API on 127.0.0.1 for as long
/// as it lives, and sends each request through the connected tab. It does not
/// wait for a tab until a request needs one, so a run the cache serves needs no
/// tab at all.
pub struct RelayFetcher {
    shared: Arc<Shared>,
    server: JoinHandle<()>,
}

impl RelayFetcher {
    /// Start listening on `127.0.0.1:<port>`.
    pub async fn bind(config: RelayConfig) -> Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, config.port))
            .await
            .with_context(|| {
                format!(
                    "starting the browser relay on 127.0.0.1:{} (is another ath already \
                     running? Wait for it, or pass a cookie to use the cookie transport)",
                    config.port
                )
            })?;
        let addr = listener.local_addr().context("reading the relay address")?;
        let shared = Arc::new(Shared {
            state: Mutex::default(),
            job_ready: Notify::new(),
            config,
            addr,
        });
        let server = tokio::spawn(serve(listener, shared.clone()));
        Ok(Self { shared, server })
    }

    /// The address the relay listens on.
    pub fn local_addr(&self) -> SocketAddr {
        self.shared.addr
    }
}

impl Drop for RelayFetcher {
    fn drop(&mut self) {
        // Aborting the server drops its connection set, which aborts every
        // connection task, parked long polls included.
        self.server.abort();
    }
}

#[async_trait]
impl Fetcher for RelayFetcher {
    async fn fetch(&self, req: &Request) -> Result<FetchResponse> {
        // Defense in depth: the tab refuses other origins too.
        if !req.url().starts_with(&format!("{ATHLETIC_ORIGIN}/")) {
            bail!("the browser relay only fetches {ATHLETIC_ORIGIN} URLs, not {req}");
        }
        let (id, mut reply) = self.shared.enqueue(req);
        // Whatever happens next (an answer, a timeout, this future dropped), the
        // job leaves the relay with it.
        let _job = JobGuard {
            shared: &self.shared,
            id,
        };
        self.shared.job_ready.notify_one();

        loop {
            match self.shared.check(id) {
                Check::Waiting => {}
                Check::NoTab { ever_seen } => bail!(self.shared.no_tab_message(ever_seen)),
                Check::NeverAnswered => bail!(
                    "the Athletic.net tab running the ath relay took {req} but did not answer \
                     within {}s (was it closed or reloaded?)",
                    self.shared.config.answer_timeout.as_secs_f64()
                ),
            }
            match timeout(TICK, &mut reply).await {
                Ok(Ok(result)) => return result.into_response(req),
                Ok(Err(_)) => bail!("the browser relay dropped {req}"),
                Err(_) => {}
            }
        }
    }

    fn challenge_advice(&self) -> &'static str {
        RELAY_CHALLENGE_ADVICE
    }
}

/// The tab's loop as a script for the DevTools console, talking to
/// `127.0.0.1:<port>`.
pub fn console_script(port: u16) -> String {
    RELAY_JS.replace(RELAY_PLACEHOLDER, &format!("http://127.0.0.1:{port}"))
}

/// The same loop as a `javascript:` bookmarklet. The header comment and
/// indentation are dropped to keep it short; line breaks are kept (as `%0A`) so
/// the script reads exactly as written.
pub fn bookmarklet(port: u16) -> String {
    let script = console_script(port);
    let body = match script.split_once("*/\n") {
        Some((header, rest)) if header.starts_with("/*") => rest,
        _ => &script,
    };
    let compact: Vec<&str> = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    format!(
        "javascript:{}",
        utf8_percent_encode(&compact.join("\n"), URI_COMPONENT)
    )
}

/// The command that prints the bookmarklet for `port`.
pub fn install_command(port: u16) -> String {
    if port == DEFAULT_PORT {
        "ath relay install".to_string()
    } else {
        format!("ath --relay-port {port} relay install")
    }
}

/// The one-time setup steps. With `bookmarklet_below` they point at the
/// bookmarklet printed after them (`ath relay install`); otherwise they say
/// which command prints it.
pub fn setup_steps(port: u16, bookmarklet_below: bool) -> String {
    let install = install_command(port);
    let get_it = if bookmarklet_below {
        "Add a bookmark (any name, e.g. \"ath relay\") whose URL is the bookmarklet below"
            .to_string()
    } else {
        format!("Run `{install}` and add a bookmark whose URL is the bookmarklet it prints")
    };
    format!(
        "ath reaches Athletic.net through a browser tab (one-time setup per browser session):\n\
         \x20 1. {get_it} (the line starting with javascript:).\n\
         \x20 2. Open {ATHLETIC_ORIGIN} in Chrome and click that bookmark.\n\
         \x20    The first time, Chrome asks for permission for www.athletic.net to reach apps \
         and services on this device (wording varies by version): click Allow.\n\
         \x20 3. Leave the tab open. It serves every ath command until it is reloaded or the \
         browser restarts; then click the bookmark again.\n\
         No bookmark? Paste the output of `{install} --console` into that tab's DevTools console \
         instead.\n\
         To skip the relay, pass a session cookie (--cookie, --cookie-file or ATH_COOKIE).\n"
    )
}

/// State shared between the fetcher and the relay's HTTP handlers.
struct Shared {
    state: Mutex<State>,
    /// Signalled when a job is queued (or queued again).
    job_ready: Notify,
    config: RelayConfig,
    addr: SocketAddr,
}

#[derive(Default)]
struct State {
    next_id: u64,
    /// Jobs not yet handed out, oldest first. May hold ids of jobs already
    /// gone (their fetch gave up); those are skipped.
    queue: VecDeque<u64>,
    /// Every job still waiting for an answer, handed out or not.
    jobs: HashMap<u64, Job>,
    /// When any tab last polled or posted a result.
    last_seen: Option<Instant>,
    /// When a request first waited with no tab ever seen.
    waiting_since: Option<Instant>,
    /// Set once the tab was silent for the connect timeout with requests
    /// waiting: later requests fail at once, until a tab polls again.
    lost: bool,
    /// The one tab jobs are handed to.
    active: Option<ActiveTab>,
    /// Long polls currently waiting, per tab id.
    held: HashMap<String, usize>,
    announced_wait: bool,
    announced_connected: bool,
}

struct ActiveTab {
    id: String,
    /// When it last polled or posted a result.
    seen: Instant,
}

struct Job {
    wire: WireJob,
    /// The tab id and loop number it was last handed to.
    holder: Option<(String, u32)>,
    handed_out: Option<Instant>,
    handouts: u32,
    reply: Option<oneshot::Sender<TabResult>>,
}

/// A job as the tab receives it from `GET /next`.
#[derive(Debug, Clone, Serialize)]
struct WireJob {
    id: u64,
    method: &'static str,
    url: String,
    headers: BTreeMap<String, String>,
    body: Option<String>,
}

/// What the tab posts to `/result`.
#[derive(Debug, Deserialize)]
struct TabResult {
    id: u64,
    #[serde(default)]
    status: Option<u16>,
    #[serde(default)]
    body: Option<String>,
    /// Set when the browser could not perform the request at all.
    #[serde(default)]
    error: Option<String>,
}

impl TabResult {
    fn into_response(self, req: &Request) -> Result<FetchResponse> {
        if let Some(error) = self.error {
            bail!("the browser tab could not fetch {req}: {error}");
        }
        let status = self
            .status
            .ok_or_else(|| anyhow!("the browser tab answered {req} without a status"))?;
        // A missing body is a tab fault, never an empty answer from Athletic.net.
        let body = self
            .body
            .ok_or_else(|| anyhow!("the browser tab answered {req} without a body"))?;
        Ok(FetchResponse { status, body })
    }
}

enum Check {
    Waiting,
    /// No tab is there to take the job.
    NoTab {
        ever_seen: bool,
    },
    /// The tab took the job but never answered.
    NeverAnswered,
}

/// What a poll gets.
enum Take {
    Job(WireJob),
    Nothing,
    Standby,
}

impl State {
    /// Whether a tab is plausibly there: it has a poll waiting, holds a request
    /// it is still within time to answer, or was heard from recently.
    fn tab_alive(&self, now: Instant, config: &RelayConfig) -> bool {
        self.held.values().any(|&n| n > 0)
            || self.jobs.values().any(|j| {
                j.handed_out
                    .is_some_and(|t| now.saturating_duration_since(t) < config.answer_timeout)
            })
            || self
                .last_seen
                .is_some_and(|t| now.saturating_duration_since(t) < config.connect_timeout)
    }

    /// Put every handed-out job `lost` matches back at the front of the queue,
    /// or fail it if it was already handed out [`MAX_HANDOUTS`] times. Returns
    /// how many were queued again.
    fn requeue(&mut self, lost: impl Fn(&(String, u32)) -> bool) -> usize {
        let mut ids: Vec<u64> = self
            .jobs
            .iter()
            .filter(|(_, j)| j.handed_out.is_some() && j.holder.as_ref().is_some_and(&lost))
            .map(|(&id, _)| id)
            .collect();
        ids.sort_unstable();
        let mut requeued = 0;
        for id in ids.into_iter().rev() {
            let job = self.jobs.get_mut(&id).expect("listed above");
            if job.handouts >= MAX_HANDOUTS {
                let mut job = self.jobs.remove(&id).expect("listed above");
                if let Some(tx) = job.reply.take() {
                    let _ = tx.send(TabResult {
                        id,
                        status: None,
                        body: None,
                        error: Some(format!(
                            "the tab lost it {MAX_HANDOUTS} times (was it reloaded or closed?)"
                        )),
                    });
                }
            } else {
                job.holder = None;
                job.handed_out = None;
                self.queue.push_front(id);
                requeued += 1;
            }
        }
        requeued
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // A panic while holding the lock can't leave the state inconsistent in
        // a way that matters more than failing the run would.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn enqueue(&self, req: &Request) -> (u64, oneshot::Receiver<TabResult>) {
        let (tx, rx) = oneshot::channel();
        let mut headers = BTreeMap::from([("accept".to_string(), "application/json".to_string())]);
        if req.method() == Method::Post {
            headers.insert("content-type".into(), "application/json".into());
        }
        for (name, value) in req.headers() {
            headers.insert(name.into(), value.into());
        }
        let mut st = self.lock();
        st.next_id += 1;
        let id = st.next_id;
        let wire = WireJob {
            id,
            method: req.method().as_str(),
            url: req.url().to_string(),
            headers,
            body: req.body().map(str::to_string),
        };
        st.jobs.insert(
            id,
            Job {
                wire,
                holder: None,
                handed_out: None,
                handouts: 0,
                reply: Some(tx),
            },
        );
        st.queue.push_back(id);
        if st.last_seen.is_none() {
            st.waiting_since.get_or_insert_with(Instant::now);
            if self.config.announce && !std::mem::replace(&mut st.announced_wait, true) {
                crate::progress::message(&format!(
                    "ath: fetching through the browser relay on {}; waiting up to {}s for the \
                     Athletic.net tab (`{}` sets it up)",
                    self.addr,
                    self.config.connect_timeout.as_secs_f64(),
                    install_command(self.addr.port())
                ));
            }
        }
        (id, rx)
    }

    /// Where the fetch of job `id` stands.
    fn check(&self, id: u64) -> Check {
        let config = &self.config;
        let now = Instant::now();
        let mut st = self.lock();
        let Some(job) = st.jobs.get(&id) else {
            // Answered (or failed) and removed; the reply is in the channel.
            return Check::Waiting;
        };
        if let Some(at) = job.handed_out {
            return if now.saturating_duration_since(at) >= config.answer_timeout {
                Check::NeverAnswered
            } else {
                Check::Waiting
            };
        }
        let ever_seen = st.last_seen.is_some();
        if st.lost {
            return Check::NoTab { ever_seen };
        }
        if st.tab_alive(now, config) {
            return Check::Waiting;
        }
        let quiet_since = st.last_seen.or(st.waiting_since).unwrap_or(now);
        if now.saturating_duration_since(quiet_since) >= config.connect_timeout {
            st.lost = true;
            Check::NoTab { ever_seen }
        } else {
            Check::Waiting
        }
    }

    fn no_tab_message(&self, ever_seen: bool) -> String {
        let secs = self.config.connect_timeout.as_secs_f64();
        let what = if ever_seen {
            format!(
                "the Athletic.net tab running the ath relay on {} stopped answering (nothing \
                 from it for {secs}s).",
                self.addr
            )
        } else {
            format!(
                "no Athletic.net browser tab connected to the ath relay on {} within {secs}s.",
                self.addr
            )
        };
        format!("{what}\n\n{}", setup_steps(self.addr.port(), false))
    }

    /// A poll from loop `lp` of tab `tab` arrived: note the contact, recover
    /// jobs that loop or a replaced tab lost, and decide whether this tab
    /// serves. Counts the poll as held when it does.
    fn register_poll(&self, tab: &str, lp: u32) -> bool {
        let now = Instant::now();
        let takeover_quiet = self.config.takeover_quiet;
        let mut st = self.lock();
        st.last_seen = Some(now);
        st.lost = false;
        if self.config.announce && !std::mem::replace(&mut st.announced_connected, true) {
            crate::progress::message("ath: Athletic.net tab connected to the browser relay");
        }
        // A loop polls only when it holds no job, so anything still handed to
        // it never arrived or was dropped.
        let mut requeued = st.requeue(|h| h.0 == tab && h.1 == lp);
        let active = match &st.active {
            None => None,
            Some(a) if a.id == tab => None,
            Some(a) => Some((
                a.id.clone(),
                st.held.get(&a.id).copied().unwrap_or(0) == 0
                    && now.saturating_duration_since(a.seen) >= takeover_quiet,
            )),
        };
        let serves = match active {
            Some((_, false)) => false,
            Some((old, true)) => {
                // The active tab went quiet (reloaded or closed): this one takes
                // over, and gets what the old one never answered.
                requeued += st.requeue(|h| h.0 == old);
                true
            }
            None => true,
        };
        if serves {
            st.active = Some(ActiveTab {
                id: tab.to_string(),
                seen: now,
            });
            *st.held.entry(tab.to_string()).or_default() += 1;
        }
        drop(st);
        for _ in 0..requeued {
            self.job_ready.notify_one();
        }
        serves
    }

    fn release_poll(&self, tab: &str) {
        let mut st = self.lock();
        if let Some(n) = st.held.get_mut(tab) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                st.held.remove(tab);
            }
        }
    }

    /// The oldest queued job for loop `lp` of tab `tab`, if that tab still serves.
    fn take_next(&self, tab: &str, lp: u32) -> Take {
        let mut st = self.lock();
        if st.active.as_ref().is_none_or(|a| a.id != tab) {
            return Take::Standby;
        }
        while let Some(id) = st.queue.pop_front() {
            if let Some(job) = st.jobs.get_mut(&id) {
                job.holder = Some((tab.to_string(), lp));
                job.handed_out = Some(Instant::now());
                job.handouts += 1;
                return Take::Job(job.wire.clone());
            }
        }
        Take::Nothing
    }

    /// Deliver a result; false if no fetch is waiting for it.
    fn deliver(&self, result: TabResult) -> bool {
        let now = Instant::now();
        let mut st = self.lock();
        st.last_seen = Some(now);
        let job = match st.jobs.get(&result.id) {
            Some(job) if job.handed_out.is_some() => st.jobs.remove(&result.id),
            _ => None,
        };
        let Some(mut job) = job else {
            return false;
        };
        if let (Some(active), Some((tab, _))) = (st.active.as_mut(), &job.holder)
            && active.id == *tab
        {
            active.seen = now;
        }
        drop(st);
        // The fetch may have just given up; then the answer is simply dropped.
        job.reply.take().is_some_and(|tx| tx.send(result).is_ok())
    }
}

/// Removes its job from the relay when the fetch that owns it ends.
struct JobGuard<'a> {
    shared: &'a Shared,
    id: u64,
}

impl Drop for JobGuard<'_> {
    fn drop(&mut self) {
        self.shared.lock().jobs.remove(&self.id);
    }
}

/// Counts a long poll as held for as long as its handler runs (hyper drops the
/// handler when the browser side of the connection closes).
struct PollGuard<'a> {
    shared: &'a Shared,
    tab: &'a str,
}

impl Drop for PollGuard<'_> {
    fn drop(&mut self) {
        self.shared.release_poll(self.tab);
    }
}

type Response = hyper::Response<Full<Bytes>>;

async fn serve(listener: TcpListener, shared: Arc<Shared>) {
    // Owned here so aborting this task aborts every connection with it.
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let stream = match accepted {
                    Ok((stream, _)) => stream,
                    Err(_) => {
                        // Usually out of file descriptors; don't spin.
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                let shared = shared.clone();
                connections.spawn(async move {
                    let service = service_fn(move |req| {
                        let shared = shared.clone();
                        async move { Ok::<_, Infallible>(handle(&shared, req).await) }
                    });
                    // A connection error only affects that connection; the tab retries.
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
            // Reap finished connections so the set doesn't grow.
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }
}

async fn handle(shared: &Shared, req: hyper::Request<Incoming>) -> Response {
    let allowed = req
        .headers()
        .get(header::ORIGIN)
        .is_some_and(|o| o == ATHLETIC_ORIGIN);
    if !allowed {
        // No CORS headers: the browser keeps the answer from any other page.
        return hyper::Response::builder()
            .status(StatusCode::FORBIDDEN)
            .body(Full::from(format!(
                "the ath relay only serves {ATHLETIC_ORIGIN}\n"
            )))
            .expect("static response");
    }
    match (req.method(), req.uri().path()) {
        (&HttpMethod::OPTIONS, _) => preflight(),
        (&HttpMethod::GET, "/next") => match poll_params(req.uri().query()) {
            Some((tab, lp)) => next_job(shared, &tab, lp).await,
            None => reply(StatusCode::BAD_REQUEST, None),
        },
        (&HttpMethod::POST, "/result") => post_result(shared, req).await,
        _ => reply(StatusCode::NOT_FOUND, None),
    }
}

/// `tab=<id>&loop=<n>` from a `/next` query.
fn poll_params(query: Option<&str>) -> Option<(String, u32)> {
    let (mut tab, mut lp) = (None, None);
    for pair in query?.split('&') {
        match pair.split_once('=') {
            Some(("tab", v)) => tab = Some(v),
            Some(("loop", v)) => lp = v.parse::<u32>().ok(),
            _ => {}
        }
    }
    let tab = tab.filter(|t| {
        !t.is_empty() && t.len() <= 64 && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })?;
    Some((tab.to_string(), lp?))
}

/// The CORS preflight, including Chrome's Private Network Access header.
fn preflight() -> Response {
    let mut resp = reply(StatusCode::NO_CONTENT, None);
    let h = resp.headers_mut();
    h.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST"),
    );
    h.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("content-type"),
    );
    h.insert(
        "access-control-allow-private-network",
        HeaderValue::from_static("true"),
    );
    h.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("600"),
    );
    resp
}

fn standby() -> Response {
    reply(StatusCode::OK, Some(r#"{"standby":true}"#.to_string()))
}

async fn next_job(shared: &Shared, tab: &str, lp: u32) -> Response {
    if !shared.register_poll(tab, lp) {
        return standby();
    }
    let _held = PollGuard { shared, tab };
    let now = Instant::now();
    let deadline = now
        .checked_add(shared.config.poll_wait)
        .unwrap_or(now + POLL_WAIT);
    loop {
        let ready = shared.job_ready.notified();
        tokio::pin!(ready);
        ready.as_mut().enable();
        match shared.take_next(tab, lp) {
            Take::Job(job) => {
                let json = serde_json::to_string(&job).expect("a job serializes");
                return reply(StatusCode::OK, Some(json));
            }
            // Another tab took over while this poll waited.
            Take::Standby => return standby(),
            Take::Nothing => {}
        }
        if timeout_at(deadline, ready).await.is_err() {
            return reply(StatusCode::NO_CONTENT, None);
        }
    }
}

async fn post_result(shared: &Shared, req: hyper::Request<Incoming>) -> Response {
    let bytes = match Limited::new(req.into_body(), MAX_RESULT_BYTES)
        .collect()
        .await
    {
        Ok(body) => body.to_bytes(),
        Err(_) => return reply(StatusCode::PAYLOAD_TOO_LARGE, None),
    };
    let Ok(result) = serde_json::from_slice::<TabResult>(&bytes) else {
        return reply(StatusCode::BAD_REQUEST, None);
    };
    if shared.deliver(result) {
        reply(StatusCode::NO_CONTENT, None)
    } else {
        // Unknown, already answered, or abandoned by its fetch.
        reply(StatusCode::NOT_FOUND, None)
    }
}

/// A response to the Athletic.net origin, with its CORS headers.
fn reply(status: StatusCode, json: Option<String>) -> Response {
    let has_body = json.is_some();
    let mut resp = hyper::Response::new(Full::from(json.unwrap_or_default()));
    *resp.status_mut() = status;
    let h = resp.headers_mut();
    h.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static(ATHLETIC_ORIGIN),
    );
    h.insert(header::VARY, HeaderValue::from_static("Origin"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if has_body {
        h.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
    }
    resp
}

#[cfg(test)]
mod tests {
    use percent_encoding::percent_decode_str;

    use super::*;

    #[test]
    fn a_tab_result_without_a_body_is_a_relay_error() {
        let req = Request::get("https://www.athletic.net/api/v1/x");
        let missing = TabResult {
            id: 1,
            status: Some(200),
            body: None,
            error: None,
        };
        let err = missing.into_response(&req).unwrap_err().to_string();
        assert!(err.contains("without a body"), "{err}");
        let empty = TabResult {
            id: 1,
            status: Some(200),
            body: Some(String::new()),
            error: None,
        };
        assert_eq!(empty.into_response(&req).unwrap().body, "");
    }

    #[test]
    fn the_script_has_one_placeholder_for_the_relay_address() {
        assert_eq!(RELAY_JS.matches(RELAY_PLACEHOLDER).count(), 1);
        let script = console_script(9999);
        assert!(script.contains("\"http://127.0.0.1:9999\""));
        assert!(!script.contains(RELAY_PLACEHOLDER));
    }

    #[test]
    fn the_bookmarklet_decodes_to_the_compacted_script() {
        let b = bookmarklet(DEFAULT_PORT);
        let encoded = b.strip_prefix("javascript:").unwrap();
        assert!(!encoded.contains(' ') && !encoded.contains('\n'));
        let decoded = percent_decode_str(encoded).decode_utf8().unwrap();
        assert!(decoded.starts_with("(() => {\nconst ATH = \"http://127.0.0.1:8767\";"));
        assert!(decoded.ends_with("})();"));
        assert!(
            !decoded.contains("ath browser relay: run in"),
            "header dropped"
        );
    }

    #[test]
    fn encodes_like_encode_uri_component() {
        let encoded = utf8_percent_encode("a b/c?d=\"e\"\n(f)!*'~._-", URI_COMPONENT).to_string();
        assert_eq!(encoded, "a%20b%2Fc%3Fd%3D%22e%22%0A(f)!*'~._-");
    }

    #[test]
    fn setup_steps_name_the_install_command_for_the_port() {
        let steps = setup_steps(DEFAULT_PORT, false);
        assert!(steps.contains("Run `ath relay install`"), "{steps}");
        assert!(steps.contains("`ath relay install --console`"), "{steps}");
        let below = setup_steps(9000, true);
        assert!(below.contains("the bookmarklet below"), "{below}");
        assert!(
            below.contains("`ath --relay-port 9000 relay install --console`"),
            "{below}"
        );
    }

    #[test]
    fn poll_params_need_a_tab_id_and_a_loop_number() {
        assert_eq!(
            poll_params(Some("tab=1b4e-9f&loop=3")),
            Some(("1b4e-9f".to_string(), 3))
        );
        for bad in [
            "",
            "tab=a",
            "loop=1",
            "tab=&loop=1",
            "tab=a&loop=x",
            "tab=a%20b&loop=1",
        ] {
            assert_eq!(poll_params(Some(bad)), None, "{bad}");
        }
        assert_eq!(poll_params(None), None);
    }
}
