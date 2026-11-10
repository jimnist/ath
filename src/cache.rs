//! On-disk cache of raw Athletic.net responses, keyed by request (ADR-0006).
//!
//! An entry is keyed by the request that produced it (method, URL and body), so
//! two POSTs to the same URL with different bodies are separate entries. How
//! long an entry stays fresh is decided by the [`DataKind`] the caller passes,
//! not by sniffing the URL.
//!
//! The raw response text is cached (not a parsed model) so a cached read
//! reparses the full original payload - later code that reads more fields
//! still works without busting the cache.
//!
//! Each entry is one `<sha256 of the request>.json` file holding the request,
//! its kind, when it was fetched and the raw response text, so `jq` on an entry
//! shows what it is (`jq -r .response` gives the response itself). Files that
//! are not entries in this layout (such as the old `<athlete_id>.json` bio
//! cache) are never matched and never break a read.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::http::Request;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheMode {
    /// Read fresh entries; write fetched ones.
    Normal,
    /// Ignore existing entries (always refetch), but write the fresh result.
    Refresh,
    /// Do not read or write the cache at all.
    Off,
}

const HOUR: Duration = Duration::from_secs(3600);

/// What a cached response is, which decides how long it stays fresh (the
/// table in ADR-0006).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataKind {
    /// The Area (division) tree for a season. It never changes.
    AreaTree,
    /// A rankings list. A finished season's list is final; one in progress
    /// changes as meets are posted.
    Rankings { season_finished: bool },
    /// A person's profile (the bio JSON).
    ///
    /// TODO: ADR-0006 also makes a profile fresh forever once the person's last
    /// season is over. That needs the bio parsed at cache time to know the
    /// last season; until then every profile is fresh for 12 hours.
    Profile,
    /// A meet's entries, which change up to the meet.
    MeetEntries,
    /// A school's division chain for one season (`TeamNav/Team`): which
    /// State, Area and league lists it is on. Final once the season is over;
    /// while it is in progress Athletic.net may still move the school.
    TeamDivisions { season_finished: bool },
    /// "Athletic.net has no such season yet": a marker stored where the
    /// season's [`DataKind::AreaTree`] would be, so the next few runs do not
    /// ask again, while a season that appears is found within hours.
    MissingSeason,
}

/// A parsed answer that may say it is not worth caching (a rankings page an
/// unpaid session saw capped, `list.rs`). Such an answer is never written to
/// the cache, and one already there is a miss.
pub trait Cacheable {
    fn cacheable(&self) -> bool {
        true
    }
}

/// The last day of an outdoor season (July 31) after which it counts as
/// finished. rankmap uses the same default (`season_end`, `docs/rankmap.md`).
const SEASON_END: (u32, u32) = (7, 31);

/// Whether `season` is over as of `today`: today is after July 31 of the
/// season's year.
pub fn season_finished(season: i64, today: chrono::NaiveDate) -> bool {
    let Ok(year) = i32::try_from(season) else {
        return season < 0;
    };
    match chrono::NaiveDate::from_ymd_opt(year, SEASON_END.0, SEASON_END.1) {
        Some(end) => today > end,
        None => false,
    }
}

/// How long an entry of some kind stays fresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// Fresh until `--refresh` asks for it again.
    Forever,
    /// Stale once older than this.
    For(Duration),
}

impl Freshness {
    /// The shorter-lived of two freshness rules.
    fn stricter(self, other: Freshness) -> Freshness {
        match (self, other) {
            (Freshness::Forever, f) | (f, Freshness::Forever) => f,
            (Freshness::For(a), Freshness::For(b)) => Freshness::For(a.min(b)),
        }
    }
}

impl DataKind {
    pub fn freshness(self) -> Freshness {
        match self {
            DataKind::AreaTree => Freshness::Forever,
            DataKind::Rankings {
                season_finished: true,
            } => Freshness::Forever,
            DataKind::Rankings {
                season_finished: false,
            } => Freshness::For(12 * HOUR),
            DataKind::Profile => Freshness::For(12 * HOUR),
            DataKind::MeetEntries => Freshness::For(HOUR),
            DataKind::TeamDivisions {
                season_finished: true,
            } => Freshness::Forever,
            DataKind::TeamDivisions {
                season_finished: false,
            } => Freshness::For(12 * HOUR),
            DataKind::MissingSeason => Freshness::For(6 * HOUR),
        }
    }
}

/// The clock the cache stamps and ages entries by; injectable for tests.
pub type Clock = Arc<dyn Fn() -> SystemTime + Send + Sync>;

/// One cache file: the request it answers, plus the raw response.
#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    method: String,
    url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    body: Option<String>,
    kind: DataKind,
    /// Seconds since the Unix epoch.
    fetched_at: u64,
    response: String,
}

/// A directory of request-keyed response entries.
#[derive(Clone)]
pub struct ResponseCache {
    dir: PathBuf,
    mode: CacheMode,
    clock: Clock,
    /// Keys written by this run, recorded in `Refresh` mode only. That mode
    /// serves these, so a request repeated within one run is fetched once.
    written: Arc<Mutex<HashSet<String>>>,
}

impl fmt::Debug for ResponseCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResponseCache")
            .field("dir", &self.dir)
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

impl ResponseCache {
    pub fn new(dir: impl AsRef<Path>, mode: CacheMode) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        if mode != CacheMode::Off {
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("creating cache dir {}", dir.display()))?;
        }
        Ok(Self {
            dir,
            mode,
            clock: Arc::new(SystemTime::now),
            written: Arc::default(),
        })
    }

    /// Replace the wall clock (tests age entries without sleeping).
    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    #[cfg(test)]
    fn path(&self, req: &Request) -> PathBuf {
        self.entry_path(&key(req))
    }

    fn entry_path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }

    fn now_secs(&self) -> u64 {
        (self.clock)()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// The cached raw response for a request, if present and still fresh.
    ///
    /// An entry is fresh under the stricter of the kind it was stored as and
    /// the `kind` asked for, so a list cached mid-season still expires after
    /// the season ends. Returns `None` in `Off` mode, in `Refresh` mode for
    /// anything this run has not written itself, and when the entry is
    /// missing, stale or unreadable (the cache is optional; a bad entry is
    /// just refetched and overwritten).
    pub fn get(&self, req: &Request, kind: DataKind) -> Option<String> {
        let key = key(req);
        match self.mode {
            CacheMode::Off => return None,
            CacheMode::Refresh if !self.written_this_run(&key) => return None,
            _ => {}
        }
        let Ok(text) = std::fs::read_to_string(self.entry_path(&key)) else {
            return None;
        };
        // A file that is not an entry, or answers a different request, is a
        // miss: the next put overwrites it.
        let Ok(entry) = serde_json::from_str::<Entry>(&text) else {
            return None;
        };
        if entry.method != req.method().as_str()
            || entry.url != req.url()
            || entry.body.as_deref() != req.body()
        {
            return None;
        }
        if let Freshness::For(ttl) = entry.kind.freshness().stricter(kind.freshness()) {
            let age = Duration::from_secs(self.now_secs().saturating_sub(entry.fetched_at));
            if age > ttl {
                return None;
            }
        }
        Some(entry.response)
    }

    fn written_this_run(&self, key: &str) -> bool {
        self.written
            .lock()
            .map(|w| w.contains(key))
            .unwrap_or(false)
    }

    /// Delete a request's entry if it still holds `response` (what
    /// [`Self::get`] gave), for one its reader will never use. An entry
    /// another task or run has rewritten since is left alone: the entry is
    /// first moved aside (atomically, so a write landing at the same moment
    /// is never the one deleted), then deleted only when it is the one read,
    /// and otherwise moved back unless a newer one has arrived meanwhile.
    /// Whether it was deleted; a no-op in `Off` mode and when there is none.
    pub fn remove_if(&self, req: &Request, response: &str) -> Result<bool> {
        if self.mode == CacheMode::Off {
            return Ok(false);
        }
        let key = key(req);
        let path = self.entry_path(&key);
        static CLAIMS: AtomicU64 = AtomicU64::new(0);
        let n = CLAIMS.fetch_add(1, Ordering::Relaxed);
        let claimed = self
            .dir
            .join(format!(".{key}.{}.{n}.del", std::process::id()));
        match std::fs::rename(&path, &claimed) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => {
                return Err(e).with_context(|| format!("deleting cache entry {}", path.display()));
            }
            Ok(()) => {}
        }
        let same = std::fs::read_to_string(&claimed)
            .ok()
            .and_then(|text| serde_json::from_str::<Entry>(&text).ok())
            .is_some_and(|entry| entry.response == response);
        let done = if same || path.exists() {
            std::fs::remove_file(&claimed)
        } else {
            std::fs::rename(&claimed, &path)
        };
        done.with_context(|| format!("deleting cache entry {}", path.display()))?;
        Ok(same)
    }

    /// Write the raw response for a request (no-op in `Off` mode).
    ///
    /// Writes to a unique temp file then renames, so a crash or a concurrent
    /// write of the same request never leaves a truncated entry.
    pub fn put(&self, req: &Request, kind: DataKind, raw: &str) -> Result<()> {
        if self.mode == CacheMode::Off {
            return Ok(());
        }
        let entry = Entry {
            method: req.method().as_str().to_string(),
            url: req.url().to_string(),
            body: req.body().map(str::to_string),
            kind,
            fetched_at: self.now_secs(),
            response: raw.to_string(),
        };
        let json = serde_json::to_string_pretty(&entry).context("serializing cache entry")?;
        let key = key(req);
        let path = self.entry_path(&key);
        // Unique per process and per write, so concurrent writers of the same
        // request (tasks here, or parallel `ath` runs) never share a temp file.
        static WRITES: AtomicU64 = AtomicU64::new(0);
        let n = WRITES.fetch_add(1, Ordering::Relaxed);
        let tmp = self
            .dir
            .join(format!(".{key}.{}.{n}.tmp", std::process::id()));
        std::fs::write(&tmp, json)
            .with_context(|| format!("writing cache temp file {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("committing cache entry {}", path.display()))?;
        if self.mode == CacheMode::Refresh
            && let Ok(mut written) = self.written.lock()
        {
            written.insert(key);
        }
        Ok(())
    }
}

/// The filesystem-safe key of a request: hex SHA-256 over method, URL and body,
/// each length-prefixed so no two requests share a preimage.
fn key(req: &Request) -> String {
    let mut h = Sha256::new();
    for part in [req.method().as_str(), req.url(), req.body().unwrap_or("")] {
        h.update((part.len() as u64).to_le_bytes());
        h.update(part.as_bytes());
    }
    h.update([u8::from(req.body().is_some())]);
    format!("{:x}", h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: u64 = 1_780_000_000;

    fn at(secs: u64) -> Clock {
        Arc::new(move || UNIX_EPOCH + Duration::from_secs(secs))
    }

    fn cache(dir: &Path, mode: CacheMode, now: u64) -> ResponseCache {
        ResponseCache::new(dir, mode).unwrap().with_clock(at(now))
    }

    fn bio_req() -> Request {
        Request::get("https://www.athletic.net/api/v1/AthleteBio/GetAthleteBioData?athleteId=42")
    }

    fn rankings_req(page: u32) -> Request {
        Request::post_json(
            "https://www.athletic.net/api/v1/tfRankings/GetRankings",
            &serde_json::json!({"eventShort": "400m", "qParams": {"page": page}}),
        )
        .unwrap()
    }

    /// Write at T0, then read back `elapsed` later.
    fn hit_after(kind: DataKind, elapsed: Duration) -> bool {
        let dir = tempfile::tempdir().unwrap();
        let req = rankings_req(1);
        cache(dir.path(), CacheMode::Normal, T0)
            .put(&req, kind, "{}")
            .unwrap();
        cache(dir.path(), CacheMode::Normal, T0 + elapsed.as_secs())
            .get(&req, kind)
            .is_some()
    }

    const YEARS: Duration = Duration::from_secs(5 * 365 * 24 * 3600);

    #[test]
    fn finished_season_rankings_never_stale() {
        let kind = DataKind::Rankings {
            season_finished: true,
        };
        assert!(hit_after(kind, YEARS));
    }

    #[test]
    fn area_tree_never_stale() {
        assert!(hit_after(DataKind::AreaTree, YEARS));
    }

    #[test]
    fn in_season_entries_stale_after_12_hours() {
        for kind in [
            DataKind::Rankings {
                season_finished: false,
            },
            DataKind::Profile,
        ] {
            assert!(hit_after(kind, 12 * HOUR), "{kind:?} fresh at 12h");
            assert!(
                !hit_after(kind, 12 * HOUR + Duration::from_secs(1)),
                "{kind:?} stale past 12h"
            );
        }
    }

    #[test]
    fn team_divisions_last_forever_once_the_season_is_over() {
        let finished = DataKind::TeamDivisions {
            season_finished: true,
        };
        assert!(hit_after(finished, YEARS));
        let in_progress = DataKind::TeamDivisions {
            season_finished: false,
        };
        assert!(hit_after(in_progress, 12 * HOUR));
        assert!(!hit_after(in_progress, 12 * HOUR + Duration::from_secs(1)));
    }

    #[test]
    fn a_missing_season_marker_lasts_six_hours_even_read_as_a_tree() {
        assert!(hit_after(DataKind::MissingSeason, 6 * HOUR));
        assert!(!hit_after(
            DataKind::MissingSeason,
            6 * HOUR + Duration::from_secs(1)
        ));
        // Read back as the forever-fresh tree it stands in for, it still expires.
        let dir = tempfile::tempdir().unwrap();
        let req = rankings_req(1);
        cache(dir.path(), CacheMode::Normal, T0)
            .put(&req, DataKind::MissingSeason, "marker")
            .unwrap();
        let later = cache(dir.path(), CacheMode::Normal, T0 + 6 * 3600 + 1);
        assert!(later.get(&req, DataKind::AreaTree).is_none());
    }

    #[test]
    fn a_season_is_finished_after_july_31() {
        let d = |s: &str| s.parse::<chrono::NaiveDate>().unwrap();
        assert!(!season_finished(2026, d("2026-07-31")));
        assert!(season_finished(2026, d("2026-08-01")));
        assert!(season_finished(2025, d("2026-03-01")));
        assert!(!season_finished(2027, d("2026-10-26")));
    }

    #[test]
    fn extra_headers_are_not_part_of_the_key() {
        // GetNavInfo's token changes every call; the answer does not.
        let url = "https://example.test/nav";
        assert_eq!(
            key(&Request::get(url).with_header("anettokens", "a")),
            key(&Request::get(url).with_header("anettokens", "b"))
        );
        let dir = tempfile::tempdir().unwrap();
        let c = cache(dir.path(), CacheMode::Normal, T0);
        c.put(
            &Request::get(url).with_header("anettokens", "a"),
            DataKind::AreaTree,
            "tree",
        )
        .unwrap();
        assert_eq!(
            c.get(&Request::get(url), DataKind::AreaTree).as_deref(),
            Some("tree")
        );
        let file = std::fs::read_dir(dir.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        let text = std::fs::read_to_string(file.path()).unwrap();
        assert!(!text.contains("anettokens"), "no token on disk: {text}");
    }

    #[test]
    fn remove_if_deletes_only_the_entry_that_was_read() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let c = cache(dir.path(), CacheMode::Normal, T0);
        let req = rankings_req(1);
        let kind = DataKind::Rankings {
            season_finished: true,
        };
        c.put(&req, kind, r#"{"capped": true}"#)?;
        let read = c.get(&req, kind).unwrap();
        // Another task writes a good answer after the read: it stays.
        c.put(&req, kind, r#"{"capped": false}"#)?;
        assert!(!c.remove_if(&req, &read)?);
        assert_eq!(c.get(&req, kind).as_deref(), Some(r#"{"capped": false}"#));
        // The entry read is deleted, and nothing is left behind.
        let read = c.get(&req, kind).unwrap();
        assert!(c.remove_if(&req, &read)?);
        assert_eq!(c.get(&req, kind), None);
        assert_eq!(std::fs::read_dir(dir.path())?.count(), 0);
        // Nothing there is nothing to delete.
        assert!(!c.remove_if(&req, &read)?);
        Ok(())
    }

    #[test]
    fn meet_entries_stale_after_an_hour() {
        assert!(hit_after(DataKind::MeetEntries, HOUR));
        assert!(!hit_after(
            DataKind::MeetEntries,
            HOUR + Duration::from_secs(1)
        ));
    }

    #[test]
    fn normal_mode_misses_then_hits() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let c = cache(dir.path(), CacheMode::Normal, T0);
        assert!(c.get(&bio_req(), DataKind::Profile).is_none(), "empty miss");
        c.put(&bio_req(), DataKind::Profile, "{\"hello\":true}")?;
        assert_eq!(
            c.get(&bio_req(), DataKind::Profile).as_deref(),
            Some("{\"hello\":true}")
        );
        Ok(())
    }

    #[test]
    fn refresh_mode_ignores_reads_but_rewrites() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let kind = DataKind::AreaTree;
        cache(dir.path(), CacheMode::Normal, T0).put(&bio_req(), kind, "cached")?;
        let refresh = cache(dir.path(), CacheMode::Refresh, T0);
        assert!(
            refresh.get(&bio_req(), kind).is_none(),
            "refresh ignores even a forever-fresh entry"
        );
        refresh.put(&bio_req(), kind, "fresh")?;
        let normal = cache(dir.path(), CacheMode::Normal, T0);
        assert_eq!(
            normal.get(&bio_req(), kind).as_deref(),
            Some("fresh"),
            "refresh rewrites the entry"
        );
        Ok(())
    }

    #[test]
    fn off_mode_neither_reads_nor_writes() -> Result<()> {
        let dir = tempfile::tempdir()?;
        cache(dir.path(), CacheMode::Normal, T0).put(&bio_req(), DataKind::Profile, "old")?;
        let off = cache(dir.path(), CacheMode::Off, T0);
        assert!(
            off.get(&bio_req(), DataKind::Profile).is_none(),
            "off mode reads nothing"
        );
        off.put(&rankings_req(1), DataKind::AreaTree, "x")?;
        assert_eq!(
            std::fs::read_dir(dir.path())?.count(),
            1,
            "off mode wrote nothing"
        );
        Ok(())
    }

    #[test]
    fn off_mode_does_not_create_the_dir() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope");
        ResponseCache::new(&missing, CacheMode::Off).unwrap();
        assert!(!missing.exists());
    }

    #[test]
    fn post_bodies_that_differ_are_separate_entries() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let c = cache(dir.path(), CacheMode::Normal, T0);
        let kind = DataKind::Rankings {
            season_finished: true,
        };
        c.put(&rankings_req(1), kind, "page one")?;
        c.put(&rankings_req(2), kind, "page two")?;
        assert_eq!(c.get(&rankings_req(1), kind).as_deref(), Some("page one"));
        assert_eq!(c.get(&rankings_req(2), kind).as_deref(), Some("page two"));
        Ok(())
    }

    #[test]
    fn method_is_part_of_the_key() {
        let url = "https://example.test/x";
        let get = Request::get(url);
        let post = Request::post_json(url, &serde_json::json!({})).unwrap();
        assert_ne!(key(&get), key(&post));
        assert_eq!(key(&get), key(&Request::get(url)), "key is stable");
    }

    #[test]
    fn entries_are_human_inspectable() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let req = rankings_req(3);
        cache(dir.path(), CacheMode::Normal, T0).put(&req, DataKind::AreaTree, "{\"a\":1}")?;
        let file = std::fs::read_dir(dir.path())?.next().unwrap()?.path();
        let name = file.file_name().unwrap().to_str().unwrap().to_string();
        assert_eq!(name.len(), 64 + ".json".len(), "hex sha256 name: {name}");
        let entry: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file)?)?;
        assert_eq!(entry["method"], "POST");
        assert_eq!(entry["url"], req.url());
        assert_eq!(entry["body"], req.body().unwrap());
        assert_eq!(entry["kind"], "area_tree");
        assert_eq!(entry["fetched_at"], T0);
        assert_eq!(entry["response"], "{\"a\":1}");
        Ok(())
    }

    #[test]
    fn old_bio_layout_dir_is_harmless() -> Result<()> {
        let dir = tempfile::tempdir()?;
        // The pre-ADR-0006 layout: raw bio JSON in `<athlete_id>.json`.
        let old = dir.path().join("42.json");
        std::fs::write(&old, "{\"athlete\":{\"IDAthlete\":42}}")?;
        let c = cache(dir.path(), CacheMode::Normal, T0);
        assert!(c.get(&bio_req(), DataKind::Profile).is_none());
        c.put(&bio_req(), DataKind::Profile, "new")?;
        assert_eq!(c.get(&bio_req(), DataKind::Profile).as_deref(), Some("new"));
        assert!(old.exists(), "old entries are left alone");
        Ok(())
    }

    #[test]
    fn in_season_entry_read_as_finished_still_expires() -> Result<()> {
        // Cached mid-season, then the season ends: the partial list must not
        // become fresh forever just because the reader now says "finished".
        let dir = tempfile::tempdir()?;
        let req = rankings_req(1);
        cache(dir.path(), CacheMode::Normal, T0).put(
            &req,
            DataKind::Rankings {
                season_finished: false,
            },
            "partial",
        )?;
        let later = cache(dir.path(), CacheMode::Normal, T0 + 13 * 3600);
        let finished = DataKind::Rankings {
            season_finished: true,
        };
        assert!(later.get(&req, finished).is_none());
        later.put(&req, finished, "final")?;
        let much_later = cache(dir.path(), CacheMode::Normal, T0 + YEARS.as_secs());
        assert_eq!(much_later.get(&req, finished).as_deref(), Some("final"));
        Ok(())
    }

    #[test]
    fn refresh_mode_serves_what_this_run_wrote() -> Result<()> {
        // A request repeated within one `--refresh` run is fetched once.
        let dir = tempfile::tempdir()?;
        let kind = DataKind::Profile;
        let refresh = cache(dir.path(), CacheMode::Refresh, T0);
        refresh.put(&bio_req(), kind, "fresh")?;
        assert_eq!(refresh.get(&bio_req(), kind).as_deref(), Some("fresh"));
        Ok(())
    }

    #[test]
    fn unreadable_path_is_a_miss_not_an_error() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let c = cache(dir.path(), CacheMode::Normal, T0);
        std::fs::create_dir(c.path(&bio_req()))?;
        assert!(c.get(&bio_req(), DataKind::Profile).is_none());
        Ok(())
    }

    #[test]
    fn unreadable_entry_is_a_miss_then_overwritten() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let c = cache(dir.path(), CacheMode::Normal, T0);
        std::fs::write(c.path(&bio_req()), "not an entry")?;
        assert!(c.get(&bio_req(), DataKind::Profile).is_none());
        c.put(&bio_req(), DataKind::Profile, "ok")?;
        assert_eq!(c.get(&bio_req(), DataKind::Profile).as_deref(), Some("ok"));
        Ok(())
    }
}
