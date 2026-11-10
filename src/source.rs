//! The single network seam: the Athletic.net data source.
//!
//! Everything above this trait is tested hermetically by driving it with
//! [`FixtureSource`]. The live implementation (`athletic::AthleticNet`: the
//! transports, Cloudflare handling, response cache) sits behind the same trait.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::de::DeserializeOwned;

use crate::area::{AreaTree, NavInfo, Region, TeamNav};
use crate::list::{PageQuery, RankingsPage};
use crate::meet::{EntriesData, EntriesQuery, MeetData, MeetEventList};
use crate::model::{Bio, Candidate};

/// A source of Athletic.net data.
#[async_trait]
pub trait DataSource: Send + Sync {
    /// Fetch a full bio by athlete id.
    async fn get_bio(&self, athlete_id: i64) -> Result<Bio>;

    /// Search for candidate athletes by name (used by Mode B resolution).
    async fn search(&self, name: &str) -> Result<Vec<Candidate>>;

    /// The high school division tree for `season` (every State and the levels
    /// below it, with their list ids), found from `from_list`, a rankings list
    /// in any season. `None` when Athletic.net has no such season yet.
    async fn area_tree(&self, season: i64, from_list: i64) -> Result<Option<AreaTree>>;

    /// The high school division tree for `season`, read through `list_id`, a
    /// rankings list known to be in that season (such as the State list in a
    /// school's chain for the season).
    async fn season_area_tree(&self, season: i64, list_id: i64) -> Result<AreaTree>;

    /// A State's own subtree for `season` (`GetNavInfo`'s `tree`, read with
    /// a token of `state_list`, the State's list in that season): the State
    /// and the levels below it, where some States' Areas are the only place
    /// they are listed ([`crate::area::complete_areas`]).
    async fn state_tree(&self, season: i64, state_list: i64) -> Result<Vec<Region>>;

    /// A school's division chain for `season` (`TeamNav/Team`).
    async fn team_divisions(&self, school_id: i64, season: i64) -> Result<TeamNav>;

    /// One page of a rankings list, as Athletic.net answered it (unchecked:
    /// [`crate::list::rankings_list`] checks that it is the list asked for).
    async fn rankings_page(&self, q: &PageQuery<'_>) -> Result<RankingsPage>;

    /// A meet: its name, date, season, location and divisions
    /// (`Meet/GetMeetData`).
    async fn meet(&self, meet_id: i64) -> Result<MeetData>;

    /// A meet's events, and which event and division pairs have entries
    /// (`Meet/GetEventListData`).
    async fn meet_events(&self, meet_id: i64) -> Result<MeetEventList>;

    /// One event's entries in one division of a meet (`Meet/GetEntriesData`).
    async fn meet_entries(&self, q: &EntriesQuery<'_>) -> Result<EntriesData>;
}

/// A [`DataSource`] backed by recorded JSON - no network.
///
/// [`FixtureSource::from_dir`] loads a directory laid out as:
///
/// - `<athlete_id>.json`: a bio (`GetAthleteBioData`).
/// - `navinfo/<season>.json`: the season's `GetNavInfo` response (its tree);
///   a season without a file is one Athletic.net does not have.
/// - `navinfo/<season>-<state_list>.json`: the `GetNavInfo` response read
///   with a token of a State's list, for its `tree` ([`DataSource::state_tree`]).
/// - `teamnav/<school_id>-<season>.json`: a school's `TeamNav/Team` response.
/// - `rankings/<list_id>-<gender>-<event>-<grades>-p<page>.json`: one page of
///   a rankings list (`GetRankings`), with `<grades>` as `9_10` or `all`
///   ([`rankings_fixture_name`]).
/// - `meets/<meet_id>/meet.json`: a meet's `GetMeetData` response.
/// - `meets/<meet_id>/events.json`: its `GetEventListData` response.
/// - `meets/<meet_id>/entries/<gender>-<div_id>-<eventShort>.json`: one
///   event's `GetEntriesData` response in one division
///   ([`entries_fixture_name`]).
///
/// - `search/<query>.json`: an athlete search's `AutoComplete/search`
///   response, for the query as [`search_fixture_name`] spells it.
///
/// Search results can also be canned per query name via
/// [`FixtureSource::with_search`]. A recording (`ath --record-dir`) is written
/// in this same layout, by the [`layout`] paths.
#[derive(Debug, Default, Clone)]
pub struct FixtureSource {
    bios: HashMap<i64, Bio>,
    searches: HashMap<String, Vec<Candidate>>,
    trees: HashMap<i64, NavInfo>,
    state_trees: HashMap<(i64, i64), NavInfo>,
    teams: HashMap<(i64, i64), TeamNav>,
    pages: HashMap<String, RankingsPage>,
    meets: HashMap<i64, MeetData>,
    meet_events: HashMap<i64, MeetEventList>,
    entries: HashMap<(i64, String), EntriesData>,
}

impl FixtureSource {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a bio directly (handy in unit tests).
    pub fn with_bio(mut self, bio: Bio) -> Self {
        self.bios.insert(bio.athlete.id, bio);
        self
    }

    /// Register the candidates a `search(name)` should return.
    pub fn with_search(mut self, name: &str, candidates: Vec<Candidate>) -> Self {
        self.searches.insert(search_fixture_name(name), candidates);
        self
    }

    /// Register a season's tree (`GetNavInfo`), as `navinfo/<season>.json`.
    pub fn with_tree(mut self, season: i64, nav: NavInfo) -> Self {
        self.trees.insert(season, nav);
        self
    }

    /// Register a State's own subtree, as `navinfo/<season>-<state_list>.json`.
    pub fn with_state_tree(mut self, season: i64, state_list: i64, nav: NavInfo) -> Self {
        self.state_trees.insert((season, state_list), nav);
        self
    }

    /// Register the page a `rankings_page(q)` should return.
    pub fn with_rankings_page(mut self, q: &PageQuery<'_>, page: RankingsPage) -> Self {
        self.pages.insert(rankings_fixture_name(q), page);
        self
    }

    /// Load bios from `dir/*.json`, plus the `navinfo/` and `teamnav/`
    /// subdirectories when present (see [`FixtureSource`]).
    pub fn from_dir(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        let mut src = Self::default();
        for (_, bio) in read_json_dir::<Bio>(dir)? {
            src.bios.insert(bio.athlete.id, bio);
        }
        let navinfo = dir.join("navinfo");
        if navinfo.is_dir() {
            for (stem, nav) in read_json_dir::<NavInfo>(&navinfo)? {
                let bad = || {
                    format!(
                        "navinfo fixture {stem}.json is not named <season>.json or \
                         <season>-<state_list>.json"
                    )
                };
                match stem.split_once('-') {
                    None => {
                        src.trees.insert(stem.parse().with_context(bad)?, nav);
                    }
                    Some((season, list)) => {
                        let key = (
                            season.parse().with_context(bad)?,
                            list.parse().with_context(bad)?,
                        );
                        src.state_trees.insert(key, nav);
                    }
                }
            }
        }
        let teamnav = dir.join("teamnav");
        if teamnav.is_dir() {
            for (stem, nav) in read_json_dir::<TeamNav>(&teamnav)? {
                let key = stem
                    .split_once('-')
                    .and_then(|(school, season)| Some((school.parse().ok()?, season.parse().ok()?)))
                    .with_context(|| {
                        format!(
                            "teamnav fixture {stem}.json is not named <school_id>-<season>.json"
                        )
                    })?;
                src.teams.insert(key, nav);
            }
        }
        let rankings = dir.join("rankings");
        if rankings.is_dir() {
            src.pages.extend(read_json_dir::<RankingsPage>(&rankings)?);
        }
        let meets = dir.join("meets");
        if meets.is_dir() {
            src.read_meets(&meets)?;
        }
        let search = dir.join("search");
        if search.is_dir() {
            for (stem, raw) in read_json_dir::<serde_json::Value>(&search)? {
                let candidates = crate::athletic::search_candidates(&raw.to_string())
                    .with_context(|| format!("reading search fixture {stem}.json"))?;
                src.searches.insert(stem, candidates);
            }
        }
        Ok(src)
    }

    /// Every `meets/<meet_id>/` directory (see [`FixtureSource`]).
    fn read_meets(&mut self, meets: &Path) -> Result<()> {
        for entry in std::fs::read_dir(meets)
            .with_context(|| format!("reading fixtures dir {}", meets.display()))?
        {
            let dir = entry?.path();
            if !dir.is_dir() {
                continue;
            }
            let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            let meet_id: i64 = name.parse().with_context(|| {
                format!("meet fixture dir {} is not named <meet_id>", dir.display())
            })?;
            let meet = dir.join("meet.json");
            if meet.is_file() {
                self.meets.insert(meet_id, read_json(&meet)?);
            }
            let events = dir.join("events.json");
            if events.is_file() {
                self.meet_events.insert(meet_id, read_json(&events)?);
            }
            let entries = dir.join("entries");
            if entries.is_dir() {
                for (stem, data) in read_json_dir::<EntriesData>(&entries)? {
                    self.entries.insert((meet_id, stem), data);
                }
            }
        }
        Ok(())
    }
}

/// One JSON fixture file, parsed.
fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading fixture {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing fixture {}", path.display()))
}

/// Every `*.json` file directly in `dir`, parsed, with its file stem.
fn read_json_dir<T: DeserializeOwned>(dir: &Path) -> Result<Vec<(String, T)>> {
    let mut out = Vec::new();
    for entry in
        std::fs::read_dir(dir).with_context(|| format!("reading fixtures dir {}", dir.display()))?
    {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let parsed: T = read_json(&path)?;
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        out.push((stem, parsed));
    }
    Ok(out)
}

/// The fixture file stem for a rankings page:
/// `<list_id>-<gender>-<event>-<grades>-p<page>`, grades as `9_10` or `all`.
pub fn rankings_fixture_name(q: &PageQuery<'_>) -> String {
    let grades = if q.grades.is_all() { "all".to_string() } else { q.grades.joined("_") };
    format!(
        "{}-{}-{}-{grades}-p{}",
        q.list_id,
        q.gender.as_str(),
        path_component(q.event),
        q.page
    )
}

/// A name from Athletic.net or the command line (an event code) as one safe
/// part of a fixture's file name: as it is when it is only ASCII letters and
/// digits (`400m`, `Shot`, `4x800m`), otherwise its letters and digits in
/// lower case, runs of anything else as one `-`, then the first 8 hex digits
/// of the SHA-256 of the name (the search names' rule, [`search_fixture_name`],
/// keeping case), so `../x` or `a/b` can never name another folder.
pub fn path_component(name: &str) -> String {
    if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric()) {
        return name.to_string();
    }
    use sha2::{Digest, Sha256};
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut out = out.trim_end_matches('-').to_string();
    let digest = Sha256::digest(name.as_bytes());
    let hash: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
    if !out.is_empty() {
        out.push('-');
    }
    out.push_str(&hash);
    out
}

/// The fixture file stem for one event's entries in a meet's division:
/// `<gender>-<div_id>-<eventShort>` (`m-13-800m`), the entries page's own
/// path parts.
pub fn entries_fixture_name(q: &EntriesQuery<'_>) -> String {
    format!(
        "{}-{}-{}",
        q.gender.as_str(),
        q.div_id,
        path_component(q.event_short)
    )
}

/// The fixture file stem for an athlete search: the query trimmed and
/// lower-cased, every run of characters other than ASCII letters and digits
/// one `-` (`"Jane Doe"` is `jane-doe`). Unless that spelling is exact (the
/// query is ASCII letters and digits in words one space apart, so the name
/// reads back as it), the first 8 hex digits of the SHA-256 of the
/// lower-cased query follow: `Mary-Kate Doe`, `O'Neil` and `José Núñez` each
/// get their own name, never `Mary Kate Doe`'s, `O Neil`'s or `Jos N ez`'s,
/// and no name is empty.
/// `tools/anonymize/anonymize.py` (`search_fixture_name`) spells it the same.
pub fn search_fixture_name(name: &str) -> String {
    use sha2::{Digest, Sha256};
    let query = name.trim();
    let mut out = String::new();
    for c in query.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut out = out.trim_end_matches('-').to_string();
    if out.is_empty() || out.replace('-', " ") != query.to_lowercase() {
        let digest = Sha256::digest(query.to_lowercase().as_bytes());
        let hash: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
        if !out.is_empty() {
            out.push('-');
        }
        out.push_str(&hash);
    }
    out
}

/// Where each response lives in a fixtures dir ([`FixtureSource`]), relative
/// to it: what `--record-dir` writes and `--fixtures-dir` reads.
pub mod layout {
    use super::*;

    /// `<athlete_id>.json`.
    pub fn bio(athlete_id: i64) -> PathBuf {
        PathBuf::from(format!("{athlete_id}.json"))
    }

    /// `teamnav/<school_id>-<season>.json`.
    pub fn teamnav(school_id: i64, season: i64) -> PathBuf {
        Path::new("teamnav").join(format!("{school_id}-{season}.json"))
    }

    /// `navinfo/<season>.json`: the season's tree.
    pub fn navinfo(season: i64) -> PathBuf {
        Path::new("navinfo").join(format!("{season}.json"))
    }

    /// `navinfo/<season>-<state_list>.json`: a State's own subtree.
    pub fn state_navinfo(season: i64, state_list: i64) -> PathBuf {
        Path::new("navinfo").join(format!("{season}-{state_list}.json"))
    }

    /// `rankings/<list_id>-<gender>-<event>-<grades>-p<page>.json`.
    pub fn rankings(q: &PageQuery<'_>) -> PathBuf {
        Path::new("rankings").join(format!("{}.json", rankings_fixture_name(q)))
    }

    /// `meets/<meet_id>/`.
    fn meet_dir(meet_id: i64) -> PathBuf {
        Path::new("meets").join(meet_id.to_string())
    }

    /// `meets/<meet_id>/meet.json`.
    pub fn meet(meet_id: i64) -> PathBuf {
        meet_dir(meet_id).join("meet.json")
    }

    /// `meets/<meet_id>/events.json`.
    pub fn meet_events(meet_id: i64) -> PathBuf {
        meet_dir(meet_id).join("events.json")
    }

    /// `meets/<meet_id>/entries/<gender>-<div_id>-<eventShort>.json`.
    pub fn meet_entries(q: &EntriesQuery<'_>) -> PathBuf {
        meet_dir(q.meet_id)
            .join("entries")
            .join(format!("{}.json", entries_fixture_name(q)))
    }

    /// `search/<query>.json` ([`search_fixture_name`]).
    pub fn search(name: &str) -> PathBuf {
        Path::new("search").join(format!("{}.json", search_fixture_name(name)))
    }
}

#[async_trait]
impl DataSource for FixtureSource {
    async fn get_bio(&self, athlete_id: i64) -> Result<Bio> {
        self.bios
            .get(&athlete_id)
            .cloned()
            .with_context(|| format!("no fixture bio for athlete {athlete_id}"))
    }

    async fn search(&self, name: &str) -> Result<Vec<Candidate>> {
        Ok(self
            .searches
            .get(&search_fixture_name(name))
            .cloned()
            .unwrap_or_default())
    }

    async fn area_tree(&self, season: i64, _from_list: i64) -> Result<Option<AreaTree>> {
        Ok(self
            .trees
            .get(&season)
            .cloned()
            .map(|nav| AreaTree::from_nav(season, nav)))
    }

    async fn season_area_tree(&self, season: i64, _list_id: i64) -> Result<AreaTree> {
        self.area_tree(season, 0)
            .await?
            .with_context(|| format!("no fixture navinfo for {season}"))
    }

    async fn state_tree(&self, season: i64, state_list: i64) -> Result<Vec<Region>> {
        self.state_trees
            .get(&(season, state_list))
            .map(|nav| nav.tree.clone())
            .with_context(|| format!("no fixture navinfo/{season}-{state_list}.json"))
    }

    async fn team_divisions(&self, school_id: i64, season: i64) -> Result<TeamNav> {
        self.teams
            .get(&(school_id, season))
            .cloned()
            .with_context(|| format!("no fixture TeamNav for school {school_id} in {season}"))
    }

    async fn rankings_page(&self, q: &PageQuery<'_>) -> Result<RankingsPage> {
        let name = rankings_fixture_name(q);
        self.pages
            .get(&name)
            .cloned()
            .with_context(|| format!("no fixture rankings page rankings/{name}.json"))
    }

    async fn meet(&self, meet_id: i64) -> Result<MeetData> {
        self.meets
            .get(&meet_id)
            .cloned()
            .with_context(|| format!("no fixture meets/{meet_id}/meet.json"))
    }

    async fn meet_events(&self, meet_id: i64) -> Result<MeetEventList> {
        self.meet_events
            .get(&meet_id)
            .cloned()
            .with_context(|| format!("no fixture meets/{meet_id}/events.json"))
    }

    async fn meet_entries(&self, q: &EntriesQuery<'_>) -> Result<EntriesData> {
        let name = entries_fixture_name(q);
        self.entries
            .get(&(q.meet_id, name.clone()))
            .cloned()
            .with_context(|| format!("no fixture meets/{}/entries/{name}.json", q.meet_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_fixture_names_are_unique_and_never_empty() {
        assert_eq!(search_fixture_name(" Jane Doe "), "jane-doe");
        assert_eq!(search_fixture_name("MARY KATE DOE"), "mary-kate-doe");
        assert_eq!(
            search_fixture_name("Mary-Kate Doe"),
            "mary-kate-doe-74813ee1"
        );
        assert_eq!(search_fixture_name("O'Neil"), "o-neil-eb1aed4f");
        assert_eq!(search_fixture_name("O Neil"), "o-neil");
        assert_eq!(search_fixture_name("Jane  Doe"), "jane-doe-760946a8");
        // The anonymizer's `search_fixture_name` gives these same names.
        let accented = search_fixture_name("José Núñez");
        assert_eq!(accented, "jos-n-ez-43884fd3");
        assert_ne!(accented, search_fixture_name("Jos N ez"));
        assert_eq!(accented, search_fixture_name("JOSÉ NÚÑEZ"));
        let cjk = search_fixture_name("山田 太郎");
        assert_eq!(path_component("4x800m"), "4x800m");
        assert_eq!(path_component("Shot"), "Shot");
        let up = path_component("../x");
        assert!(up.starts_with("x-") && up.len() == 10, "{up}");
        assert_ne!(path_component("a/b"), path_component("a-b"));
        assert!(!path_component("a/b").contains('/'));
        assert_eq!(path_component("").len(), 8);
        assert_eq!(cjk, "ed6d53c9");
        assert_ne!(cjk, search_fixture_name("鈴木 一郎"));
    }
}
