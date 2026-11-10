//! The browser relay transport, driven by in-process fake tabs that
//! speak the relay protocol over real HTTP (`GET /next?tab=..&loop=..`,
//! `POST /result`), the way `src/relay.js` does in an Athletic.net tab.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ath::athletic::{AthleticNet, RetryConfig};
use ath::cache::{CacheMode, DataKind, ResponseCache};
use ath::http::{Fetcher, Request};
use ath::relay::{RelayConfig, RelayFetcher};
use ath::source::DataSource;
use serde_json::{Value, json};
use tokio::sync::Barrier;
use tokio::task::JoinHandle;

const ORIGIN: &str = "https://www.athletic.net";
const API: &str = "https://www.athletic.net/api/v1";

/// Short waits so the tests stay fast; nothing printed.
fn config(connect_ms: u64) -> RelayConfig {
    let mut config = RelayConfig::new(0, Duration::from_millis(connect_ms));
    config.poll_wait = Duration::from_millis(300);
    config.answer_timeout = Duration::from_secs(5);
    config.takeover_quiet = Duration::from_millis(200);
    config.announce = false;
    config
}

/// A relay on a free port.
async fn relay(connect_ms: u64) -> RelayFetcher {
    RelayFetcher::bind(config(connect_ms)).await.unwrap()
}

fn base(relay: &RelayFetcher) -> String {
    format!("http://{}", relay.local_addr())
}

/// A client that ignores any configured proxy, so localhost is reached directly.
fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// One raw `GET /next` as tab `tab`, loop `lp`.
async fn poll(base: &str, tab: &str, lp: u32) -> reqwest::Response {
    client()
        .get(format!("{base}/next?tab={tab}&loop={lp}"))
        .header("origin", ORIGIN)
        .send()
        .await
        .unwrap()
}

fn get(path: &str) -> Request {
    Request::get(format!("{API}{path}"))
}

/// What a fake tab answers a job with: `(status, body)`.
type Answer = Arc<dyn Fn(&Value) -> (u16, String) + Send + Sync>;

/// A fake tab that echoes each job back as the response body.
fn echo() -> Answer {
    Arc::new(|job: &Value| (200, job.to_string()))
}

/// One fake tab loop: poll `/next`, answer each job, post it to `/result`.
#[derive(Clone)]
struct Loop {
    base: String,
    tab: String,
    lp: u32,
    answer: Answer,
    seen: Arc<Mutex<Vec<Value>>>,
    hold: Option<Arc<Barrier>>,
    delay: Duration,
}

impl Loop {
    fn new(relay: &RelayFetcher, answer: Answer) -> Self {
        Self {
            base: base(relay),
            tab: "tab-a".into(),
            lp: 0,
            answer,
            seen: Default::default(),
            hold: None,
            delay: Duration::ZERO,
        }
    }

    fn tab(mut self, tab: &str) -> Self {
        self.tab = tab.into();
        self
    }

    fn lp(mut self, lp: u32) -> Self {
        self.lp = lp;
        self
    }

    fn seen(mut self, seen: &Arc<Mutex<Vec<Value>>>) -> Self {
        self.seen = seen.clone();
        self
    }

    /// Wait at `hold` before answering each job.
    fn hold(mut self, hold: &Arc<Barrier>) -> Self {
        self.hold = Some(hold.clone());
        self
    }

    /// Take this long to answer each job (a slow Athletic.net).
    fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// Run until the relay goes away (or the task is aborted).
    fn spawn(self) -> JoinHandle<()> {
        tokio::spawn(async move {
            let http = client();
            let next = format!("{}/next?tab={}&loop={}", self.base, self.tab, self.lp);
            loop {
                let Ok(resp) = http.get(&next).header("origin", ORIGIN).send().await else {
                    return;
                };
                if resp.status() == 204 {
                    continue;
                }
                assert_eq!(resp.status(), 200);
                let Ok(job) = resp.json::<Value>().await else {
                    return;
                };
                if job["standby"] == true {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
                self.seen.lock().unwrap().push(job.clone());
                if let Some(hold) = &self.hold {
                    hold.wait().await;
                }
                tokio::time::sleep(self.delay).await;
                let (status, body) = (self.answer)(&job);
                let result = json!({"id": job["id"], "status": status, "body": body});
                let posted = http
                    .post(format!("{}/result", self.base))
                    .header("origin", ORIGIN)
                    .json(&result)
                    .send()
                    .await;
                if posted.is_err() {
                    return;
                }
            }
        })
    }
}

#[tokio::test]
async fn get_is_handed_out_and_answered() {
    let relay = relay(5000).await;
    Loop::new(&relay, echo()).spawn();

    let url = format!("{API}/AthleteBio/GetAthleteBioData?athleteId=1&sport=tf&level=0");
    let resp = relay.fetch(&Request::get(&url)).await.unwrap();
    assert_eq!(resp.status, 200);
    let job: Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(job["method"], "GET");
    assert_eq!(job["url"], url);
    assert_eq!(job["body"], Value::Null);
    assert_eq!(job["headers"], json!({"accept": "application/json"}));
}

#[tokio::test]
async fn post_carries_its_json_body_and_content_type() {
    let relay = relay(5000).await;
    Loop::new(&relay, echo()).spawn();

    let body = json!({"eventShort": "400m", "qParams": {"page": 2}});
    let req = Request::post_json(format!("{API}/tfRankings/GetRankings"), &body).unwrap();
    let resp = relay.fetch(&req).await.unwrap();
    let job: Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(job["method"], "POST");
    assert_eq!(job["headers"]["content-type"], "application/json");
    let sent: Value = serde_json::from_str(job["body"].as_str().unwrap()).unwrap();
    assert_eq!(sent, body);
}

#[tokio::test]
async fn extra_headers_reach_the_tab() {
    // GetNavInfo needs the rankings token in an `anettokens` header.
    let relay = relay(5000).await;
    Loop::new(&relay, echo()).spawn();

    let url = format!("{API}/tfRankings/GetNavInfo?seasonId=2026");
    let req = Request::get(&url).with_header("anettokens", "tok.en");
    let resp = relay.fetch(&req).await.unwrap();
    let job: Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(
        job["headers"],
        json!({"accept": "application/json", "anettokens": "tok.en"})
    );
}

#[tokio::test]
async fn several_requests_are_in_flight_at_once() {
    const N: u32 = 3;
    let relay = relay(5000).await;
    // Each loop holds its job until all N are out: only possible if the relay
    // hands out a second request before the first is answered.
    let barrier = Arc::new(Barrier::new(N as usize));
    let answer: Answer = Arc::new(|job: &Value| (200, job["url"].as_str().unwrap().to_string()));
    for lp in 0..N {
        Loop::new(&relay, answer.clone())
            .lp(lp)
            .hold(&barrier)
            .spawn();
    }

    let urls: Vec<String> = (0..N).map(|i| format!("{API}/x?i={i}")).collect();
    let relay = &relay;
    let fetches = urls
        .iter()
        .map(|u| async move { relay.fetch(&Request::get(u)).await });
    let all = tokio::time::timeout(Duration::from_secs(5), futures::future::join_all(fetches))
        .await
        .expect("requests were not in flight together");
    for (resp, url) in all.into_iter().zip(&urls) {
        assert_eq!(&resp.unwrap().body, url, "each caller gets its own answer");
    }
}

#[tokio::test]
async fn an_idle_long_poll_answers_204() {
    let relay = relay(5000).await;
    let started = Instant::now();
    let resp = poll(&base(&relay), "tab-a", 0).await;
    assert_eq!(resp.status(), 204);
    assert!(
        started.elapsed() >= Duration::from_millis(250),
        "held as a long poll"
    );
    assert_eq!(resp.headers()["access-control-allow-origin"], ORIGIN);
}

#[tokio::test]
async fn a_poll_without_tab_and_loop_is_a_bad_request() {
    let relay = relay(5000).await;
    for query in ["", "?tab=a", "?loop=0", "?tab=&loop=0", "?tab=a&loop=x"] {
        let resp = client()
            .get(format!("{}/next{query}", base(&relay)))
            .header("origin", ORIGIN)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 400, "{query}");
    }
}

fn assert_setup_steps(err: &str) {
    for want in [
        "relay install",
        "https://www.athletic.net",
        "click Allow",
        "--console",
        "--cookie",
    ] {
        assert!(err.contains(want), "missing {want:?} in: {err}");
    }
}

#[tokio::test]
async fn no_tab_times_out_with_the_setup_instructions_then_fails_fast() {
    let relay = relay(300).await;
    let started = Instant::now();
    let err = relay.fetch(&get("/x")).await.unwrap_err().to_string();
    assert!(started.elapsed() < Duration::from_secs(3), "did not hang");
    assert!(
        err.contains("no Athletic.net browser tab connected"),
        "{err}"
    );
    assert_setup_steps(&err);

    // The relay now knows there is no tab: later requests fail at once.
    let started = Instant::now();
    let err = relay.fetch(&get("/y")).await.unwrap_err().to_string();
    assert!(
        started.elapsed() < Duration::from_millis(150),
        "failed fast"
    );
    assert_setup_steps(&err);
}

#[tokio::test]
async fn a_tab_that_polls_again_is_used_after_the_relay_gave_up() {
    let relay = relay(300).await;
    relay.fetch(&get("/x")).await.unwrap_err();
    Loop::new(&relay, echo()).spawn();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(relay.fetch(&get("/y")).await.unwrap().status, 200);
}

#[tokio::test]
async fn a_tab_gone_mid_run_fails_later_requests_fast() {
    let relay = relay(300).await;
    let tab = Loop::new(&relay, echo()).spawn();
    relay.fetch(&get("/1")).await.unwrap();
    tab.abort();

    let started = Instant::now();
    let err = relay.fetch(&get("/2")).await.unwrap_err().to_string();
    assert!(started.elapsed() < Duration::from_secs(3), "did not hang");
    assert!(err.contains("stopped answering"), "{err}");
    assert_setup_steps(&err);

    let started = Instant::now();
    relay.fetch(&get("/3")).await.unwrap_err();
    assert!(
        started.elapsed() < Duration::from_millis(150),
        "every later request does not wait the full timeout again"
    );
}

#[tokio::test]
async fn a_busy_tab_keeps_queued_requests_alive() {
    // One loop, each answer slower than the relay timeout: the second request
    // waits in the queue longer than the timeout while the tab is busy.
    let relay = relay(300).await;
    Loop::new(&relay, echo())
        .delay(Duration::from_millis(700))
        .spawn();
    let (ra, rb) = (get("/a"), get("/b"));
    let (a, b) = tokio::join!(relay.fetch(&ra), relay.fetch(&rb));
    assert_eq!(a.unwrap().status, 200);
    assert_eq!(b.unwrap().status, 200);
}

/// Start a fetch of `path` in the background and take its job with a raw poll
/// as `tab`/loop 0, never answering it (the browser side vanished).
async fn take_and_vanish(relay: &Arc<RelayFetcher>, path: &str, tab: &str) -> JoinHandle<String> {
    let fetching = relay.clone();
    let req = get(path);
    let fetch = tokio::spawn(async move {
        match fetching.fetch(&req).await {
            Ok(resp) => resp.body,
            Err(e) => format!("ERR {e}"),
        }
    });
    let resp = poll(&base(relay), tab, 0).await;
    assert_eq!(resp.status(), 200);
    let job: Value = resp.json().await.unwrap();
    assert!(job["url"].as_str().unwrap().ends_with(path), "{job}");
    fetch
}

#[tokio::test]
async fn a_job_its_loop_lost_is_handed_out_again() {
    let relay = Arc::new(relay(5000).await);
    let fetch = take_and_vanish(&relay, "/lost", "tab-a").await;
    // The same loop polling again means it never got (or dropped) the job.
    let seen = Arc::new(Mutex::new(Vec::new()));
    Loop::new(&relay, echo()).seen(&seen).spawn();
    let body = tokio::time::timeout(Duration::from_secs(2), fetch)
        .await
        .expect("recovered without waiting for the answer timeout")
        .unwrap();
    assert!(body.contains("/lost"), "{body}");
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_reloaded_tab_takes_over_and_gets_the_lost_job() {
    let relay = Arc::new(relay(5000).await);
    let fetch = take_and_vanish(&relay, "/lost", "old-tab").await;
    Loop::new(&relay, echo()).tab("new-tab").spawn();
    let body = tokio::time::timeout(Duration::from_secs(2), fetch)
        .await
        .expect("the new tab took over")
        .unwrap();
    assert!(body.contains("/lost"), "{body}");
}

#[tokio::test]
async fn a_job_lost_twice_fails_instead_of_looping() {
    let relay = Arc::new(relay(5000).await);
    let fetch = take_and_vanish(&relay, "/lost", "tab-a").await;
    // Handed out a second time, and lost again.
    let again: Value = poll(&base(&relay), "tab-a", 0).await.json().await.unwrap();
    assert!(again["url"].as_str().unwrap().ends_with("/lost"));
    poll(&base(&relay), "tab-a", 0).await;
    let body = tokio::time::timeout(Duration::from_secs(2), fetch)
        .await
        .unwrap()
        .unwrap();
    assert!(body.starts_with("ERR") && body.contains("lost"), "{body}");
}

#[tokio::test]
async fn a_second_tab_waits_on_standby() {
    let relay = relay(5000).await;
    let seen_a = Arc::new(Mutex::new(Vec::new()));
    Loop::new(&relay, echo()).seen(&seen_a).spawn();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let started = Instant::now();
    let resp = poll(&base(&relay), "tab-b", 0).await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<Value>().await.unwrap(),
        json!({"standby": true})
    );
    assert!(started.elapsed() < Duration::from_millis(150), "not held");

    relay.fetch(&get("/x")).await.unwrap();
    assert_eq!(seen_a.lock().unwrap().len(), 1, "the active tab served it");
}

#[tokio::test]
async fn a_standby_tab_takes_over_when_the_active_one_goes_quiet() {
    let relay = relay(5000).await;
    let a = Loop::new(&relay, echo()).spawn();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let seen_b = Arc::new(Mutex::new(Vec::new()));
    Loop::new(&relay, echo()).tab("tab-b").seen(&seen_b).spawn();
    a.abort();
    let resp = tokio::time::timeout(Duration::from_secs(3), relay.fetch(&get("/x")))
        .await
        .expect("tab b took over")
        .unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(seen_b.lock().unwrap().len(), 1);
}

const CHALLENGE: &str = "<!DOCTYPE html><html><head><title>Just a moment...</title></head></html>";

const BIO_JSON: &str = r#"{
    "athlete": {"IDAthlete": 1001, "SchoolID": 501, "FirstName": "Alice", "LastName": "Anders", "Gender": "F"},
    "allTeams": {},
    "eventsTF": [{"Event": "400 Meters", "IDEvent": 21, "IDEventType": 0}],
    "resultsTF": [{"AthleteID": 1001, "Result": "58.24a", "SortInt": 58240, "EventID": 21, "EventTypeID": 0, "SeasonID": 2026, "Exhibition": false}]
}"#;

/// A tab whose first `challenges` answers are Cloudflare challenges, then the bio.
fn challenging_tab(challenges: usize) -> Answer {
    let count = Arc::new(Mutex::new(0usize));
    Arc::new(move |_job: &Value| {
        let mut n = count.lock().unwrap();
        *n += 1;
        if *n <= challenges {
            (403, CHALLENGE.to_string())
        } else {
            (200, BIO_JSON.to_string())
        }
    })
}

fn retry(base_delay_ms: u64) -> RetryConfig {
    RetryConfig {
        max_retries: 3,
        base_delay: Duration::from_millis(base_delay_ms),
    }
}

#[tokio::test]
async fn a_relayed_challenge_backs_off_and_retries() {
    let relay = relay(5000).await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    Loop::new(&relay, challenging_tab(2)).seen(&seen).spawn();

    let src = AthleticNet::new(relay).with_retry(retry(40));
    let started = Instant::now();
    let bio = src.get_bio(1001).await.unwrap();
    assert_eq!(bio.athlete.id, 1001);
    assert_eq!(
        seen.lock().unwrap().len(),
        3,
        "two challenges, then the bio"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(40 + 80),
        "backed off between attempts"
    );
}

#[tokio::test]
async fn a_persistent_relayed_challenge_stops_and_asks_about_the_tab() {
    let relay = relay(5000).await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    Loop::new(&relay, challenging_tab(usize::MAX))
        .seen(&seen)
        .spawn();

    let src = AthleticNet::new(relay).with_retry(retry(10));
    let err = src.get_bio(1001).await.unwrap_err().to_string();
    assert!(err.contains("Cloudflare challenge (HTTP 403)"), "{err}");
    assert!(err.contains("browser tab"), "advice fits the relay: {err}");
    assert!(
        !err.contains("--cookie"),
        "no cookie advice for the relay: {err}"
    );
    assert_eq!(
        seen.lock().unwrap().len(),
        4,
        "1 try + 3 retries, no hammering"
    );
}

#[tokio::test]
async fn other_origins_are_refused() {
    let relay = relay(5000).await;
    let http = client();
    let next = format!("{}/next?tab=evil&loop=0", base(&relay));
    let result = format!("{}/result", base(&relay));

    for origin in [
        Some("https://evil.example"),
        Some("http://www.athletic.net"),
        Some("https://athletic.net"),
        Some("null"),
        None,
    ] {
        let mut get = http.get(&next);
        let mut post = http
            .post(&result)
            .json(&json!({"id": 1, "status": 200, "body": "x"}));
        let mut options = http.request(reqwest::Method::OPTIONS, &result);
        if let Some(o) = origin {
            get = get.header("origin", o);
            post = post.header("origin", o);
            options = options.header("origin", o);
        }
        for resp in [
            get.send().await.unwrap(),
            post.send().await.unwrap(),
            options.send().await.unwrap(),
        ] {
            assert_eq!(resp.status(), 403, "origin {origin:?}");
            assert!(
                resp.headers().get("access-control-allow-origin").is_none(),
                "origin {origin:?} got CORS headers"
            );
        }
    }

    // A refused poll never takes a job (or the active-tab slot): a real tab
    // still gets it.
    let req = get("/x");
    let fetch = relay.fetch(&req);
    let refused = async {
        let resp = http
            .get(&next)
            .header("origin", "https://evil.example")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 403);
        Loop::new(&relay, echo()).spawn();
    };
    let (resp, ()) = tokio::join!(fetch, refused);
    assert_eq!(resp.unwrap().status, 200);
}

#[tokio::test]
async fn preflight_allows_athletic_net_and_private_network_access() {
    let relay = relay(5000).await;
    let resp = client()
        .request(reqwest::Method::OPTIONS, format!("{}/result", base(&relay)))
        .header("origin", ORIGIN)
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", "content-type")
        .header("access-control-request-private-network", "true")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);
    let h = resp.headers();
    assert_eq!(h["access-control-allow-origin"], ORIGIN);
    assert_eq!(h["access-control-allow-private-network"], "true");
    assert_eq!(h["access-control-allow-methods"], "GET, POST");
    assert_eq!(h["access-control-allow-headers"], "content-type");
}

#[tokio::test]
async fn only_athletic_net_urls_are_relayed() {
    let relay = relay(5000).await;
    let err = relay
        .fetch(&Request::get("https://www.athletic.net.evil.example/api"))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("only fetches https://www.athletic.net"),
        "{err}"
    );
}

#[tokio::test]
async fn a_tab_side_error_fails_the_fetch() {
    let relay = relay(5000).await;
    let base = base(&relay);
    let tab = tokio::spawn(async move {
        let http = client();
        let job: Value = loop {
            let resp = poll(&base, "tab-a", 0).await;
            if resp.status() == 200 {
                break resp.json().await.unwrap();
            }
        };
        let posted = http
            .post(format!("{base}/result"))
            .header("origin", ORIGIN)
            .json(&json!({"id": job["id"], "error": "TypeError: Failed to fetch"}))
            .send()
            .await
            .unwrap();
        assert_eq!(posted.status(), 204);
        // The job is gone once answered: a second answer is refused.
        let again = http
            .post(format!("{base}/result"))
            .header("origin", ORIGIN)
            .json(&json!({"id": job["id"], "status": 200, "body": "{}"}))
            .send()
            .await
            .unwrap();
        assert_eq!(again.status(), 404);
    });
    let err = relay.fetch(&get("/x")).await.unwrap_err().to_string();
    assert!(err.contains("Failed to fetch"), "{err}");
    tab.await.unwrap();
}

#[tokio::test]
async fn the_relay_works_as_a_data_source_end_to_end() {
    let relay = relay(5000).await;
    Loop::new(&relay, challenging_tab(0)).spawn();
    let src: Box<dyn DataSource> = Box::new(AthleticNet::new(relay));
    assert_eq!(
        src.get_bio(1001).await.unwrap().results_tf[0].sort_int,
        58240
    );
}

#[tokio::test]
async fn a_run_the_cache_serves_needs_no_tab() {
    let dir = tempfile::tempdir().unwrap();
    let cache = ResponseCache::new(dir.path(), CacheMode::Normal).unwrap();
    let req = get("/AthleteBio/GetAthleteBioData?athleteId=1001&sport=tf&level=0");
    cache.put(&req, DataKind::Profile, BIO_JSON).unwrap();

    // No tab ever connects; a 5s timeout would show if the relay were waited on.
    let src = AthleticNet::new(relay(5000).await).with_cache(cache);
    let started = Instant::now();
    assert_eq!(src.get_bio(1001).await.unwrap().athlete.id, 1001);
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[tokio::test]
async fn dropping_the_relay_closes_its_parked_polls() {
    let mut config = config(5000);
    config.poll_wait = Duration::from_secs(10);
    let relay = RelayFetcher::bind(config).await.unwrap();
    let base = base(&relay);
    let parked = tokio::spawn(async move {
        client()
            .get(format!("{base}/next?tab=tab-a&loop=0"))
            .header("origin", ORIGIN)
            .send()
            .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(relay);
    let result = tokio::time::timeout(Duration::from_secs(2), parked)
        .await
        .expect("the parked poll was not left hanging")
        .unwrap();
    assert!(
        result.is_err(),
        "connection closed, not answered: {result:?}"
    );
}
