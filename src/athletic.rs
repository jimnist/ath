//! The live Athletic.net [`DataSource`], built on a [`Fetcher`].
//!
//! Handles Cloudflare challenges and throttled answers (retry-with-backoff;
//! persistent → a clear error with advice) and parses the `api/v1` endpoints.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use chrono::NaiveDate;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::area::{AreaTree, Gender, NavInfo, RankingsToken, Region, TeamNav};
use crate::cache::{Cacheable, DataKind, ResponseCache, season_finished};
use crate::http::{FetchResponse, Fetcher, Request};
use crate::list::{Grades, PageQuery, RankingsPage};
use crate::meet::{EntriesData, EntriesQuery, MeetData, MeetEventList};
use crate::model::{Bio, Candidate};
use crate::record::{Recorder, SEARCH_QUERY_FIELD, set_top_level_string};
use crate::source::{DataSource, layout};

const API_BASE: &str = "https://www.athletic.net/api/v1";

/// The event of the rankings request that fetches a token. Any event works;
/// the token is for the list's division, not the event.
const TOKEN_EVENT: &str = "400m";

/// The gender trees are fetched with. List ids are the same for both, so one
/// fetch (and one cache entry) per season serves both; gender only changes
/// the links.
const TREE_GENDER: Gender = Gender::F;

/// What to do when Athletic.net keeps throttling (empty or non-JSON 2xx
/// answers). The same for every transport, since no credential is at fault.
pub const THROTTLE_ADVICE: &str = "Athletic.net is throttling this burst of requests: wait a few \
minutes and re-run, and consider a lower --concurrency or a higher --delay-ms.";

/// The delay between rankings pages once Athletic.net has pushed back.
const SLOWED_PAGE_DELAY: Duration = Duration::from_secs(1);

/// The longest delay between rankings pages, however often Athletic.net
/// pushes back.
const MAX_PAGE_DELAY: Duration = Duration::from_secs(8);

/// Spaces out the rankings requests actually sent (a cached page never
/// waits): one at a time, a fixed delay apart, slowed for the rest of the run
/// (doubling, from [`SLOWED_PAGE_DELAY`] up to [`MAX_PAGE_DELAY`]) whenever
/// one of them drew a throttled answer or a Cloudflare challenge that had to
/// be retried. One pacer serves every list of a run, so one list's pushback
/// slows the next.
#[derive(Debug)]
struct Pacer {
    delay: Duration,
    /// When the last paced request came back.
    last: Option<tokio::time::Instant>,
}

impl Pacer {
    fn new(delay: Duration) -> Self {
        Self { delay, last: None }
    }

    /// Wait until the delay since the last fetched page has passed.
    async fn wait(&self) {
        if let Some(last) = self.last {
            tokio::time::sleep_until(last + self.delay).await;
        }
    }

    /// After a request: note when it ended, and slow down if it was pushed
    /// back (needed a retry).
    fn done(&mut self, pushed_back: bool) {
        self.last = Some(tokio::time::Instant::now());
        if !pushed_back {
            return;
        }
        let slower = self
            .delay
            .saturating_mul(2)
            .clamp(SLOWED_PAGE_DELAY, MAX_PAGE_DELAY);
        if slower > self.delay {
            self.delay = slower;
            crate::progress::message(&format!(
                "ath: Athletic.net is pushing back; reading one rankings page every {:.1}s from \
                 now on",
                slower.as_secs_f64()
            ));
        }
    }
}

/// Retry policy for Cloudflare challenges and throttled answers.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub base_delay: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_millis(500),
        }
    }
}

/// A State subtree read this run, with the response it came from when
/// recording (`None` otherwise: the body is kept only to be recorded again).
type HeldTree = (Vec<Region>, Option<String>);

/// The live Athletic.net data source.
pub struct AthleticNet<F: Fetcher> {
    fetcher: F,
    retry: RetryConfig,
    cache: Option<ResponseCache>,
    /// Set once a cache write has failed, so the warning prints once per run
    /// instead of once per request.
    cache_warned: AtomicBool,
    /// Paces the rankings requests sent (`--delay-ms`).
    pages: tokio::sync::Mutex<Pacer>,
    /// Today's date, for whether a season is finished; `None` reads the
    /// local clock (tests pin it).
    today: Option<NaiveDate>,
    /// State subtrees read this run, by season and State list, so a run that
    /// completes the same State twice (a Profile's Area, then `ath rank`'s
    /// lists) asks once even with the cache off. Kept with the response they
    /// came from, for the Recording.
    state_trees: std::sync::Mutex<HashMap<(i64, i64), HeldTree>>,
    /// Each meet's `jwtMeet` token, from this run's `GetMeetData` (`None`
    /// when it gave none, so it is not asked again). Never cached: it is a
    /// credential for the meet's event list and entries.
    meet_tokens: std::sync::Mutex<HashMap<i64, Option<String>>>,
    /// Where every response this source answers with is also written
    /// (`--record-dir`), when recording.
    recorder: Option<Recorder>,
}

impl<F: Fetcher> AthleticNet<F> {
    pub fn new(fetcher: F) -> Self {
        Self {
            fetcher,
            retry: RetryConfig::default(),
            cache: None,
            cache_warned: AtomicBool::new(false),
            pages: tokio::sync::Mutex::new(Pacer::new(Duration::ZERO)),
            today: None,
            state_trees: Default::default(),
            meet_tokens: Default::default(),
            recorder: None,
        }
    }

    /// Also write every response this source answers with (from Athletic.net
    /// or the cache alike) into a Recording ([`crate::record`]).
    pub fn with_recorder(mut self, recorder: Recorder) -> Self {
        self.recorder = Some(recorder);
        self
    }

    /// A response to hold for recording it again later: only when recording.
    fn held_body(&self, body: &str) -> Option<String> {
        self.recorder.as_ref().map(|_| body.to_string())
    }

    /// Write `raw`, the response behind an answer, to `rel` in the
    /// Recording, when recording.
    fn record(&self, rel: std::path::PathBuf, raw: &str) -> Result<()> {
        match &self.recorder {
            Some(recorder) => recorder.write(&rel, raw),
            None => Ok(()),
        }
    }

    /// Pin today's date (tests).
    pub fn with_today(mut self, today: NaiveDate) -> Self {
        self.today = Some(today);
        self
    }

    fn today(&self) -> NaiveDate {
        self.today
            .unwrap_or_else(|| chrono::Local::now().date_naive())
    }

    pub fn with_retry(mut self, retry: RetryConfig) -> Self {
        self.retry = retry;
        self
    }

    /// The delay between rankings pages fetched from Athletic.net.
    pub fn with_page_delay(mut self, delay: Duration) -> Self {
        self.pages = tokio::sync::Mutex::new(Pacer::new(delay));
        self
    }

    pub fn with_cache(mut self, cache: ResponseCache) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Send a request (GET or POST), retrying on a Cloudflare challenge or a
    /// throttled answer (an empty or non-JSON 2xx) with exponential backoff.
    /// After the retry budget is exhausted, returns an error with advice: the
    /// fetcher's for its transport on a challenge (ADR 0004), or
    /// [`THROTTLE_ADVICE`] when Athletic.net is throttling.
    async fn fetch_text(&self, req: &Request) -> Result<String> {
        self.fetch_text_retried(req).await.0
    }

    /// [`Self::fetch_text`], also saying how many retries the request
    /// needed (Athletic.net's pushback).
    async fn fetch_text_retried(&self, req: &Request) -> (Result<String>, u32) {
        let (resp, attempts) = self.fetch_retried(req).await;
        (resp.map(|r| r.body), attempts)
    }

    /// [`Self::fetch_text_retried`] with the answer's HTTP status, for
    /// callers that must tell a refusal (401/403 with a JSON body) from data.
    async fn fetch_retried(&self, req: &Request) -> (Result<FetchResponse>, u32) {
        let mut attempt: u32 = 0;
        loop {
            let resp = match self.fetcher.fetch(req).await {
                Ok(resp) => resp,
                Err(e) => return (Err(e), attempt),
            };
            let (what, advice) = if resp.looks_like_challenge() {
                ("a Cloudflare challenge", self.fetcher.challenge_advice())
            } else if resp.looks_throttled() {
                let what = if resp.body.trim().is_empty() {
                    "an empty response"
                } else {
                    "a non-JSON page"
                };
                (what, THROTTLE_ADVICE)
            } else {
                return (Ok(resp), attempt);
            };
            if attempt >= self.retry.max_retries {
                let err = anyhow::anyhow!(
                    "Athletic.net returned {what} (HTTP {}) after {} attempts. {advice}",
                    resp.status,
                    attempt + 1,
                );
                return (Err(err), attempt);
            }
            // Saturating so a large max_retries can't overflow into a panic.
            let backoff = self
                .retry
                .base_delay
                .saturating_mul(2u32.saturating_pow(attempt));
            tokio::time::sleep(backoff).await;
            attempt += 1;
        }
    }

    /// Send a request and parse its JSON response, going through the cache
    /// when `kind` says what the data is (`None` is never cached). `what`
    /// names the data in parse errors. Only answers that are
    /// [`Cacheable::cacheable`] are cached or served from the cache. With the
    /// response text it was parsed from.
    async fn fetch_json<T: DeserializeOwned + Cacheable>(
        &self,
        req: &Request,
        kind: Option<DataKind>,
        what: &str,
    ) -> Result<(T, String)> {
        self.fetch_json_as(req, kind, what, false).await
    }

    /// [`Self::fetch_json`] for rankings requests: one at a time through the
    /// page pacer ([`AthleticNet::with_page_delay`]) when the request is
    /// sent, never when the cache answers. With the response text.
    async fn fetch_json_paced<T: DeserializeOwned + Cacheable>(
        &self,
        req: &Request,
        kind: Option<DataKind>,
        what: &str,
    ) -> Result<(T, String)> {
        self.fetch_json_as(req, kind, what, true).await
    }

    async fn fetch_json_as<T: DeserializeOwned + Cacheable>(
        &self,
        req: &Request,
        kind: Option<DataKind>,
        what: &str,
        paced: bool,
    ) -> Result<(T, String)> {
        let cached = || kind.and_then(|kind| self.cached(req, kind));
        if let Some(parsed) = cached() {
            return Ok(parsed);
        }
        let body = if paced {
            // Requests wait their turn here, so look again: another task may
            // have just fetched and cached this one.
            let mut pacer = self.pages.lock().await;
            if let Some(parsed) = cached() {
                return Ok(parsed);
            }
            pacer.wait().await;
            let (body, retries) = self.fetch_text_retried(req).await;
            pacer.done(retries > 0);
            body?
        } else {
            self.fetch_text(req).await?
        };
        let parsed = self.parse_and_store(req, kind, &body, what)?;
        Ok((parsed, body))
    }

    /// A cache hit, reparsed from the stored raw payload. One that no longer
    /// parses (say, a model change) is a miss: refetch and overwrite it. One
    /// that is not [`Cacheable::cacheable`] (written before such answers were
    /// refused) is a miss too, and deleted so it does not linger, unless it
    /// has been replaced since it was read. With the stored response text.
    fn cached<T: DeserializeOwned + Cacheable>(
        &self,
        req: &Request,
        kind: DataKind,
    ) -> Option<(T, String)> {
        let cache = self.cache.as_ref()?;
        let raw = cache.get(req, kind)?;
        let parsed: T = serde_json::from_str(&raw).ok()?;
        if parsed.cacheable() {
            return Some((parsed, raw));
        }
        if let Err(e) = cache.remove_if(req, &raw) {
            self.warn_cache(&e, "delete from");
        }
        None
    }

    /// Parse a fetched body, then cache it under `kind` (`None` is never
    /// cached) when it is [`Cacheable::cacheable`]. Parsing first means an
    /// invalid (but JSON-ish) body can't poison the cache for as long as its
    /// kind stays fresh.
    fn parse_and_store<T: DeserializeOwned + Cacheable>(
        &self,
        req: &Request,
        kind: Option<DataKind>,
        body: &str,
        what: &str,
    ) -> Result<T> {
        let parsed: T = parse(body, what)?;
        if let Some(kind) = kind.filter(|_| parsed.cacheable()) {
            self.store(req, kind, body);
        }
        Ok(parsed)
    }

    /// Write a cache entry. The cache is an optimization: failing to write it
    /// never fails a request whose data is already in hand.
    fn store(&self, req: &Request, kind: DataKind, raw: &str) {
        if let Some(cache) = &self.cache
            && let Err(e) = cache.put(req, kind, raw)
        {
            self.warn_cache(&e, "write to");
        }
    }

    /// Warn that the cache failed (`verb`: "write to", "delete from"), once a
    /// run: one failure means the cache dir is in trouble, whatever the verb.
    fn warn_cache(&self, e: &anyhow::Error, verb: &str) {
        if !self.cache_warned.swap(true, Ordering::Relaxed) {
            crate::progress::message(&format!(
                "ath: warning - could not {verb} the response cache, continuing without it: {e:#}"
            ));
        }
    }

    /// A rankings response for a list, for the `GetNavInfo` token it carries
    /// and the list's season. Never cached: the token expires.
    async fn rankings_token(&self, list_id: i64) -> Result<RankingsToken> {
        let req = rankings_request(list_id, TREE_GENDER, TOKEN_EVENT, &Grades::all(), 1)?;
        let (token, _) = self
            .fetch_json_paced(&req, None, &format!("rankings for list {list_id}"))
            .await?;
        Ok(token)
    }

    /// Send a meet request and check its answer: a 2xx whose body is a JSON
    /// object with `required` (`meet`, `events`, `entries`). A 401 or 403, or
    /// an authorization denial in place of the data (`{"Message":
    /// "Authorization has been denied ..."}`), is [`MeetAnswer::Refused`]; any
    /// other answer, another error object included, is an error naming the
    /// call and Athletic.net's message. Nothing here is cached.
    async fn send_meet(&self, req: &Request, required: &str, what: &str) -> Result<MeetAnswer> {
        let resp = self.fetch_retried(req).await.0?;
        let value: Option<serde_json::Value> = serde_json::from_str(&resp.body).ok();
        let message = value
            .as_ref()
            .and_then(|v| v.get("Message").or_else(|| v.get("message")))
            .and_then(|m| m.as_str())
            .map(str::to_string);
        if matches!(resp.status, 401 | 403) {
            return Ok(MeetAnswer::Refused {
                status: resp.status,
                message,
            });
        }
        if !(200..300).contains(&resp.status) {
            bail!(
                "Athletic.net answered {what} with HTTP {}{}",
                resp.status,
                message.map_or_else(String::new, |m| format!(": {m}"))
            );
        }
        match value {
            Some(v) if v.get(required).is_some_and(|d| !d.is_null()) => Ok(MeetAnswer::Data(v)),
            Some(serde_json::Value::Object(_))
                if message.as_deref().is_some_and(is_authorization_denial) =>
            {
                Ok(MeetAnswer::Refused {
                    status: resp.status,
                    message,
                })
            }
            Some(serde_json::Value::Object(_)) if message.is_some() => bail!(
                "Athletic.net answered {what} with an error: {}",
                message.unwrap_or_default()
            ),
            _ => bail!(
                "Athletic.net answered {what} with something other than its data (no `{required}`)"
            ),
        }
    }

    /// `GetMeetData` from Athletic.net, never the cache, with its token and
    /// the response text without it. The response is parsed once; it is
    /// cached without the token ([`DataKind::MeetEntries`]), and the token is
    /// kept for this run only.
    async fn fetch_meet(&self, meet_id: i64) -> Result<(MeetData, Option<String>, String)> {
        let req = meet_request(meet_id);
        let what = format!("meet {meet_id}");
        let mut value = match self.send_meet(&req, "meet", &what).await? {
            MeetAnswer::Data(v) => v,
            MeetAnswer::Refused { status, message } => {
                bail!("{}", refused(&what, status, message.as_deref(), ""))
            }
        };
        let token = value
            .as_object_mut()
            .and_then(|o| o.remove("jwtMeet"))
            .and_then(|t| t.as_str().map(str::to_string))
            .filter(|t| !t.trim().is_empty());
        let stored = value.to_string();
        let data: MeetData =
            serde_json::from_value(value).with_context(|| format!("parsing {what}"))?;
        self.store(&req, DataKind::MeetEntries, &stored);
        self.meet_tokens
            .lock()
            .unwrap()
            .insert(meet_id, token.clone());
        Ok((data, token, stored))
    }

    /// The meet's token for this run: the one its `GetMeetData` gave, or a
    /// fresh `GetMeetData` when this run has not read it (the meet came from
    /// the cache, which never holds the token, or the held one was refused).
    /// A `GetMeetData` this run read without a token is not sent again.
    async fn meet_token(&self, meet_id: i64) -> Result<String> {
        let held = self.meet_tokens.lock().unwrap().get(&meet_id).cloned();
        let token = match held {
            Some(token) => token,
            None => self.fetch_meet(meet_id).await?.1,
        };
        token.with_context(|| {
            format!(
                "Athletic.net gave no jwtMeet token for meet {meet_id}, which its event list \
                 and entries need; the meet may not publish its entries"
            )
        })
    }

    /// A meet request that needs the meet's token: from the cache when fresh
    /// ([`DataKind::MeetEntries`]), otherwise sent with the token in an
    /// `anettokens` header (fetching the token only then). A refused token is
    /// dropped and fetched again once; a second refusal is an error. With
    /// the response text, as cached.
    async fn fetch_with_meet_token<T: DeserializeOwned + Cacheable>(
        &self,
        meet_id: i64,
        req: Request,
        required: &str,
        what: &str,
    ) -> Result<(T, String)> {
        if let Some(cached) = self.cached(&req, DataKind::MeetEntries) {
            return Ok(cached);
        }
        let mut retried = false;
        loop {
            let token = self.meet_token(meet_id).await?;
            let sent = req.clone().with_header("anettokens", token);
            match self.send_meet(&sent, required, what).await? {
                MeetAnswer::Data(value) => {
                    let stored = value.to_string();
                    let parsed: T =
                        serde_json::from_value(value).with_context(|| format!("parsing {what}"))?;
                    self.store(&req, DataKind::MeetEntries, &stored);
                    return Ok((parsed, stored));
                }
                MeetAnswer::Refused { status, message } => {
                    let why = |retried: &str| refused(what, status, message.as_deref(), retried);
                    if retried {
                        bail!("{}", why(", even with a fresh meet token"));
                    }
                    retried = true;
                    // A fresh token; without one, the refusal is the news.
                    self.meet_tokens.lock().unwrap().remove(&meet_id);
                    if self.fetch_meet(meet_id).await?.1.is_none() {
                        bail!("{}", why(", and a fresh GetMeetData gave no meet token"));
                    }
                }
            }
        }
    }
}

/// True for Athletic.net's message when it denies a request for want of
/// authorization (`Authorization has been denied for this request.`).
fn is_authorization_denial(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    ["authoriz", "denied", "unauthorized"]
        .iter()
        .any(|w| m.contains(w))
}

/// A meet request's checked answer ([`AthleticNet::send_meet`]).
enum MeetAnswer {
    /// The data, as JSON.
    Data(serde_json::Value),
    /// Refused: a 401/403, or an error object instead of the data.
    Refused {
        status: u16,
        message: Option<String>,
    },
}

/// Why a meet request was refused, and what may help (ADR 0001).
fn refused(what: &str, status: u16, message: Option<&str>, retried: &str) -> String {
    format!(
        "Athletic.net refused {what} (HTTP {status}{}){retried}. Reading a \
         meet's entries may need a signed-in session with a paid Athletic.net subscription \
         (ADR 0001), or the meet does not publish its entries.",
        message.map_or_else(String::new, |m| format!(": {m}"))
    )
}

// Answers always worth caching (when their kind is). Rankings pages are not
// when capped (`list.rs`).
impl Cacheable for Bio {}
impl Cacheable for TeamNav {}
impl Cacheable for RankingsToken {}
impl Cacheable for SearchResponse {}
// Refusals and error objects never get this far (`send_meet`).
impl Cacheable for MeetData {}
impl Cacheable for MeetEventList {}
impl Cacheable for EntriesData {}

/// `GetRankings` for one page of a list, as Athletic.net's own page asks for
/// it: the default filters (FAT-only, official marks), `qParams.grades` only
/// for a Grade group and `qParams.page` only past the first page.
fn rankings_request(
    list_id: i64,
    gender: Gender,
    event: &str,
    grades: &Grades,
    page: u32,
) -> Result<Request> {
    let g = gender.as_str();
    let mut q_params = serde_json::Map::new();
    if !grades.is_all() {
        q_params.insert("grades".into(), grades.as_slice().into());
    }
    if page > 1 {
        q_params.insert("page".into(), page.into());
    }
    Request::post_json(
        format!("{API_BASE}/tfRankings/GetRankings"),
        &serde_json::json!({
            "reportType": "list", "mode": g, "divListId": list_id,
            "recordSetId": null, "locationId": null, "teamId": null, "listId": null,
            "meetId": null, "indoor": false, "eventShort": event, "gender": g,
            "qParams": q_params, "qualifyingListKey": "", "version": 2, "debug": "",
        }),
    )
}

/// `GetNavInfo` for a season's high school tree. Sent with the `anettokens`
/// header of a list's rankings token; the cache key leaves the token out.
fn tree_request(season: i64) -> Request {
    Request::get(format!(
        "{API_BASE}/tfRankings/GetNavInfo?seasonId={season}&level=4&gender={}\
         &recordSetId=0&locationId=0&teamId=0&indoor=false",
        TREE_GENDER.as_str()
    ))
}

/// The cache key of the "no such season" marker for `season` as seen from
/// `from_list`. Never sent: it is the tree request plus the start list,
/// because the marker's evidence is only that list's own `seasons` map, so
/// one list's marker never answers for another.
fn missing_season_request(season: i64, from_list: i64) -> Request {
    Request::get(format!(
        "{}&athFromList={from_list}",
        tree_request(season).url()
    ))
}

/// The cache key of a State's own subtree ([`DataSource::state_tree`]) for
/// `season`, read with a token of `state_list`. Never sent: it is the tree
/// request plus the State's list, so a State's subtree is kept apart from the
/// season's national tree and never stands in for it.
fn state_tree_request(season: i64, state_list: i64) -> Request {
    Request::get(format!(
        "{}&athStateList={state_list}",
        tree_request(season).url()
    ))
}

/// `GetMeetData` for a meet. Needs no token, and answers with the
/// `jwtMeet` token the meet's event list and entries need.
fn meet_request(meet_id: i64) -> Request {
    Request::get(format!(
        "{API_BASE}/Meet/GetMeetData?meetId={meet_id}&sport=tf"
    ))
}

/// `GetEventListData` for a meet. Sent with the meet's token in an
/// `anettokens` header; the cache key leaves it out.
fn meet_events_request(meet_id: i64) -> Request {
    Request::get(format!("{API_BASE}/Meet/GetEventListData?meetId={meet_id}"))
}

/// `GetEntriesData` for one event of a meet in one division, as the entries
/// page asks for it (`seasonId` 0). Sent with the meet's token.
fn meet_entries_request(q: &EntriesQuery<'_>) -> Result<Request> {
    Request::post_json(
        format!("{API_BASE}/Meet/GetEntriesData"),
        &serde_json::json!({
            "meetId": q.meet_id, "sport": "tf", "gender": q.gender.as_str(),
            "divId": q.div_id, "eventShort": q.event_short, "seasonId": 0,
        }),
    )
}

/// Parse a JSON response; `what` names the data in the error.
fn parse<T: DeserializeOwned>(body: &str, what: &str) -> Result<T> {
    serde_json::from_str(body).with_context(|| format!("parsing {what}"))
}

/// A season's tree must hold States; anything else is not worth keeping.
fn check_tree(nav: &NavInfo) -> Result<()> {
    if nav.has_states() {
        Ok(())
    } else {
        bail!("the navigation response lists no States")
    }
}

/// The cached marker for a season Athletic.net does not have
/// ([`DataKind::MissingSeason`]), with its evidence: the season of the start
/// list whose `seasons` map lacked it.
#[derive(Serialize, Deserialize)]
struct MissingSeasonMarker {
    ath_missing_season: bool,
    start_season: i64,
}

impl<F: Fetcher> AthleticNet<F> {
    /// The one cache read for trees. `Some(Some(tree))` for the season's
    /// cached national tree; with `from_list`, `Some(None)` for a fresh "no
    /// such season" marker read from that list, honored only when the list is
    /// in another season (a list in the season leads to its tree itself);
    /// otherwise `None`, a miss. A tree comes with its response text.
    fn cached_tree(
        &self,
        season: i64,
        from_list: Option<i64>,
    ) -> Option<Option<(AreaTree, String)>> {
        let cache = self.cache.as_ref()?;
        if let Some(raw) = cache.get(&tree_request(season), DataKind::AreaTree)
            && let Ok(nav) = serde_json::from_str::<NavInfo>(&raw)
            && check_tree(&nav).is_ok()
            && nav.is_national()
        {
            return Some(Some((AreaTree::from_nav(season, nav), raw)));
        }
        let marker = cache.get(
            &missing_season_request(season, from_list?),
            DataKind::MissingSeason,
        )?;
        let marker: MissingSeasonMarker = serde_json::from_str(&marker).ok()?;
        (marker.ath_missing_season && marker.start_season != season).then_some(None)
    }

    /// Fetch a season's tree with a list's token. A tree without States is an
    /// error. Only the national tree ([`NavInfo::is_national`]) is cached,
    /// forever: a token that answers with part of it serves this answer but
    /// never stands in for the season's tree. With the response text.
    async fn fetch_tree(&self, season: i64, token: &str) -> Result<(NavInfo, String)> {
        let what = format!("the {season} division tree");
        let (nav, body) = self.fetch_nav(season, token, &what).await?;
        check_tree(&nav).with_context(|| format!("checking {what}"))?;
        Ok((nav, body))
    }

    /// `GetNavInfo` for `season` with a list's token, as Athletic.net
    /// answered it, and its body. The national tree in it is kept as the
    /// season's ([`NavInfo::is_national`]), whichever list's token asked.
    async fn fetch_nav(&self, season: i64, token: &str, what: &str) -> Result<(NavInfo, String)> {
        let req = tree_request(season);
        let body = self
            .fetch_text(&req.clone().with_header("anettokens", token))
            .await?;
        let nav: NavInfo = parse(&body, what)?;
        if check_tree(&nav).is_ok() && nav.is_national() {
            self.store(&req, DataKind::AreaTree, &body);
        }
        self.keep_state_tree(season, &nav, &body);
        Ok((nav, body))
    }

    /// When `nav` was read with a State list's token, its `tree` is that
    /// State's own subtree: remember it for the run and, once it has
    /// divisions below the State, keep it as [`DataSource::state_tree`] would,
    /// so completing the State's Areas later costs no request. A subtree with
    /// nothing below the State yet is not kept (Areas added later are found).
    fn keep_state_tree(&self, season: i64, nav: &NavInfo, body: &str) {
        let Some(root) = nav
            .tree
            .iter()
            .find(|r| r.div_type == "State" && r.depth == Some(0))
        else {
            return;
        };
        if nav.tree.iter().any(|r| r.id != root.id) {
            self.store(
                &state_tree_request(season, root.id),
                DataKind::AreaTree,
                body,
            );
        }
        self.state_trees
            .lock()
            .unwrap()
            .insert((season, root.id), (nav.tree.clone(), self.held_body(body)));
    }

    /// A list's rankings token and season (`None` when the response does not
    /// say).
    async fn token_for(&self, list_id: i64) -> Result<(String, Option<i64>)> {
        let r = self.rankings_token(list_id).await?;
        let season = r.division.as_ref().and_then(|d| d.season_id);
        Ok((r.token, season))
    }

    /// [`DataSource::season_area_tree`] past the cache: rankings for
    /// `list_id` (token, checked to be in `season` when the response says
    /// its season) -> `GetNavInfo` (`regions`). With the response text.
    async fn fetch_season_tree(&self, season: i64, list_id: i64) -> Result<(AreaTree, String)> {
        let (token, list_season) = self.token_for(list_id).await?;
        if let Some(s) = list_season
            && s != season
        {
            bail!("rankings list {list_id} is in season {s}, not {season}");
        }
        let (nav, body) = self.fetch_tree(season, &token).await?;
        Ok((AreaTree::from_nav(season, nav), body))
    }

    /// Record a season's tree (the response it came from, read with a token
    /// of `list`): the national tree as the season's (`navinfo/<season>.json`,
    /// always replacing what is there); a partial answer as the list's own
    /// (`navinfo/<season>-<list>.json`), and as the season's only by an
    /// exclusive create ([`Recorder::write_new`]), so it never overwrites the
    /// national tree, even one written at the same moment.
    fn record_tree(&self, season: i64, list: i64, found: (AreaTree, String)) -> Result<AreaTree> {
        if let Some(recorder) = &self.recorder {
            let (tree, body) = &found;
            if tree.national {
                recorder.write(&layout::navinfo(season), body)?;
            } else {
                recorder.write(&layout::state_navinfo(season, list), body)?;
                recorder.write_new(&layout::navinfo(season), body)?;
            }
        }
        Ok(found.0)
    }
}

#[async_trait]
impl<F: Fetcher> DataSource for AthleticNet<F> {
    /// Hop from `from_list` to the season asked for: rankings for
    /// `from_list` (token, its season S) -> `GetNavInfo` for S (`seasons`)
    /// -> rankings and `GetNavInfo` through `seasons[season]`. Four requests,
    /// two when `from_list` is in the season, none once the national tree is
    /// cached (forever: [`DataKind::AreaTree`]). "No such season" is cached
    /// for a few hours per start list ([`DataKind::MissingSeason`]). When the
    /// rankings response does not say S, the hop reads `seasons` from the
    /// season asked for instead, and nothing about the season is remembered.
    async fn area_tree(&self, season: i64, from_list: i64) -> Result<Option<AreaTree>> {
        if let Some(cached) = self.cached_tree(season, Some(from_list)) {
            return cached
                .map(|found| self.record_tree(season, from_list, found))
                .transpose();
        }
        let (token, start_season) = self.token_for(from_list).await.with_context(|| {
            format!("could not read the rankings list to start from ({from_list}, see --from-list)")
        })?;
        let list_id = match start_season {
            Some(s) if s == season => {
                let (nav, body) = self.fetch_tree(season, &token).await?;
                let found = (AreaTree::from_nav(season, nav), body);
                return self.record_tree(season, from_list, found).map(Some);
            }
            Some(s) => {
                // The start list's own season's tree, whose `seasons` leads
                // elsewhere. Never read from the cache: a cached tree's
                // `seasons` would never learn of a new season.
                let (start, _) = self.fetch_tree(s, &token).await?;
                let Some(list_id) = start.season_list_id(season) else {
                    let marker = MissingSeasonMarker {
                        ath_missing_season: true,
                        start_season: s,
                    };
                    self.store(
                        &missing_season_request(season, from_list),
                        DataKind::MissingSeason,
                        &serde_json::to_string(&marker)?,
                    );
                    return Ok(None);
                };
                list_id
            }
            None => {
                let req = tree_request(season).with_header("anettokens", &token);
                let hop: NavInfo = parse(
                    &self.fetch_text(&req).await?,
                    &format!("the seasons of list {from_list}"),
                )?;
                let Some(list_id) = hop.season_list_id(season) else {
                    return Ok(None);
                };
                list_id
            }
        };
        let found = self.fetch_season_tree(season, list_id).await?;
        self.record_tree(season, list_id, found).map(Some)
    }

    /// Rankings for `list_id` (token, checked to be in `season`) ->
    /// `GetNavInfo` (`regions`). Two requests, none once the national tree is
    /// cached.
    async fn season_area_tree(&self, season: i64, list_id: i64) -> Result<AreaTree> {
        let found = match self.cached_tree(season, None) {
            Some(Some(found)) => found,
            _ => self.fetch_season_tree(season, list_id).await?,
        };
        self.record_tree(season, list_id, found)
    }

    /// Rankings for `state_list` (token, checked to be in `season`) ->
    /// `GetNavInfo` (`tree`). Two requests, none once read this run or
    /// cached: a subtree that starts at the State's list and has divisions
    /// below it is kept forever ([`DataKind::AreaTree`], under its own key);
    /// one with nothing below the State yet is not, so Areas Athletic.net adds
    /// later in a season are found. The national tree that comes with it is
    /// kept as the season's ([`Self::fetch_nav`]).
    async fn state_tree(&self, season: i64, state_list: i64) -> Result<Vec<Region>> {
        // Cloned out of the memo (the body only when recording), then written
        // with the lock released.
        let held = self
            .state_trees
            .lock()
            .unwrap()
            .get(&(season, state_list))
            .cloned();
        if let Some((tree, body)) = held {
            if let Some(body) = body {
                self.record(layout::state_navinfo(season, state_list), &body)?;
            }
            return Ok(tree);
        }
        let key = state_tree_request(season, state_list);
        let starts_at_list = |nav: &NavInfo| {
            nav.tree
                .iter()
                .any(|r| r.div_type == "State" && r.id == state_list)
        };
        let cached = self
            .cache
            .as_ref()
            .and_then(|cache| cache.get(&key, DataKind::AreaTree))
            .and_then(|raw| Some((serde_json::from_str::<NavInfo>(&raw).ok()?, raw)))
            .filter(|(nav, _)| starts_at_list(nav));
        let (tree, body) = match cached {
            Some((nav, raw)) => (nav.tree, raw),
            None => {
                let (token, list_season) = self.token_for(state_list).await?;
                if let Some(s) = list_season
                    && s != season
                {
                    bail!("rankings list {state_list} is in season {s}, not {season}");
                }
                let what = format!("the {season} tree of list {state_list}");
                let (nav, body) = self.fetch_nav(season, &token, &what).await?;
                if !starts_at_list(&nav) {
                    bail!("{what} does not start at that list");
                }
                (nav.tree, body)
            }
        };
        self.record(layout::state_navinfo(season, state_list), &body)?;
        self.state_trees
            .lock()
            .unwrap()
            .insert((season, state_list), (tree.clone(), self.held_body(&body)));
        Ok(tree)
    }

    async fn team_divisions(&self, school_id: i64, season: i64) -> Result<TeamNav> {
        let req = Request::get(format!(
            "{API_BASE}/TeamNav/Team?team={school_id}&sport=tf&season={season}"
        ));
        let kind = DataKind::TeamDivisions {
            season_finished: season_finished(season, self.today()),
        };
        let (nav, raw) = self
            .fetch_json(
                &req,
                Some(kind),
                &format!("divisions of school {school_id} in {season}"),
            )
            .await?;
        self.record(layout::teamnav(school_id, season), &raw)?;
        Ok(nav)
    }

    /// Cached per page: forever once the season is finished, 12 hours while
    /// it is in progress ([`DataKind::Rankings`]). Pages fetched from
    /// Athletic.net are paced ([`AthleticNet::with_page_delay`]); cached
    /// ones are not. A page that may be capped (for a session without a paid
    /// subscription) is never cached nor served from the cache, so it cannot
    /// stand in for the whole page in a later run that could read it (the
    /// `Cacheable` impl in `list.rs`).
    async fn rankings_page(&self, q: &PageQuery<'_>) -> Result<RankingsPage> {
        let req = rankings_request(q.list_id, q.gender, q.event, q.grades, q.page)?;
        let kind = DataKind::Rankings {
            season_finished: season_finished(q.season, self.today()),
        };
        let (page, raw) = self
            .fetch_json_paced(
                &req,
                Some(kind),
                &format!("page {} of rankings list {}", q.page, q.list_id),
            )
            .await?;
        // The page's token is a credential, never data: the recorder empties it.
        self.record(layout::rankings(q), &raw)?;
        Ok(page)
    }

    async fn get_bio(&self, athlete_id: i64) -> Result<Bio> {
        let req = Request::get(format!(
            "{API_BASE}/AthleteBio/GetAthleteBioData?athleteId={athlete_id}&sport=tf&level=0"
        ));
        let (bio, raw) = self
            .fetch_json(
                &req,
                Some(DataKind::Profile),
                &format!("bio JSON for athlete {athlete_id}"),
            )
            .await?;
        self.record(layout::bio(athlete_id), &raw)?;
        Ok(bio)
    }

    /// Cached for about an hour ([`DataKind::MeetEntries`]) without its
    /// `jwtMeet` token.
    async fn meet(&self, meet_id: i64) -> Result<MeetData> {
        let (meet, raw) = match self.cached(&meet_request(meet_id), DataKind::MeetEntries) {
            Some(cached) => cached,
            None => {
                let (meet, _, raw) = self.fetch_meet(meet_id).await?;
                (meet, raw)
            }
        };
        // Without its token, as cached.
        self.record(layout::meet(meet_id), &raw)?;
        Ok(meet)
    }

    /// Cached for about an hour ([`DataKind::MeetEntries`]).
    async fn meet_events(&self, meet_id: i64) -> Result<MeetEventList> {
        let (events, raw) = self
            .fetch_with_meet_token(
                meet_id,
                meet_events_request(meet_id),
                "events",
                &format!("the event list of meet {meet_id}"),
            )
            .await?;
        self.record(layout::meet_events(meet_id), &raw)?;
        Ok(events)
    }

    /// Cached for about an hour ([`DataKind::MeetEntries`]).
    async fn meet_entries(&self, q: &EntriesQuery<'_>) -> Result<EntriesData> {
        let (entries, raw) = self
            .fetch_with_meet_token(
                q.meet_id,
                meet_entries_request(q)?,
                "entries",
                &format!(
                    "{} entries of meet {} (division {}, {})",
                    q.event_short,
                    q.meet_id,
                    q.div_id,
                    q.gender.as_str()
                ),
            )
            .await?;
        self.record(layout::meet_entries(q), &raw)?;
        Ok(entries)
    }

    async fn search(&self, name: &str) -> Result<Vec<Candidate>> {
        let q = urlencode(name);
        // `fq=type:Athlete` scopes the Solr query to athletes (not meets/teams).
        let req = Request::get(format!(
            "{API_BASE}/AutoComplete/search?q={q}&fq=type%3AAthlete"
        ));
        // Search results are not one of ADR-0006's cached kinds; always live.
        let (parsed, raw): (SearchResponse, String) = self
            .fetch_json(&req, None, &format!("search JSON for {name:?}"))
            .await?;
        if self.recorder.is_some() {
            // Set, not added: a search read back from a recording has one.
            let raw = set_top_level_string(&raw, SEARCH_QUERY_FIELD, name);
            self.record(layout::search(name), &raw)?;
        }
        Ok(parsed.candidates())
    }
}

/// The athletes in an `AutoComplete/search` response (`raw`), as
/// [`DataSource::search`] answers with them.
pub fn search_candidates(raw: &str) -> Result<Vec<Candidate>> {
    Ok(parse::<SearchResponse>(raw, "search JSON")?.candidates())
}

impl SearchResponse {
    fn candidates(self) -> Vec<Candidate> {
        self.response
            .docs
            .into_iter()
            .filter(|d| d.doc_type == "Athlete")
            .filter_map(|d| d.into_candidate())
            .collect()
    }
}

/// The Solr-style AutoComplete response (only the fields we use).
#[derive(Debug, Deserialize)]
struct SearchResponse {
    response: SearchBody,
}

#[derive(Debug, Deserialize)]
struct SearchBody {
    #[serde(default)]
    docs: Vec<SearchDoc>,
}

#[derive(Debug, Deserialize)]
struct SearchDoc {
    id_db: String,
    #[serde(rename = "type", default)]
    doc_type: String,
    #[serde(default)]
    textsuggest: String,
    #[serde(default)]
    subtext: Option<String>,
    #[serde(default)]
    score: f64,
}

impl SearchDoc {
    fn into_candidate(self) -> Option<Candidate> {
        let athlete_id: i64 = self.id_db.parse().ok()?;
        let (school, city, state) = parse_subtext(self.subtext.as_deref());
        Some(Candidate {
            athlete_id,
            name: self.textsuggest,
            school,
            city,
            state,
            score: self.score,
        })
    }
}

/// Parse a search `subtext` of the form `"School (HS)||City, State"` into its
/// parts. Athlete subtext reliably carries the `||` separator; without it we
/// can't distinguish school from location, so we return nothing rather than
/// mislabel a bare location as a school.
fn parse_subtext(subtext: Option<&str>) -> (Option<String>, Option<String>, Option<String>) {
    let Some((school_part, loc_part)) = subtext.and_then(|t| t.split_once("||")) else {
        return (None, None, None);
    };
    let school = non_empty(school_part);
    let (city, state) = match loc_part.rsplit_once(", ") {
        Some((c, s)) => (non_empty(c), non_empty(s)),
        None => (non_empty(loc_part), None),
    };
    (school, city, state)
}

/// Trim and convert to `Some` unless empty.
fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Percent-encode a query value (spaces and the handful of chars we might pass).
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use anyhow::anyhow;

    use super::*;
    use crate::cache::CacheMode;
    use crate::http::{FetchResponse, Method};

    /// A `Fetcher` that returns queued responses in order, then errors
    /// ("exhausted") - so tests assert fetch count via how many it consumes.
    /// Every request it is sent is recorded.
    struct StubFetcher {
        responses: Mutex<VecDeque<FetchResponse>>,
        sent: Mutex<Vec<Request>>,
    }

    impl StubFetcher {
        fn new(responses: Vec<FetchResponse>) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
                sent: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl Fetcher for StubFetcher {
        async fn fetch(&self, req: &Request) -> Result<FetchResponse> {
            self.sent.lock().unwrap().push(req.clone());
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| anyhow!("stub fetcher exhausted"))
        }

        fn challenge_advice(&self) -> &'static str {
            "STUB ADVICE"
        }
    }

    fn rankings_post() -> Request {
        Request::post_json(
            format!("{API_BASE}/tfRankings/GetRankings"),
            &serde_json::json!({"eventShort": "400m", "qParams": {"page": 1}}),
        )
        .unwrap()
    }

    fn ok_json(body: &str) -> FetchResponse {
        FetchResponse {
            status: 200,
            body: body.to_string(),
        }
    }

    fn challenge() -> FetchResponse {
        FetchResponse {
            status: 503,
            body: "<!DOCTYPE html><html><head><title>Just a moment...</title></head></html>"
                .to_string(),
        }
    }

    const BIO_JSON: &str = r#"{
        "athlete": {"IDAthlete": 1001, "SchoolID": 501, "FirstName": "Alice", "LastName": "Anders", "Gender": "F"},
        "allTeams": {},
        "eventsTF": [{"Event": "400 Meters", "IDEvent": 21, "IDEventType": 0}],
        "resultsTF": [{"AthleteID": 1001, "Result": "58.24a", "SortInt": 58240, "EventID": 21, "EventTypeID": 0, "SeasonID": 2026, "Exhibition": false}]
    }"#;

    fn instant_retry() -> RetryConfig {
        RetryConfig {
            max_retries: 3,
            base_delay: Duration::ZERO,
        }
    }

    #[tokio::test]
    async fn get_bio_parses_json() {
        let src = AthleticNet::new(StubFetcher::new(vec![ok_json(BIO_JSON)]));
        let bio = src.get_bio(1001).await.unwrap();
        assert_eq!(bio.athlete.id, 1001);
        assert_eq!(bio.results_tf.len(), 1);
        assert_eq!(bio.results_tf[0].sort_int, 58240);
    }

    #[tokio::test]
    async fn retries_past_a_transient_challenge() {
        let src = AthleticNet::new(StubFetcher::new(vec![challenge(), ok_json(BIO_JSON)]))
            .with_retry(instant_retry());
        let bio = src.get_bio(1001).await.unwrap();
        assert_eq!(bio.athlete.id, 1001);
    }

    #[tokio::test]
    async fn persistent_challenge_errors_with_the_transports_advice() {
        let stub = StubFetcher::new(vec![challenge(), challenge(), challenge(), challenge()]);
        let src = AthleticNet::new(stub).with_retry(instant_retry());
        let err = src.get_bio(1001).await.unwrap_err().to_string();
        assert!(err.contains("Cloudflare challenge"), "got: {err}");
        assert!(
            err.contains("STUB ADVICE"),
            "should carry the fetcher's advice: {err}"
        );
    }

    #[tokio::test]
    async fn search_parses_and_filters_to_athletes() {
        let json = r#"{"response":{"numFound":2,"docs":[
            {"id_db":"982389051","type":"Athlete","textsuggest":"Jamie Uddumcombe","subtext":"Cranevale Heights (HS)||Laurelworth Falls, CA","score":3605.7},
            {"id_db":"900710","type":"XCMeet","textsuggest":"Harbor Hills Invite","subtext":"2026 Harbor High||Harbor, OR","score":49.0}
        ]}}"#;
        let src = AthleticNet::new(StubFetcher::new(vec![ok_json(json)]));
        let cands = src.search("Jamie Uddumcombe").await.unwrap();
        assert_eq!(cands.len(), 1, "only the Athlete doc is kept");
        let c = &cands[0];
        assert_eq!(c.athlete_id, 982389051);
        assert_eq!(c.name, "Jamie Uddumcombe");
        assert_eq!(c.school.as_deref(), Some("Cranevale Heights (HS)"));
        assert_eq!(c.city.as_deref(), Some("Laurelworth Falls"));
        assert_eq!(c.state.as_deref(), Some("CA"));
    }

    #[tokio::test]
    async fn get_bio_sends_a_get_to_the_bio_api() {
        let stub = StubFetcher::new(vec![ok_json(BIO_JSON)]);
        let src = AthleticNet::new(stub);
        src.get_bio(1001).await.unwrap();
        let sent = src.fetcher.sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].method(), Method::Get);
        assert_eq!(sent[0].body(), None);
        assert!(
            sent[0]
                .url()
                .ends_with("/AthleteBio/GetAthleteBioData?athleteId=1001&sport=tf&level=0"),
            "{}",
            sent[0].url()
        );
    }

    #[tokio::test]
    async fn post_retries_past_a_transient_challenge() {
        let src = AthleticNet::new(StubFetcher::new(vec![challenge(), ok_json("{\"ok\":1}")]))
            .with_retry(instant_retry());
        let body = src.fetch_text(&rankings_post()).await.unwrap();
        assert_eq!(body, "{\"ok\":1}");
        let sent = src.fetcher.sent.lock().unwrap();
        assert_eq!(sent.len(), 2, "retried once");
        assert!(
            sent.iter().all(|r| *r == rankings_post()),
            "same POST resent"
        );
    }

    #[tokio::test]
    async fn persistent_challenge_on_a_post_stops_and_asks() {
        let stub = StubFetcher::new(vec![challenge(), challenge(), challenge(), challenge()]);
        let src = AthleticNet::new(stub).with_retry(instant_retry());
        let err = src
            .fetch_text(&rankings_post())
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("Cloudflare challenge"), "got: {err}");
        assert!(err.contains("STUB ADVICE"), "should ask for help: {err}");
        assert_eq!(src.fetcher.sent.lock().unwrap().len(), 4, "no hammering");
    }

    fn answer(status: u16, body: &str) -> FetchResponse {
        FetchResponse {
            status,
            body: body.to_string(),
        }
    }

    #[tokio::test]
    async fn a_throttled_answer_is_retried() {
        // Seen live through the relay: a 200 with no body, then a good answer.
        // A 200 HTML page instead of JSON is the same throttle (HANDOFF.md).
        for throttled in [answer(200, ""), answer(200, "<html>slow down</html>")] {
            let stub = StubFetcher::new(vec![throttled, ok_json(BIO_JSON)]);
            let src = AthleticNet::new(stub).with_retry(instant_retry());
            assert_eq!(src.get_bio(1001).await.unwrap().athlete.id, 1001);
            assert_eq!(src.fetcher.sent.lock().unwrap().len(), 2);
        }
    }

    #[tokio::test]
    async fn persistent_throttling_stops_with_throttle_advice() {
        let stub = StubFetcher::new(vec![answer(200, ""); 4]);
        let src = AthleticNet::new(stub).with_retry(instant_retry());
        let msg = format!("{:#}", src.get_bio(1001).await.unwrap_err());
        assert!(msg.contains("empty response"), "got: {msg}");
        assert!(msg.contains(THROTTLE_ADVICE), "throttle advice: {msg}");
        assert!(!msg.contains("STUB ADVICE"), "no challenge advice: {msg}");
        assert_eq!(src.fetcher.sent.lock().unwrap().len(), 4, "no hammering");
    }

    #[tokio::test]
    async fn an_empty_error_status_is_not_retried() {
        // A bad id or server error fails at once, as before: not a throttle.
        let stub = StubFetcher::new(vec![answer(404, ""), ok_json(BIO_JSON)]);
        let src = AthleticNet::new(stub).with_retry(instant_retry());
        let msg = format!("{:#}", src.get_bio(1001).await.unwrap_err());
        assert_eq!(src.fetcher.sent.lock().unwrap().len(), 1);
        assert!(!msg.contains("STUB ADVICE") && !msg.contains(THROTTLE_ADVICE));
    }

    #[tokio::test]
    async fn an_empty_challenge_is_still_called_a_challenge() {
        let stub = StubFetcher::new(vec![answer(429, ""); 4]);
        let src = AthleticNet::new(stub).with_retry(instant_retry());
        let msg = format!("{:#}", src.get_bio(1001).await.unwrap_err());
        assert!(msg.contains("Cloudflare challenge"), "got: {msg}");
        assert!(msg.contains("STUB ADVICE"), "got: {msg}");
    }

    fn cache_in(dir: &std::path::Path, mode: CacheMode) -> ResponseCache {
        ResponseCache::new(dir, mode).unwrap()
    }

    #[tokio::test]
    async fn cache_hit_avoids_a_second_fetch() {
        let dir = tempfile::tempdir().unwrap();
        // Only one response queued; a second fetch would exhaust the stub.
        let src = AthleticNet::new(StubFetcher::new(vec![ok_json(BIO_JSON)]))
            .with_cache(cache_in(dir.path(), CacheMode::Normal));

        let first = src.get_bio(1001).await.unwrap();
        let second = src.get_bio(1001).await.unwrap();
        assert_eq!(first.athlete.id, second.athlete.id);
        // If the cache had missed, the stub would have been called twice and the
        // second call would have errored (exhausted).
    }

    #[tokio::test]
    async fn refresh_refetches_and_rewrites_the_bio() {
        let dir = tempfile::tempdir().unwrap();
        let seeded = AthleticNet::new(StubFetcher::new(vec![ok_json(BIO_JSON)]))
            .with_cache(cache_in(dir.path(), CacheMode::Normal));
        seeded.get_bio(1001).await.unwrap();

        let renamed = BIO_JSON.replace("Alice", "Alicia");
        let refresh = AthleticNet::new(StubFetcher::new(vec![ok_json(&renamed)]))
            .with_cache(cache_in(dir.path(), CacheMode::Refresh));
        let bio = refresh.get_bio(1001).await.unwrap();
        assert_eq!(bio.athlete.first_name, "Alicia", "refresh refetched");

        // A later normal run reads the rewritten entry without fetching.
        let normal = AthleticNet::new(StubFetcher::new(vec![]))
            .with_cache(cache_in(dir.path(), CacheMode::Normal));
        let bio = normal.get_bio(1001).await.unwrap();
        assert_eq!(
            bio.athlete.first_name, "Alicia",
            "refresh rewrote the entry"
        );
    }

    #[tokio::test]
    async fn invalid_bio_is_not_cached() {
        let dir = tempfile::tempdir().unwrap();
        let src = AthleticNet::new(StubFetcher::new(vec![
            ok_json("{\"not\":\"a bio\"}"),
            ok_json(BIO_JSON),
        ]))
        .with_cache(cache_in(dir.path(), CacheMode::Normal));
        assert!(src.get_bio(1001).await.is_err());
        assert_eq!(src.get_bio(1001).await.unwrap().athlete.id, 1001);
    }

    #[tokio::test]
    async fn a_failed_cache_write_does_not_fail_the_fetch() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path().join("cache");
        let cache = cache_in(&cache_dir, CacheMode::Normal);
        // The dir vanishing mid-run makes every write fail.
        std::fs::remove_dir(&cache_dir).unwrap();
        let src = AthleticNet::new(StubFetcher::new(vec![ok_json(BIO_JSON)])).with_cache(cache);
        assert_eq!(src.get_bio(1001).await.unwrap().athlete.id, 1001);
    }

    #[tokio::test]
    async fn a_cached_entry_that_no_longer_parses_is_refetched() {
        let dir = tempfile::tempdir().unwrap();
        let req = Request::get(format!(
            "{API_BASE}/AthleteBio/GetAthleteBioData?athleteId=1001&sport=tf&level=0"
        ));
        let cache = cache_in(dir.path(), CacheMode::Normal);
        cache
            .put(&req, DataKind::Profile, "{\"not\":\"a bio\"}")
            .unwrap();
        let src = AthleticNet::new(StubFetcher::new(vec![ok_json(BIO_JSON)])).with_cache(cache);
        assert_eq!(src.get_bio(1001).await.unwrap().athlete.id, 1001);
        assert_eq!(src.fetcher.sent.lock().unwrap().len(), 1, "refetched");
    }

    fn token(token: &str, list_id: i64, season: i64) -> FetchResponse {
        ok_json(
            &serde_json::json!({
                "jwtTFTopReport": token,
                "division": {"ID": list_id, "ParentID": 1, "BaseDivID": 278, "SeasonID": season, "Name": "California"},
                "groupedRankings": [],
            })
            .to_string(),
        )
    }

    /// A rankings response that does not say its list's season.
    fn token_without_season(token: &str, list_id: i64) -> FetchResponse {
        ok_json(
            &serde_json::json!({
                "jwtTFTopReport": token,
                "division": {"ID": list_id, "Name": "California"},
            })
            .to_string(),
        )
    }

    /// A national tree: the High School level, California (`ca_list`, with
    /// its Central Coast Section) and every other State.
    fn nav(seasons: serde_json::Value, ca_list: i64) -> FetchResponse {
        let mut regions = vec![
            serde_json::json!({"id": 1, "name": "High School", "state": null, "divType": "Level", "depth": 0}),
            serde_json::json!({"id": ca_list + 21, "name": " Central Coast", "state": "CA", "divType": "Section", "depth": 2}),
        ];
        regions.extend(crate::area::US_STATES.iter().zip(0..).map(|(st, i)| {
            let id = if *st == "CA" { ca_list } else { 900_000 + i };
            serde_json::json!({"id": id, "name": st, "state": st, "divType": "State", "depth": 1})
        }));
        ok_json(
            &serde_json::json!({"divListId": ca_list, "seasons": seasons, "regions": regions})
                .to_string(),
        )
    }

    /// Part of a tree: California alone, as a token for some lists answers.
    fn partial_nav(seasons: serde_json::Value, ca_list: i64) -> FetchResponse {
        ok_json(
            &serde_json::json!({
                "divListId": ca_list,
                "seasons": seasons,
                "regions": [
                    {"id": 1, "name": "High School", "state": null, "divType": "Level", "depth": 0},
                    {"id": ca_list, "name": "California", "state": "CA", "divType": "State", "depth": 1},
                ],
            })
            .to_string(),
        )
    }

    fn header<'a>(req: &'a Request, name: &str) -> Option<&'a str> {
        req.headers().find(|(n, _)| *n == name).map(|(_, v)| v)
    }

    fn body_json(req: &Request) -> serde_json::Value {
        serde_json::from_str(req.body().unwrap()).unwrap()
    }

    const CA_2026: i64 = 900_500;
    const CA_2025: i64 = 900_499;

    fn nav_req_ends(season: i64) -> String {
        format!(
            "/tfRankings/GetNavInfo?seasonId={season}&level=4&gender=f\
             &recordSetId=0&locationId=0&teamId=0&indoor=false"
        )
    }

    #[tokio::test]
    async fn area_tree_from_a_list_in_the_season_takes_two_requests_then_none() {
        let dir = tempfile::tempdir().unwrap();
        let stub = StubFetcher::new(vec![
            token("tok-a", CA_2026, 2026),
            nav(serde_json::json!({"2026": CA_2026}), CA_2026),
        ]);
        let src = AthleticNet::new(stub).with_cache(cache_in(dir.path(), CacheMode::Normal));
        let tree = src.area_tree(2026, CA_2026).await.unwrap().unwrap();
        assert_eq!(tree.state("CA").unwrap().id, CA_2026);
        {
            let sent = src.fetcher.sent.lock().unwrap();
            assert_eq!(sent.len(), 2);
            assert_eq!(sent[0].method(), Method::Post);
            assert!(sent[0].url().ends_with("/tfRankings/GetRankings"));
            let body = body_json(&sent[0]);
            assert_eq!(body["divListId"], CA_2026);
            assert_eq!((&body["gender"], &body["mode"]), (&"f".into(), &"f".into()));
            assert!(
                sent[1].url().ends_with(&nav_req_ends(2026)),
                "{}",
                sent[1].url()
            );
            assert_eq!(header(&sent[1], "anettokens"), Some("tok-a"));
        }
        // Cached forever, through either way in: nothing more is sent.
        let again = src.area_tree(2026, CA_2026).await.unwrap().unwrap();
        assert_eq!(again.regions.len(), tree.regions.len());
        src.season_area_tree(2026, CA_2026).await.unwrap();
        assert_eq!(src.fetcher.sent.lock().unwrap().len(), 2);
    }

    /// The national tree as a Maine list's token answers it: every State in
    /// `regions` (Maine, 900_019, without its Classes; California's Central
    /// Coast Section, as the live answer has), and Maine's own subtree,
    /// starting at `root`, in `tree`: Class A, or nothing below Maine yet
    /// when `classes` is false.
    fn me_nav_with(root: i64, classes: bool) -> FetchResponse {
        let mut regions = vec![
            serde_json::json!({"id": 1, "name": "High School", "state": null, "divType": "Level", "depth": 0}),
            serde_json::json!({"id": 900_004 + 21, "name": " Central Coast", "state": "CA", "divType": "Section", "depth": 2}),
        ];
        regions.extend(crate::area::US_STATES.iter().zip(0..).map(|(st, i)| {
            serde_json::json!({"id": 900_000 + i, "name": st, "state": st, "divType": "State", "depth": 1})
        }));
        ok_json(
            &serde_json::json!({
                "divListId": root,
                "seasons": {"2026": root},
                "regions": regions,
                "tree": if classes {
                    serde_json::json!([
                        {"id": root, "name": "Maine", "state": "ME", "divType": "State", "depth": 0},
                        {"id": root + 1, "name": "A", "state": "ME", "divType": "Class", "depth": 1},
                    ])
                } else {
                    serde_json::json!([
                        {"id": root, "name": "Maine", "state": "ME", "divType": "State", "depth": 0},
                    ])
                },
            })
            .to_string(),
        )
    }

    fn me_nav(root: i64) -> FetchResponse {
        me_nav_with(root, true)
    }

    const ME_2026: i64 = 900_019;

    #[tokio::test]
    async fn a_states_own_tree_takes_two_requests_then_none() {
        let dir = tempfile::tempdir().unwrap();
        let stub = StubFetcher::new(vec![token("tok-me", ME_2026, 2026), me_nav(ME_2026)]);
        let src = AthleticNet::new(stub).with_cache(cache_in(dir.path(), CacheMode::Normal));
        let tree = src.state_tree(2026, ME_2026).await.unwrap();
        let ids: Vec<i64> = tree.iter().map(|r| r.id).collect();
        assert_eq!(ids, [ME_2026, ME_2026 + 1]);
        {
            let sent = src.fetcher.sent.lock().unwrap();
            assert_eq!(sent.len(), 2);
            assert_eq!(body_json(&sent[0])["divListId"], ME_2026);
            assert!(
                sent[1].url().ends_with(&nav_req_ends(2026)),
                "{}",
                sent[1].url()
            );
            assert_eq!(header(&sent[1], "anettokens"), Some("tok-me"));
        }
        // Cached under its own key, and the national tree that came with it
        // as the season's: nothing more is sent for either.
        assert_eq!(src.state_tree(2026, ME_2026).await.unwrap().len(), 2);
        let national = src.season_area_tree(2026, ME_2026).await.unwrap();
        assert!(national.national);
        assert!(
            national.areas("ME", "Class").is_empty(),
            "the subtree stays apart"
        );
        assert_eq!(src.fetcher.sent.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_states_tree_that_starts_elsewhere_is_an_error_and_not_kept() {
        let dir = tempfile::tempdir().unwrap();
        let stub = StubFetcher::new(vec![
            token("tok-me", ME_2026, 2026),
            me_nav(42),
            token("tok-me", ME_2026, 2026),
            me_nav(ME_2026),
        ]);
        let src = AthleticNet::new(stub).with_cache(cache_in(dir.path(), CacheMode::Normal));
        let err = src.state_tree(2026, ME_2026).await.unwrap_err().to_string();
        assert!(err.contains("does not start at that list"), "{err}");
        assert_eq!(src.state_tree(2026, ME_2026).await.unwrap().len(), 2);
        assert_eq!(src.fetcher.sent.lock().unwrap().len(), 4, "asked again");
    }

    #[tokio::test]
    async fn a_state_lists_own_tree_is_kept_from_any_read_of_it() {
        // A Maine Profile's Area reads the season's tree through Maine's list:
        // its answer already holds Maine's subtree, so completing Maine's
        // Areas afterwards sends nothing, in this run or the next.
        let dir = tempfile::tempdir().unwrap();
        let stub = StubFetcher::new(vec![token("tok-me", ME_2026, 2026), me_nav(ME_2026)]);
        let src = AthleticNet::new(stub).with_cache(cache_in(dir.path(), CacheMode::Normal));
        src.season_area_tree(2026, ME_2026).await.unwrap();
        assert_eq!(src.state_tree(2026, ME_2026).await.unwrap().len(), 2);
        assert_eq!(src.fetcher.sent.lock().unwrap().len(), 2);
        let later = AthleticNet::new(StubFetcher::new(vec![]))
            .with_cache(cache_in(dir.path(), CacheMode::Normal));
        assert_eq!(later.state_tree(2026, ME_2026).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_states_tree_with_nothing_below_it_is_not_kept() {
        let dir = tempfile::tempdir().unwrap();
        let stub = StubFetcher::new(vec![
            token("tok-me", ME_2026, 2026),
            me_nav_with(ME_2026, false),
        ]);
        let src = AthleticNet::new(stub).with_cache(cache_in(dir.path(), CacheMode::Normal));
        assert_eq!(src.state_tree(2026, ME_2026).await.unwrap().len(), 1);
        // A later run (a fresh source on the same cache) asks again and finds
        // the Classes Athletic.net added since.
        let later = AthleticNet::new(StubFetcher::new(vec![
            token("tok-me", ME_2026, 2026),
            me_nav(ME_2026),
        ]))
        .with_cache(cache_in(dir.path(), CacheMode::Normal));
        assert_eq!(later.state_tree(2026, ME_2026).await.unwrap().len(), 2);
        assert_eq!(later.fetcher.sent.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_states_tree_is_read_once_a_run_without_a_cache() {
        let stub = StubFetcher::new(vec![token("tok-me", ME_2026, 2026), me_nav(ME_2026)]);
        let src = AthleticNet::new(stub);
        src.state_tree(2026, ME_2026).await.unwrap();
        assert_eq!(src.state_tree(2026, ME_2026).await.unwrap().len(), 2);
        assert_eq!(src.fetcher.sent.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_states_tree_from_a_list_in_another_season_is_an_error() {
        let stub = StubFetcher::new(vec![token("tok-me", ME_2026, 2025)]);
        let src = AthleticNet::new(stub);
        let err = src.state_tree(2026, ME_2026).await.unwrap_err().to_string();
        assert!(err.contains("is in season 2025, not 2026"), "{err}");
    }

    #[tokio::test]
    async fn area_tree_hops_from_the_start_lists_own_season() {
        let seasons = serde_json::json!({"2025": CA_2025, "2026": CA_2026});
        let stub = StubFetcher::new(vec![
            token("tok-a", CA_2026, 2026),
            nav(seasons.clone(), CA_2026),
            token("tok-b", CA_2025, 2025),
            nav(seasons, CA_2025),
        ]);
        let src = AthleticNet::new(stub);
        let tree = src.area_tree(2025, CA_2026).await.unwrap().unwrap();
        assert_eq!(tree.season, 2025);
        assert_eq!(tree.state("CA").unwrap().id, CA_2025);
        let sent = src.fetcher.sent.lock().unwrap();
        assert_eq!(sent.len(), 4);
        assert_eq!(body_json(&sent[0])["divListId"], CA_2026);
        // The hop reads the start list's own season (2026) with its token,
        // then follows `seasons[2025]`.
        assert!(
            sent[1].url().ends_with(&nav_req_ends(2026)),
            "{}",
            sent[1].url()
        );
        assert_eq!(header(&sent[1], "anettokens"), Some("tok-a"));
        assert_eq!(body_json(&sent[2])["divListId"], CA_2025);
        assert!(
            sent[3].url().ends_with(&nav_req_ends(2025)),
            "{}",
            sent[3].url()
        );
        assert_eq!(header(&sent[3], "anettokens"), Some("tok-b"));
    }

    #[tokio::test]
    async fn the_hop_never_reads_seasons_from_a_cached_tree() {
        // 2026's tree was cached before 2027 existed; asking for 2027 must
        // still read today's `seasons`.
        let dir = tempfile::tempdir().unwrap();
        let seeded = AthleticNet::new(StubFetcher::new(vec![
            token("tok-a", CA_2026, 2026),
            nav(serde_json::json!({"2026": CA_2026}), CA_2026),
        ]))
        .with_cache(cache_in(dir.path(), CacheMode::Normal));
        seeded.area_tree(2026, CA_2026).await.unwrap().unwrap();

        let src = AthleticNet::new(StubFetcher::new(vec![
            token("tok-a", CA_2026, 2026),
            nav(
                serde_json::json!({"2026": CA_2026, "2027": 180_000}),
                CA_2026,
            ),
            token("tok-c", 180_000, 2027),
            nav(serde_json::json!({}), 180_000),
        ]))
        .with_cache(cache_in(dir.path(), CacheMode::Normal));
        let tree = src.area_tree(2027, CA_2026).await.unwrap().unwrap();
        assert_eq!(tree.state("CA").unwrap().id, 180_000);
    }

    #[tokio::test]
    async fn a_missing_season_is_none_and_remembered_for_a_few_hours() {
        let dir = tempfile::tempdir().unwrap();
        let at = |secs: u64| -> crate::cache::Clock {
            std::sync::Arc::new(move || std::time::UNIX_EPOCH + Duration::from_secs(secs))
        };
        let t0 = 1_780_000_000;
        let cache = |now| cache_in(dir.path(), CacheMode::Normal).with_clock(at(now));
        let answers = || {
            vec![
                token("tok-a", CA_2026, 2026),
                nav(serde_json::json!({"2026": CA_2026}), CA_2026),
            ]
        };
        let src = AthleticNet::new(StubFetcher::new(answers())).with_cache(cache(t0));
        assert!(src.area_tree(2028, CA_2026).await.unwrap().is_none());
        // Within six hours: answered from the cache (the stub is empty now).
        let soon = AthleticNet::new(StubFetcher::new(vec![])).with_cache(cache(t0 + 3600));
        assert!(soon.area_tree(2028, CA_2026).await.unwrap().is_none());
        // After that: asked again.
        let later =
            AthleticNet::new(StubFetcher::new(answers())).with_cache(cache(t0 + 6 * 3600 + 1));
        assert!(later.area_tree(2028, CA_2026).await.unwrap().is_none());
        assert_eq!(later.fetcher.sent.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_missing_season_marker_only_answers_for_its_own_start_list() {
        const CA_2028: i64 = 190_000;
        let dir = tempfile::tempdir().unwrap();
        let normal = || cache_in(dir.path(), CacheMode::Normal);
        let seeded = AthleticNet::new(StubFetcher::new(vec![
            token("tok-a", CA_2026, 2026),
            nav(serde_json::json!({"2026": CA_2026}), CA_2026),
        ]))
        .with_cache(normal());
        assert!(seeded.area_tree(2028, CA_2026).await.unwrap().is_none());

        // Another start list, itself in 2028: the 2026 list's evidence says
        // nothing about it.
        let src = AthleticNet::new(StubFetcher::new(vec![
            token("tok-b", CA_2028, 2028),
            nav(serde_json::json!({"2028": CA_2028}), CA_2028),
        ]))
        .with_cache(normal());
        let tree = src.area_tree(2028, CA_2028).await.unwrap().unwrap();
        assert_eq!(tree.state("CA").unwrap().id, CA_2028);

        // A school's chain (`season_area_tree`) never reads a marker.
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(dir.path(), CacheMode::Normal);
        let marker = r#"{"ath_missing_season":true,"start_season":2025}"#;
        cache
            .put(
                &missing_season_request(2026, CA_2025),
                DataKind::MissingSeason,
                marker,
            )
            .unwrap();
        let src = AthleticNet::new(StubFetcher::new(vec![
            token("tok-a", CA_2026, 2026),
            nav(serde_json::json!({}), CA_2026),
        ]))
        .with_cache(cache);
        let tree = src.season_area_tree(2026, CA_2026).await.unwrap();
        assert_eq!(tree.state("CA").unwrap().id, CA_2026);
    }

    #[tokio::test]
    async fn a_marker_from_a_list_in_the_season_is_never_honored() {
        // Evidence that cannot be right (the start list is in the season
        // itself) is ignored, and the list leads to the tree.
        let dir = tempfile::tempdir().unwrap();
        let cache = cache_in(dir.path(), CacheMode::Normal);
        cache
            .put(
                &missing_season_request(2026, CA_2026),
                DataKind::MissingSeason,
                r#"{"ath_missing_season":true,"start_season":2026}"#,
            )
            .unwrap();
        let src = AthleticNet::new(StubFetcher::new(vec![
            token("tok-a", CA_2026, 2026),
            nav(serde_json::json!({"2026": CA_2026}), CA_2026),
        ]))
        .with_cache(cache);
        assert!(src.area_tree(2026, CA_2026).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn a_start_list_without_a_season_hops_through_the_season_asked_for() {
        // As before rankings were read for their season: `GetNavInfo` for the
        // season asked for, with the start list's token, then its `seasons`.
        let stub = StubFetcher::new(vec![
            token_without_season("tok-a", CA_2026),
            nav(serde_json::json!({"2025": CA_2025}), CA_2026),
            token_without_season("tok-b", CA_2025),
            nav(serde_json::json!({"2025": CA_2025}), CA_2025),
        ]);
        let src = AthleticNet::new(stub);
        let tree = src.area_tree(2025, CA_2026).await.unwrap().unwrap();
        assert_eq!(tree.state("CA").unwrap().id, CA_2025);
        let sent = src.fetcher.sent.lock().unwrap();
        assert_eq!(sent.len(), 4);
        assert!(sent[1].url().ends_with(&nav_req_ends(2025)));
        assert_eq!(header(&sent[1], "anettokens"), Some("tok-a"));
        assert_eq!(header(&sent[3], "anettokens"), Some("tok-b"));
    }

    #[tokio::test]
    async fn a_start_list_without_a_season_finds_no_such_season_and_forgets_it() {
        let dir = tempfile::tempdir().unwrap();
        let src = AthleticNet::new(StubFetcher::new(vec![
            token_without_season("tok-a", CA_2026),
            ok_json(r#"{"seasons": {"2026": 900500}}"#),
        ]))
        .with_cache(cache_in(dir.path(), CacheMode::Normal));
        assert!(src.area_tree(2028, CA_2026).await.unwrap().is_none());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn a_list_that_does_not_say_its_season_is_taken_at_its_word() {
        let src = AthleticNet::new(StubFetcher::new(vec![
            token_without_season("tok", CA_2026),
            nav(serde_json::json!({}), CA_2026),
        ]));
        let tree = src.season_area_tree(2026, CA_2026).await.unwrap();
        assert_eq!(tree.state("CA").unwrap().id, CA_2026);
    }

    #[tokio::test]
    async fn only_the_national_tree_is_cached() {
        let dir = tempfile::tempdir().unwrap();
        let partial = || {
            vec![
                token("tok-a", CA_2026, 2026),
                partial_nav(serde_json::json!({"2026": CA_2026}), CA_2026),
            ]
        };
        let src = AthleticNet::new(StubFetcher::new([partial(), partial()].concat()))
            .with_cache(cache_in(dir.path(), CacheMode::Normal));
        // Used for the answer, but not kept: the second ask fetches again.
        for _ in 0..2 {
            let tree = src.area_tree(2026, CA_2026).await.unwrap().unwrap();
            assert!(!tree.national);
            assert_eq!(tree.state("CA").unwrap().id, CA_2026);
        }
        assert_eq!(src.fetcher.sent.lock().unwrap().len(), 4);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);

        // A partial tree already in the cache (written before this rule) is
        // a miss.
        let cache = cache_in(dir.path(), CacheMode::Normal);
        cache
            .put(
                &tree_request(2026),
                DataKind::AreaTree,
                &partial_nav(serde_json::json!({}), CA_2026).body,
            )
            .unwrap();
        let src = AthleticNet::new(StubFetcher::new(vec![
            token("tok-a", CA_2026, 2026),
            nav(serde_json::json!({}), CA_2026),
        ]))
        .with_cache(cache);
        assert!(src.season_area_tree(2026, CA_2026).await.unwrap().national);
        assert_eq!(src.fetcher.sent.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_list_in_another_season_is_refused() {
        let src = AthleticNet::new(StubFetcher::new(vec![token("tok", CA_2025, 2025)]));
        let err = src.season_area_tree(2026, CA_2025).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("is in season 2025, not 2026"),
            "{err:#}"
        );
    }

    #[tokio::test]
    async fn an_unreadable_start_list_says_so() {
        let src = AthleticNet::new(StubFetcher::new(vec![FetchResponse {
            status: 404,
            body: "<html>Not found</html>".into(),
        }]));
        let err = src.area_tree(2026, 42).await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("rankings list to start from (42, see --from-list)"),
            "{msg}"
        );
    }

    #[tokio::test]
    async fn a_tree_without_states_is_an_error_and_not_cached() {
        let dir = tempfile::tempdir().unwrap();
        let src = AthleticNet::new(StubFetcher::new(vec![
            token("tok-a", CA_2026, 2026),
            ok_json(r#"{"seasons": {"2026": 900500}, "regions": []}"#),
        ]))
        .with_cache(cache_in(dir.path(), CacheMode::Normal));
        let err = src.area_tree(2026, CA_2026).await.unwrap_err();
        assert!(format!("{err:#}").contains("lists no States"), "{err:#}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    const TEAMNAV: &str = r#"{
        "team": {"ID": 5103, "Name": "Seacliff Prep", "Level": 4, "City": "San Francisco", "State": "CA"},
        "divisions": [{"id": 900500, "b": 903, "name": "California"}, {"id": 900502, "b": 904, "name": "Central Coast"}]
    }"#;

    /// The kind `team_divisions` cached its one entry as.
    async fn team_divisions_kind(season: i64) -> serde_json::Value {
        let dir = tempfile::tempdir().unwrap();
        let src = AthleticNet::new(StubFetcher::new(vec![ok_json(TEAMNAV)]))
            .with_cache(cache_in(dir.path(), CacheMode::Normal))
            .with_today("2026-10-26".parse().unwrap());
        let nav = src.team_divisions(5103, season).await.unwrap();
        assert_eq!(nav.divisions[1].b, Some(904));
        {
            let sent = src.fetcher.sent.lock().unwrap();
            assert!(
                sent[0]
                    .url()
                    .ends_with(&format!("/TeamNav/Team?team=5103&sport=tf&season={season}")),
                "{}",
                sent[0].url()
            );
        }
        let file = std::fs::read_dir(dir.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        let entry: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(file.path()).unwrap()).unwrap();
        entry["kind"].clone()
    }

    #[tokio::test]
    async fn team_divisions_are_cached_by_whether_the_season_is_over() {
        assert_eq!(
            team_divisions_kind(2026).await,
            serde_json::json!({"team_divisions": {"season_finished": true}})
        );
        assert_eq!(
            team_divisions_kind(2027).await,
            serde_json::json!({"team_divisions": {"season_finished": false}})
        );
    }

    const PAGE: &str = r#"{"eventShort": "400m", "division": {"ID": 900502, "SeasonID": 2026},
        "minCount": 1, "groupedRankings": [[{"AthleteID": 7, "AthleteName": "A", "display": "55.48",
        "SortIntRaw": 55480, "SortIntCalc": 55480, "SortIntOrig": 55480, "rank": 1}]]}"#;

    #[tokio::test]
    async fn rankings_pages_are_asked_for_like_athletic_nets_own_page() {
        let src = AthleticNet::new(StubFetcher::new(vec![ok_json(PAGE); 3]));
        let all = Grades::all();
        let grades = Grades::parse("9-10").unwrap();
        let page = |grades, page| PageQuery {
            list_id: 900_502,
            season: 2026,
            gender: Gender::M,
            event: "Shot",
            grades,
            page,
        };
        let first = src.rankings_page(&page(&all, 1)).await.unwrap();
        assert_eq!(first.grouped_rankings[0][0].sort_int, 55480);
        src.rankings_page(&page(&grades, 1)).await.unwrap();
        src.rankings_page(&page(&grades, 3)).await.unwrap();
        let sent = src.fetcher.sent.lock().unwrap();
        assert!(sent.iter().all(|r| r.method() == Method::Post
            && r.url().ends_with("/tfRankings/GetRankings")));
        let bodies: Vec<serde_json::Value> = sent.iter().map(body_json).collect();
        assert_eq!(bodies[0]["divListId"], 900_502);
        assert_eq!(bodies[0]["eventShort"], "Shot");
        assert_eq!(
            (&bodies[0]["gender"], &bodies[0]["mode"]),
            (&"m".into(), &"m".into())
        );
        // No filter for every grade, and no page number on the first page.
        assert_eq!(bodies[0]["qParams"], serde_json::json!({}));
        assert_eq!(bodies[1]["qParams"], serde_json::json!({"grades": [9, 10]}));
        assert_eq!(
            bodies[2]["qParams"],
            serde_json::json!({"grades": [9, 10], "page": 3})
        );
    }

    #[tokio::test]
    async fn rankings_pages_are_cached_by_whether_the_season_is_over() {
        for (season, finished) in [(2026, true), (2027, false)] {
            let dir = tempfile::tempdir().unwrap();
            let src = AthleticNet::new(StubFetcher::new(vec![ok_json(PAGE)]))
                .with_cache(cache_in(dir.path(), CacheMode::Normal))
                .with_today("2026-10-26".parse().unwrap());
            let grades = Grades::all();
            let q = PageQuery {
                list_id: 900_502,
                season,
                gender: Gender::F,
                event: "400m",
                grades: &grades,
                page: 1,
            };
            src.rankings_page(&q).await.unwrap();
            // Served from the cache: the stub has nothing left.
            src.rankings_page(&q).await.unwrap();
            let file = std::fs::read_dir(dir.path())
                .unwrap()
                .next()
                .unwrap()
                .unwrap();
            let entry: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(file.path()).unwrap()).unwrap();
            assert_eq!(
                entry["kind"],
                serde_json::json!({"rankings": {"season_finished": finished}})
            );
        }
    }

    /// A page as a session without a paid subscription gets it: a made-up
    /// person, then a Masked row.
    const CAPPED_PAGE: &str = r#"{"eventShort": "400m", "division": {"ID": 900502, "SeasonID": 2026},
        "minCount": 696, "groupedRankings": [[
        {"AthleteID": 940000001, "AthleteName": "Wren Halloway", "display": "53.10", "SortIntCalc": 53100, "rank": 1},
        {"AthleteID": 0, "AthleteName": "Xxxxxx Xxxxxx", "display": "", "SortIntCalc": 55400, "rank": 2,
         "TeamID": 0, "MeetID": 0, "GradeID": 99}]]}"#;

    #[tokio::test]
    async fn a_capped_page_is_never_cached() {
        let dir = tempfile::tempdir().unwrap();
        let src = AthleticNet::new(StubFetcher::new(vec![
            ok_json(CAPPED_PAGE),
            ok_json(CAPPED_PAGE),
        ]))
        .with_cache(cache_in(dir.path(), CacheMode::Normal))
        .with_today("2026-10-26".parse().unwrap());
        // A finished season, which would otherwise be cached forever.
        let q = ccs_page(1);
        assert!(src.rankings_page(&q).await.unwrap().is_capped_after(0));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        // So the next read asks Athletic.net again.
        src.rankings_page(&q).await.unwrap();
        assert_eq!(src.fetcher.sent.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_capped_page_already_in_the_cache_is_a_miss() {
        // Cached by an ath from before Masked rows were known: a later run
        // with a paid subscription must not be served it.
        let dir = tempfile::tempdir().unwrap();
        let q = ccs_page(1);
        let req = rankings_request(q.list_id, q.gender, q.event, q.grades, q.page).unwrap();
        let kind = DataKind::Rankings {
            season_finished: true,
        };
        cache_in(dir.path(), CacheMode::Normal)
            .put(&req, kind, CAPPED_PAGE)
            .unwrap();
        let src = AthleticNet::new(StubFetcher::new(vec![ok_json(PAGE)]))
            .with_cache(cache_in(dir.path(), CacheMode::Normal))
            .with_today("2026-10-26".parse().unwrap());
        let page = src.rankings_page(&q).await.unwrap();
        assert!(!page.is_capped_after(0));
        assert_eq!(src.fetcher.sent.lock().unwrap().len(), 1);
        // The whole page replaced it, and is served from the cache now.
        src.rankings_page(&q).await.unwrap();
        assert_eq!(src.fetcher.sent.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_capped_page_in_the_cache_is_deleted_even_if_the_answer_is_capped_again() {
        let dir = tempfile::tempdir().unwrap();
        let q = ccs_page(1);
        let req = rankings_request(q.list_id, q.gender, q.event, q.grades, q.page).unwrap();
        let kind = DataKind::Rankings {
            season_finished: true,
        };
        cache_in(dir.path(), CacheMode::Normal)
            .put(&req, kind, CAPPED_PAGE)
            .unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        let src = AthleticNet::new(StubFetcher::new(vec![ok_json(CAPPED_PAGE)]))
            .with_cache(cache_in(dir.path(), CacheMode::Normal))
            .with_today("2026-10-26".parse().unwrap());
        assert!(src.rankings_page(&q).await.unwrap().is_capped_after(0));
        assert_eq!(src.fetcher.sent.lock().unwrap().len(), 1);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    fn ccs_page(page: u32) -> PageQuery<'static> {
        static ALL: std::sync::LazyLock<Grades> = std::sync::LazyLock::new(Grades::all);
        PageQuery {
            list_id: 900_502,
            season: 2026,
            gender: Gender::F,
            event: "400m",
            grades: &ALL,
            page,
        }
    }

    // The paced tests run on tokio's paused clock: sleeps advance virtual
    // time at once, so timings are exact and the tests do not wait.

    #[tokio::test(start_paused = true)]
    async fn fetched_pages_are_paced_and_cached_ones_are_not() {
        let dir = tempfile::tempdir().unwrap();
        let delay = Duration::from_millis(300);
        let src = AthleticNet::new(StubFetcher::new(vec![ok_json(PAGE); 2]))
            .with_cache(cache_in(dir.path(), CacheMode::Normal))
            .with_page_delay(delay);
        let start = tokio::time::Instant::now();
        src.rankings_page(&ccs_page(1)).await.unwrap();
        assert_eq!(
            start.elapsed(),
            Duration::ZERO,
            "the first page never waits"
        );
        src.rankings_page(&ccs_page(2)).await.unwrap();
        assert_eq!(start.elapsed(), delay);
        // From the cache: no waiting (and nothing left in the stub).
        let start = tokio::time::Instant::now();
        src.rankings_page(&ccs_page(1)).await.unwrap();
        src.rankings_page(&ccs_page(2)).await.unwrap();
        assert_eq!(start.elapsed(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn the_token_request_is_paced_with_the_pages() {
        let delay = Duration::from_millis(300);
        let src = AthleticNet::new(StubFetcher::new(vec![
            token("tok", CA_2026, 2026),
            ok_json(PAGE),
        ]))
        .with_page_delay(delay);
        let start = tokio::time::Instant::now();
        src.rankings_token(CA_2026).await.unwrap();
        src.rankings_page(&ccs_page(1)).await.unwrap();
        assert_eq!(start.elapsed(), delay);
    }

    #[tokio::test(start_paused = true)]
    async fn a_pushed_back_page_slows_the_rest_of_the_run() {
        let src = AthleticNet::new(StubFetcher::new(vec![
            answer(200, ""),
            challenge(),
            ok_json(PAGE),
            ok_json(PAGE),
            ok_json(PAGE),
        ]))
        .with_retry(instant_retry())
        .with_page_delay(Duration::from_millis(250));
        src.rankings_page(&ccs_page(1)).await.unwrap();
        assert_eq!(src.pages.lock().await.delay, SLOWED_PAGE_DELAY);
        // A clean page keeps the slower pace: never back to fast.
        let start = tokio::time::Instant::now();
        src.rankings_page(&ccs_page(2)).await.unwrap();
        src.rankings_page(&ccs_page(3)).await.unwrap();
        assert_eq!(start.elapsed(), SLOWED_PAGE_DELAY * 2);
    }

    #[tokio::test(start_paused = true)]
    async fn pushback_on_other_requests_does_not_slow_the_pages() {
        let src = AthleticNet::new(StubFetcher::new(vec![answer(200, ""), ok_json(BIO_JSON)]))
            .with_retry(instant_retry())
            .with_page_delay(Duration::from_millis(250));
        src.get_bio(1001).await.unwrap();
        assert_eq!(src.pages.lock().await.delay, Duration::from_millis(250));
    }

    #[test]
    fn the_pacer_doubles_up_to_its_ceiling() {
        let mut p = Pacer::new(Duration::from_millis(250));
        p.done(false);
        assert_eq!(p.delay, Duration::from_millis(250));
        for _ in 0..10 {
            p.done(true);
        }
        assert_eq!(p.delay, MAX_PAGE_DELAY);
    }

    #[tokio::test]
    async fn search_is_never_cached() {
        let dir = tempfile::tempdir().unwrap();
        let json = r#"{"response":{"docs":[]}}"#;
        let src = AthleticNet::new(StubFetcher::new(vec![ok_json(json), ok_json(json)]))
            .with_cache(cache_in(dir.path(), CacheMode::Normal));
        src.search("Jamie Uddumcombe").await.unwrap();
        src.search("Jamie Uddumcombe").await.unwrap();
        assert_eq!(src.fetcher.sent.lock().unwrap().len(), 2);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    const MEET_JSON: &str = r#"{"meet": {"ID": 900700, "Name": "Test Invite",
        "MeetDate": "2099-01-09T00:00:00", "SeasonID": 12099},
        "tfDivisions": [{"IDMeetDiv": 1, "MeetID": 900700, "IDDiv": 13, "Division": "Varsity", "DivisionShort": "V"}],
        "jwtMeet": "SECRET-MEET-TOKEN"}"#;
    const EVENTS_JSON: &str = r#"{"events": [{"ID": 4, "Event": "800 Meters", "EventShort": "800m", "Gender": "M", "Type": "T"}],
        "eventDivsWithEntries": [{"e": 4, "d": 13, "ge_p": null}], "eventDivsWithResults": []}"#;
    const ENTRIES_JSON: &str = r#"{"eventId": 4, "currentEventValid": true, "entries": [
        {"IDAthlete": 900100001, "FirstName": "Milo", "LastName": "Marsh", "AcceptanceStatus": 10, "Scratched": false}],
        "teams": [], "releyLegs": []}"#;

    fn entries_q() -> EntriesQuery<'static> {
        EntriesQuery {
            meet_id: 900700,
            gender: Gender::M,
            div_id: 13,
            event_short: "800m",
        }
    }

    #[tokio::test]
    async fn a_meets_events_and_entries_are_read_with_its_token() {
        let src = AthleticNet::new(StubFetcher::new(vec![
            ok_json(MEET_JSON),
            ok_json(EVENTS_JSON),
            ok_json(ENTRIES_JSON),
        ]));
        let meet = src.meet(900700).await.unwrap();
        assert_eq!(meet.meet.name, "Test Invite");
        assert_eq!(src.meet_events(900700).await.unwrap().events.len(), 1);
        let entries = src.meet_entries(&entries_q()).await.unwrap();
        assert_eq!(entries.entries[0].athlete_id, Some(900100001));

        let sent = src.fetcher.sent.lock().unwrap();
        assert_eq!(sent.len(), 3, "the token is reused, not fetched again");
        assert!(
            sent[0]
                .url()
                .ends_with("/Meet/GetMeetData?meetId=900700&sport=tf"),
            "{}",
            sent[0].url()
        );
        assert_eq!(header(&sent[0], "anettokens"), None);
        assert!(
            sent[1]
                .url()
                .ends_with("/Meet/GetEventListData?meetId=900700")
        );
        assert_eq!(header(&sent[1], "anettokens"), Some("SECRET-MEET-TOKEN"));
        assert_eq!(sent[2].method(), Method::Post);
        assert!(sent[2].url().ends_with("/Meet/GetEntriesData"));
        assert_eq!(header(&sent[2], "anettokens"), Some("SECRET-MEET-TOKEN"));
        assert_eq!(
            body_json(&sent[2]),
            serde_json::json!({"meetId": 900700, "sport": "tf", "gender": "m", "divId": 13,
                               "eventShort": "800m", "seasonId": 0})
        );
    }

    #[tokio::test]
    async fn the_event_list_fetches_the_token_when_the_run_has_none() {
        let src = AthleticNet::new(StubFetcher::new(vec![
            ok_json(MEET_JSON),
            ok_json(EVENTS_JSON),
        ]));
        src.meet_events(900700).await.unwrap();
        let sent = src.fetcher.sent.lock().unwrap();
        assert!(sent[0].url().contains("/Meet/GetMeetData"));
        assert_eq!(header(&sent[1], "anettokens"), Some("SECRET-MEET-TOKEN"));
    }

    #[tokio::test]
    async fn a_meet_without_a_token_says_its_entries_cannot_be_read() {
        let no_token = MEET_JSON.replace("\"SECRET-MEET-TOKEN\"", "null");
        let src = AthleticNet::new(StubFetcher::new(vec![ok_json(&no_token)]));
        let err = format!("{:#}", src.meet_events(900700).await.unwrap_err());
        assert!(err.contains("no jwtMeet token for meet 900700"), "{err}");
    }

    #[tokio::test]
    async fn the_cache_keeps_a_meet_without_its_token() {
        let dir = tempfile::tempdir().unwrap();
        let src = AthleticNet::new(StubFetcher::new(vec![
            ok_json(MEET_JSON),
            ok_json(EVENTS_JSON),
            ok_json(ENTRIES_JSON),
        ]))
        .with_cache(cache_in(dir.path(), CacheMode::Normal));
        src.meet(900700).await.unwrap();
        src.meet_events(900700).await.unwrap();
        src.meet_entries(&entries_q()).await.unwrap();
        let mut entries = 0;
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            assert!(!text.contains("SECRET-MEET-TOKEN"), "{text}");
            assert!(text.contains("\"meet_entries\""), "{text}");
            entries += 1;
        }
        assert_eq!(entries, 3);

        // The next run reads all three from the cache, sending nothing.
        let cached = AthleticNet::new(StubFetcher::new(vec![]))
            .with_cache(cache_in(dir.path(), CacheMode::Normal));
        assert_eq!(cached.meet(900700).await.unwrap().meet.id, 900700);
        cached.meet_events(900700).await.unwrap();
        cached.meet_entries(&entries_q()).await.unwrap();

        // Entries not in the cache need the token, which the cached meet
        // lacks: GetMeetData is sent again for it.
        let other = EntriesQuery {
            div_id: 14,
            ..entries_q()
        };
        let fresh = AthleticNet::new(StubFetcher::new(vec![
            ok_json(MEET_JSON),
            ok_json(ENTRIES_JSON),
        ]))
        .with_cache(cache_in(dir.path(), CacheMode::Normal));
        fresh.meet(900700).await.unwrap();
        fresh.meet_entries(&other).await.unwrap();
        let sent = fresh.fetcher.sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert!(sent[0].url().contains("/Meet/GetMeetData"));
        assert_eq!(header(&sent[1], "anettokens"), Some("SECRET-MEET-TOKEN"));
    }

    const DENIED: &str = r#"{"Message":"Authorization has been denied for this request."}"#;

    #[tokio::test]
    async fn a_refused_token_is_fetched_again_once() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = MEET_JSON.replace("SECRET-MEET-TOKEN", "FRESH-MEET-TOKEN");
        let src = AthleticNet::new(StubFetcher::new(vec![
            ok_json(MEET_JSON),
            answer(403, DENIED),
            ok_json(&fresh),
            ok_json(EVENTS_JSON),
        ]))
        .with_cache(cache_in(dir.path(), CacheMode::Normal));
        let list = src.meet_events(900700).await.unwrap();
        assert_eq!(list.events.len(), 1);
        let sent = src.fetcher.sent.lock().unwrap();
        assert_eq!(sent.len(), 4);
        assert_eq!(header(&sent[1], "anettokens"), Some("SECRET-MEET-TOKEN"));
        assert!(
            sent[2].url().contains("/Meet/GetMeetData"),
            "the token again"
        );
        assert_eq!(header(&sent[3], "anettokens"), Some("FRESH-MEET-TOKEN"));
    }

    #[tokio::test]
    async fn a_second_refusal_fails_and_mentions_a_paid_subscription() {
        let dir = tempfile::tempdir().unwrap();
        // A 401, then an error object in a 200: both are refusals.
        let src = AthleticNet::new(StubFetcher::new(vec![
            ok_json(MEET_JSON),
            answer(401, DENIED),
            ok_json(MEET_JSON),
            ok_json(DENIED),
        ]))
        .with_cache(cache_in(dir.path(), CacheMode::Normal));
        let err = format!("{:#}", src.meet_entries(&entries_q()).await.unwrap_err());
        assert!(err.contains("Athletic.net refused"), "{err}");
        assert!(
            err.contains("HTTP 200: Authorization has been denied"),
            "{err}"
        );
        assert!(err.contains("paid Athletic.net subscription"), "{err}");
        assert_eq!(src.fetcher.sent.lock().unwrap().len(), 4, "one retry only");
        // Only the two GetMeetData answers were cached, never a refusal.
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            assert!(text.contains("GetMeetData"), "{text}");
        }
    }

    #[tokio::test]
    async fn an_error_status_or_a_wrong_shape_is_an_error_and_never_cached() {
        for (resp, want) in [
            (
                answer(500, r#"{"Message":"An error has occurred."}"#),
                "HTTP 500: An error has occurred.",
            ),
            (ok_json(r#"{"eventDivsWithEntries": []}"#), "no `events`"),
            (ok_json(r#"{"events": null}"#), "no `events`"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let src = AthleticNet::new(StubFetcher::new(vec![ok_json(MEET_JSON), resp]))
                .with_cache(cache_in(dir.path(), CacheMode::Normal));
            let err = format!("{:#}", src.meet_events(900700).await.unwrap_err());
            assert!(err.contains(want), "{err}");
            assert_eq!(src.fetcher.sent.lock().unwrap().len(), 2, "not retried");
            assert_eq!(
                std::fs::read_dir(dir.path()).unwrap().count(),
                1,
                "only the meet"
            );
        }
    }

    #[tokio::test]
    async fn another_error_object_is_a_plain_error_without_a_token_refetch() {
        let src = AthleticNet::new(StubFetcher::new(vec![
            ok_json(MEET_JSON),
            ok_json(r#"{"Message":"An error has occurred."}"#),
        ]));
        let err = format!("{:#}", src.meet_events(900700).await.unwrap_err());
        assert_eq!(
            err,
            "Athletic.net answered the event list of meet 900700 with an error: An error has \
             occurred."
        );
        assert_eq!(
            src.fetcher.sent.lock().unwrap().len(),
            2,
            "no token refetch"
        );
    }

    #[tokio::test]
    async fn a_meet_read_without_a_token_is_not_asked_again() {
        let no_token = MEET_JSON.replace("\"SECRET-MEET-TOKEN\"", "null");
        let src = AthleticNet::new(StubFetcher::new(vec![ok_json(&no_token)]));
        src.meet(900700).await.unwrap();
        for _ in 0..2 {
            let err = format!("{:#}", src.meet_events(900700).await.unwrap_err());
            assert!(err.contains("no jwtMeet token for meet 900700"), "{err}");
        }
        assert_eq!(
            src.fetcher.sent.lock().unwrap().len(),
            1,
            "GetMeetData once"
        );
    }

    #[tokio::test]
    async fn a_refusal_with_no_fresh_token_reports_the_refusal() {
        let no_token = MEET_JSON.replace("\"SECRET-MEET-TOKEN\"", "null");
        let src = AthleticNet::new(StubFetcher::new(vec![
            ok_json(MEET_JSON),
            answer(403, DENIED),
            ok_json(&no_token),
        ]));
        let err = format!("{:#}", src.meet_events(900700).await.unwrap_err());
        assert!(
            err.contains(
                "refused the event list of meet 900700 (HTTP 403: Authorization has been \
                 denied for this request.), and a fresh GetMeetData gave no meet token"
            ),
            "{err}"
        );
        assert!(err.contains("paid Athletic.net subscription"), "{err}");
        assert!(!err.contains("may not publish"), "{err}");
        assert_eq!(src.fetcher.sent.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn a_refused_meet_says_so() {
        let src = AthleticNet::new(StubFetcher::new(vec![answer(403, DENIED)]));
        let err = format!("{:#}", src.meet(900700).await.unwrap_err());
        assert!(err.contains("refused meet 900700 (HTTP 403"), "{err}");
    }

    #[tokio::test]
    async fn a_states_own_tree_is_recorded_from_the_network_the_run_and_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let rec = dir.path().join("rec");
        let stub = StubFetcher::new(vec![token("tok-me", ME_2026, 2026), me_nav(ME_2026)]);
        let src = AthleticNet::new(stub)
            .with_cache(cache_in(dir.path(), CacheMode::Normal))
            .with_recorder(Recorder::new(&rec).unwrap());
        src.state_tree(2026, ME_2026).await.unwrap();
        let path = rec.join(format!("navinfo/2026-{ME_2026}.json"));
        let body = me_nav(ME_2026).body;
        assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
        // Again from this run's memo, then from the cache in a new run.
        std::fs::remove_file(&path).unwrap();
        src.state_tree(2026, ME_2026).await.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
        std::fs::remove_file(&path).unwrap();
        let again = AthleticNet::new(StubFetcher::new(vec![]))
            .with_cache(cache_in(dir.path(), CacheMode::Normal))
            .with_recorder(Recorder::new(&rec).unwrap());
        again.state_tree(2026, ME_2026).await.unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
        // The fixture source reads it back as the same subtree.
        let fixtures = crate::source::FixtureSource::from_dir(&rec).unwrap();
        let ids = |t: Vec<Region>| t.iter().map(|r| r.id).collect::<Vec<_>>();
        assert_eq!(
            ids(fixtures.state_tree(2026, ME_2026).await.unwrap()),
            ids(again.state_tree(2026, ME_2026).await.unwrap())
        );
    }

    #[tokio::test]
    async fn a_search_is_recorded_and_replays() {
        let json = r#"{"response":{"numFound":1,"docs":[
            {"id_db":"900000001","type":"Athlete","textsuggest":"Wren Halvorsen","subtext":"Lakeshore (HS)||Riverton, ZZ","score":12.5}
        ]}}"#;
        let dir = tempfile::tempdir().unwrap();
        let src = AthleticNet::new(StubFetcher::new(vec![ok_json(json)]))
            .with_recorder(Recorder::new(dir.path()).unwrap());
        let live = src.search("Wren Halvorsen").await.unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("search/wren-halvorsen.json")).unwrap(),
            json.replacen('{', "{\"athQuery\": \"Wren Halvorsen\", ", 1)
        );
        let fixtures = crate::source::FixtureSource::from_dir(dir.path()).unwrap();
        let replayed = fixtures.search("wren halvorsen").await.unwrap();
        assert_eq!(format!("{live:?}"), format!("{replayed:?}"));
    }

    #[tokio::test]
    async fn a_rankings_page_is_recorded_without_its_token() {
        let dir = tempfile::tempdir().unwrap();
        let page = r#"{"jwtTFTopReport": "secret.jwt.value", "eventShort": "400m", "division": {"ID": 900001, "SeasonID": 2026}, "minCount": 0, "groupedRankings": []}"#;
        let src = AthleticNet::new(StubFetcher::new(vec![ok_json(page)]))
            .with_recorder(Recorder::new(dir.path()).unwrap());
        let grades = Grades::all();
        let q = PageQuery {
            list_id: 900001,
            season: 2026,
            gender: Gender::F,
            event: "400m",
            grades: &grades,
            page: 1,
        };
        src.rankings_page(&q).await.unwrap();
        let got =
            std::fs::read_to_string(dir.path().join("rankings/900001-f-400m-all-p1.json")).unwrap();
        assert_eq!(got, page.replace("secret.jwt.value", ""));
    }

    /// The season's tree in a Recording is the national one whichever order
    /// the answers came in; a partial answer is also kept as its list's own.
    #[tokio::test]
    async fn a_partial_tree_never_overwrites_the_national_one_in_a_recording() {
        let seasons = || serde_json::json!({"2026": CA_2026});
        let record = |rec: &std::path::Path, answer: FetchResponse| {
            let src = AthleticNet::new(StubFetcher::new(vec![token("tok", CA_2026, 2026), answer]))
                .with_recorder(Recorder::new(rec).unwrap());
            async move { src.season_area_tree(2026, CA_2026).await.unwrap() }
        };
        let season_file =
            |rec: &std::path::Path| std::fs::read_to_string(rec.join("navinfo/2026.json")).unwrap();
        let national = nav(seasons(), CA_2026).body;
        let partial = partial_nav(seasons(), CA_2026).body;
        let own = format!("navinfo/2026-{CA_2026}.json");
        for national_first in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let rec = dir.path().join("rec");
            let answers = if national_first {
                [nav(seasons(), CA_2026), partial_nav(seasons(), CA_2026)]
            } else {
                [partial_nav(seasons(), CA_2026), nav(seasons(), CA_2026)]
            };
            let [first, second] = answers;
            record(&rec, first).await;
            if !national_first {
                // Alone, the partial answer stands for the season too.
                assert_eq!(season_file(&rec), partial);
            }
            record(&rec, second).await;
            assert_eq!(
                season_file(&rec),
                national,
                "national first: {national_first}"
            );
            assert_eq!(std::fs::read_to_string(rec.join(&own)).unwrap(), partial);
        }
    }

    #[tokio::test]
    async fn a_teamnav_is_recorded_without_its_token() {
        let json =
            r#"{"team": {"ID": 1, "Name": "A"}, "mascotToken2": "abc.def", "divisions": []}"#;
        let dir = tempfile::tempdir().unwrap();
        let src = AthleticNet::new(StubFetcher::new(vec![ok_json(json)]))
            .with_recorder(Recorder::new(dir.path()).unwrap());
        src.team_divisions(1, 2026).await.unwrap();
        let got = std::fs::read_to_string(dir.path().join("teamnav/1-2026.json")).unwrap();
        assert_eq!(got, json.replace("abc.def", ""));
    }

    #[tokio::test]
    async fn a_search_read_back_from_a_recording_keeps_one_query() {
        let json = r#"{"athQuery": "old", "response": {"docs": []}}"#;
        let dir = tempfile::tempdir().unwrap();
        let src = AthleticNet::new(StubFetcher::new(vec![ok_json(json)]))
            .with_recorder(Recorder::new(dir.path()).unwrap());
        src.search("Wren Halvorsen").await.unwrap();
        let got = std::fs::read_to_string(dir.path().join("search/wren-halvorsen.json")).unwrap();
        assert_eq!(
            got,
            r#"{"athQuery": "Wren Halvorsen", "response": {"docs": []}}"#
        );
    }
}
