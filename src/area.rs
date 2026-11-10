//! Areas: the level directly below a State in Athletic.net's hierarchy
//! (`CONTEXT.md`), with `ath areas` and a Profile's Area.
//!
//! Two Athletic.net answers feed this module, both read through the
//! [`DataSource`] seam:
//!
//! - A season's division tree ([`AreaTree`]), from the rankings navigation
//!   endpoint `tfRankings/GetNavInfo`: every State and every level below it,
//!   with each division's list id for that season. The national `regions`
//!   lack some States' levels (Maine's Classes); those come from the State's
//!   own subtree ([`complete_areas`]).
//! - A school's division chain for a season ([`TeamNav`]), from
//!   `TeamNav/Team`: the lists the school is on (United States, High School,
//!   its State, its Area, its league...).
//!
//! Which level of the tree counts as a State's Area is configured per State,
//! in `areas/<ST>.json` ([`configured_states`], `docs/areas.md`); a State
//! without a file has no Areas. A school is
//! in an Area when its chain holds one of that season's Area lists for its
//! State: Athletic.net's lists decide membership, `ath` never infers it.
//! Membership is by school, never club.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::contract::{Report, columns};
use crate::model::{Bio, SchoolBasis, SchoolPick};
use crate::source::DataSource;

mod files {
    include!(concat!(env!("OUT_DIR"), "/area_files.rs"));
}

/// One State's Area configuration: an `areas/<ST>.json` file (`docs/areas.md`).
///
/// Athletic.net names the level below a State differently in each State
/// (California `Section`, New York `Association`, Texas `League`), and in some
/// States that level is not what people mean by an Area, so each State is
/// added deliberately, as a file. Fields `ath` does not read (the label
/// pages show, the counties a map colors) are for the reports under `tools/`.
#[derive(Debug, Deserialize)]
pub struct StateAreas {
    /// Two-letter code.
    pub state: String,
    /// The division type (`divType`) of the Area level in Athletic.net's
    /// tree: `Section` for California.
    pub level: String,
    /// The State's Areas, by Athletic.net's names.
    pub areas: Vec<AreaConfig>,
}

/// One Area in a State's file.
#[derive(Debug, Deserialize)]
pub struct AreaConfig {
    /// Athletic.net's name for it (`Central Coast`).
    pub name: String,
    /// The abbreviation people use for it (`CCS`), which `--in` and
    /// `--area` also take. Athletic.net's data has only the names.
    pub abbr: Option<String>,
}

/// Every configured State, parsed once from the embedded `areas/*.json`. A
/// file that does not parse is a build mistake, caught by the
/// `area_files_are_valid` test.
pub fn configured_states() -> &'static [StateAreas] {
    static PARSED: std::sync::OnceLock<Vec<StateAreas>> = std::sync::OnceLock::new();
    PARSED.get_or_init(|| {
        files::AREA_FILES
            .iter()
            .map(|(name, text)| {
                serde_json::from_str(text).unwrap_or_else(|e| panic!("areas/{name}: {e}"))
            })
            .collect()
    })
}

/// A State's configuration, if it has one.
fn configured(state: &str) -> Option<&'static StateAreas> {
    let state = state.trim();
    configured_states()
        .iter()
        .find(|s| s.state.eq_ignore_ascii_case(state))
}

/// The abbreviation configured for an Area, if any.
fn area_abbreviation(state: &str, name: &str) -> Option<&'static str> {
    configured(state)?
        .areas
        .iter()
        .find(|a| a.name.eq_ignore_ascii_case(name.trim()))?
        .abbr
        .as_deref()
}

/// The Area level (`divType`) configured for a State, if any.
pub fn area_level(state: &str) -> Option<&'static str> {
    configured(state).map(|s| s.level.as_str())
}

/// The rankings list `ath areas` starts from when it has no school to start
/// from: California's 2026 high school list. It is only used for the token
/// its rankings response carries (`GetNavInfo`'s `seasons` then leads to the
/// season asked for); every list id `ath` reports is discovered. `--from-list`
/// overrides it.
pub const DEFAULT_FROM_LIST: i64 = 168_546;

/// The event a list link opens on when no event is known. Athletic.net's
/// rankings list page always shows one event; the 100m is the first it lists.
pub const DEFAULT_LINK_EVENT: &str = "100m";

/// Athletic.net's `eventShort` for a flat race given as `400m` or
/// `400 Meters` (any case): `400m`. `None` for anything else.
pub fn flat_race_short(event: &str) -> Option<String> {
    let e = event.trim().to_ascii_lowercase();
    let meters = e
        .strip_suffix(" meters")
        .or_else(|| e.strip_suffix('m'))?
        .trim();
    (!meters.is_empty() && meters.chars().all(|c| c.is_ascii_digit())).then(|| format!("{meters}m"))
}

/// Refuse an `--event` that is a bare number (`400`): no event is named so,
/// so it would match nothing; most likely a flat race without its unit.
/// Every command's `--event` refuses it with the same message.
pub fn refuse_bare_number(event: &str) -> Result<()> {
    let code = event.trim();
    if !code.is_empty() && code.chars().all(|c| c.is_ascii_digit()) {
        bail!("--event {code:?} is a bare number; did you mean {code}m?");
    }
    Ok(())
}

/// The `eventShort` for `ath areas --event`: a flat race in either form, or
/// an Athletic.net code such as `Shot` or `LJ` as given.
pub fn link_event(event: &str) -> Result<String> {
    if let Some(short) = flat_race_short(event) {
        return Ok(short);
    }
    refuse_bare_number(event)?;
    let code = event.trim();
    if !code.is_empty() && code.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Ok(code.to_string());
    }
    bail!(
        "--event takes a flat race (\"400m\" or \"400 Meters\") or an Athletic.net event code \
         such as Shot or LJ, not {event:?}"
    )
}

/// Athletic.net's rankings list page for a division (State or Area), on one
/// event (`eventShort`).
pub fn rankings_list_url(list_id: i64, gender: Gender, event: &str) -> String {
    format!(
        "https://www.athletic.net/TrackAndField/rankings/list/{list_id}/{}/{event}",
        gender.as_str()
    )
}

/// Which gender's lists to link to. Division ids are the same for both (the
/// gender is a separate part of a list's URL), so trees are fetched once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Gender {
    F,
    M,
}

impl Gender {
    pub fn as_str(self) -> &'static str {
        match self {
            Gender::F => "f",
            Gender::M => "m",
        }
    }

    /// `girls`, `boys`.
    pub fn noun(self) -> &'static str {
        match self {
            Gender::F => "girls",
            Gender::M => "boys",
        }
    }

    /// `girls'`, `boys'`.
    pub fn possessive(self) -> &'static str {
        match self {
            Gender::F => "girls'",
            Gender::M => "boys'",
        }
    }

    /// From a bio's `F` / `M`; anything else is `None`.
    pub fn from_bio(s: &str) -> Option<Gender> {
        match s.trim() {
            "f" | "F" => Some(Gender::F),
            "m" | "M" => Some(Gender::M),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Athletic.net responses

/// `GET tfRankings/GetNavInfo` (only the fields we use). The response also
/// carries `divListId, levels, levelDivId, countries, countryDivId,
/// regionDivId, events, events_main, recordSets`.
#[derive(Debug, Clone, Deserialize)]
pub struct NavInfo {
    /// Season -> the list id of the token's division in that season (how a
    /// list in one season leads to the same place in another).
    #[serde(default)]
    pub seasons: HashMap<String, serde_json::Value>,
    /// Every division of the season's tree: the high school level, each
    /// State, and the levels below them.
    #[serde(default)]
    pub regions: Vec<Region>,
    /// The subtree of the token's own division: that division at depth 0 and
    /// the levels below it. The only place some States' Areas are listed
    /// (Maine's Classes are not in `regions`).
    #[serde(default)]
    pub tree: Vec<Region>,
}

impl NavInfo {
    /// The list id for `season`, if Athletic.net has that season.
    pub fn season_list_id(&self, season: i64) -> Option<i64> {
        let v = self.seasons.get(&season.to_string())?;
        v.as_i64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
    }

    /// Whether this is a usable tree: it lists States.
    pub fn has_states(&self) -> bool {
        self.regions.iter().any(|r| r.div_type == "State")
    }

    /// Whether this is the national high school tree: it has the `High
    /// School` level and a State for nearly every State (see
    /// [`NATIONAL_STATES`]). A token for some other list can answer with a
    /// smaller tree, which is fine for one answer but must never be kept as
    /// the season's tree.
    pub fn is_national(&self) -> bool {
        let high_school = self
            .regions
            .iter()
            .any(|r| r.div_type == "Level" && r.name.trim().eq_ignore_ascii_case("High School"));
        let states: HashSet<&str> = self
            .regions
            .iter()
            .filter(|r| r.div_type == "State" && !r.is_custom())
            .filter_map(|r| r.state.as_deref().map(str::trim))
            .filter_map(|s| {
                US_STATES
                    .iter()
                    .find(|c| c.eq_ignore_ascii_case(s))
                    .copied()
            })
            .collect();
        high_school && states.len() >= NATIONAL_STATES
    }
}

/// The 50 States and DC.
pub const US_STATES: [&str; 51] = [
    "AL", "AK", "AZ", "AR", "CA", "CO", "CT", "DE", "DC", "FL", "GA", "HI", "ID", "IL", "IN", "IA",
    "KS", "KY", "LA", "ME", "MD", "MA", "MI", "MN", "MS", "MO", "MT", "NE", "NV", "NH", "NJ", "NM",
    "NY", "NC", "ND", "OH", "OK", "OR", "PA", "RI", "SC", "SD", "TN", "TX", "UT", "VT", "VA", "WA",
    "WV", "WI", "WY",
];

/// How many of [`US_STATES`] a tree must list to be the national tree (one
/// may be missing, should Athletic.net drop a State for a season).
pub const NATIONAL_STATES: usize = 50;

/// One division in a season's tree: `{id, name, state, divType, subDivType,
/// depth, customDivision}`.
#[derive(Debug, Clone, Deserialize)]
pub struct Region {
    pub id: i64,
    #[serde(default)]
    pub name: String,
    /// Two-letter State code; null for levels above a State.
    #[serde(default)]
    pub state: Option<String>,
    /// `Level`, `State`, `Section`, `Association`, `League`, ...
    #[serde(rename = "divType", default)]
    pub div_type: String,
    #[serde(default)]
    pub depth: Option<i64>,
    /// A user-made list, not part of the official hierarchy.
    #[serde(rename = "customDivision", default)]
    pub custom_division: serde_json::Value,
}

impl Region {
    fn is_custom(&self) -> bool {
        match &self.custom_division {
            serde_json::Value::Bool(b) => *b,
            serde_json::Value::Number(n) => n.as_i64().is_some_and(|n| n != 0),
            _ => false,
        }
    }

    fn in_state(&self, state: &str) -> bool {
        self.state
            .as_deref()
            .is_some_and(|s| s.trim().eq_ignore_ascii_case(state))
    }

    fn list(&self, gender: Gender, event: &str) -> ListRef {
        ListRef {
            id: self.id,
            name: self.name.trim().to_string(),
            url: rankings_list_url(self.id, gender, event),
        }
    }
}

/// A season's division tree.
#[derive(Debug, Clone)]
pub struct AreaTree {
    pub season: i64,
    pub regions: Vec<Region>,
    /// Whether this is the national tree ([`NavInfo::is_national`]). A
    /// partial one may lack a State that exists.
    pub national: bool,
}

impl AreaTree {
    /// The tree from a `GetNavInfo` response for `season`.
    pub fn from_nav(season: i64, nav: NavInfo) -> AreaTree {
        AreaTree {
            season,
            national: nav.is_national(),
            regions: nav.regions,
        }
    }

    /// What to add to "no such State" when this tree may just not show it.
    pub fn partial_note(&self) -> &'static str {
        if self.national {
            ""
        } else {
            " (Athletic.net answered with only part of the national high school tree, so the \
             State may still exist)"
        }
    }

    /// What to add to "no Area lists" when this tree may just not show them.
    pub fn partial_areas_note(&self) -> &'static str {
        if self.national {
            ""
        } else {
            " (Athletic.net answered with only part of the national high school tree, so they \
             may still exist; --refresh asks again)"
        }
    }

    /// A State's own division.
    pub fn state(&self, state: &str) -> Option<&Region> {
        self.regions
            .iter()
            .find(|r| r.div_type == "State" && r.in_state(state) && !r.is_custom())
    }

    /// A State's divisions of type `level` (its Areas when `level` is its
    /// configured Area level), sorted by name.
    pub fn areas(&self, state: &str, level: &str) -> Vec<&Region> {
        let mut areas: Vec<&Region> = self
            .regions
            .iter()
            .filter(|r| r.div_type == level && r.in_state(state) && !r.is_custom())
            .collect();
        areas.sort_by(|a, b| (a.name.trim(), a.id).cmp(&(b.name.trim(), b.id)));
        areas
    }
}

/// `GET TeamNav/Team?team=<id>&sport=tf&season=<year>` (only the fields we
/// use). The response also carries `userAthleteIdOnTeam, hasCoachAccess,
/// mascotToken2, grades, customLists, customDivisions, linkedTeams`.
#[derive(Debug, Clone, Deserialize)]
pub struct TeamNav {
    pub team: TeamInfo,
    /// The school's chain for the season, top down. Empty when the school is
    /// on no lists that season.
    #[serde(default)]
    pub divisions: Vec<TeamDivision>,
}

impl TeamNav {
    /// The State link of the chain: the one right below the `High School`
    /// level (United States, High School, California, Central Coast, ...).
    pub fn state_list_id(&self) -> Option<i64> {
        let hs = self
            .divisions
            .iter()
            .position(|d| d.name.trim().eq_ignore_ascii_case("High School"))?;
        self.divisions.get(hs + 1).map(|d| d.id)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TeamInfo {
    #[serde(rename = "ID")]
    pub id: i64,
    #[serde(rename = "Name", default)]
    pub name: String,
    /// 4 is high school (the same scale as a bio's team `Level`).
    #[serde(rename = "Level", default)]
    pub level: i64,
    #[serde(rename = "City", default)]
    pub city: Option<String>,
    #[serde(rename = "State", default)]
    pub state: Option<String>,
}

/// One link of a school's division chain.
#[derive(Debug, Clone, Deserialize)]
pub struct TeamDivision {
    /// The division's list id for the season.
    pub id: i64,
    /// The base division id, the same in every season (Central Coast is 355
    /// whatever its season's list id).
    #[serde(default)]
    pub b: Option<i64>,
    /// Athletic.net pads names with leading spaces; trim before showing.
    #[serde(default)]
    pub name: String,
}

/// A rankings response (only what reading a tree needs).
#[derive(Debug, Clone, Deserialize)]
pub struct RankingsToken {
    /// The token `GetNavInfo` takes in its `anettokens` header.
    #[serde(rename = "jwtTFTopReport")]
    pub token: String,
    #[serde(default)]
    pub division: Option<RankingsDivision>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RankingsDivision {
    #[serde(rename = "ID", default)]
    pub id: Option<i64>,
    #[serde(rename = "SeasonID", default)]
    pub season_id: Option<i64>,
    /// The list's name (`Central Coast`).
    #[serde(rename = "Name", default)]
    pub name: Option<String>,
}

// ---------------------------------------------------------------------------
// `ath areas`

/// A rankings list: a State or an Area.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListRef {
    /// The list (division) id for the season.
    pub id: i64,
    pub name: String,
    /// The Athletic.net rankings list page, on the report's link event.
    pub url: String,
}

/// A State's Areas for one season, as `ath areas` prints them.
#[derive(Debug, Clone, Serialize)]
pub struct Areas {
    /// Two-letter State code, upper case.
    pub state: String,
    pub season: i64,
    /// The gender the list links are for.
    pub gender: Gender,
    /// The event (`eventShort`) the list links open on.
    pub event: String,
    /// The configured Area level (`Section` for California); `null` for a
    /// State without a configuration.
    pub level: Option<String>,
    /// The State's own list.
    pub state_list: ListRef,
    /// The State's Areas, by name; empty for a State without a configuration.
    pub areas: Vec<ListRef>,
    /// Why there are no Areas, for a State without a configuration.
    pub note: Option<String>,
}

/// What `ath areas` is asked for.
#[derive(Debug, Clone)]
pub struct AreasQuery<'a> {
    pub state: &'a str,
    pub season: i64,
    /// For the links only: the tree is the same for both.
    pub gender: Gender,
    /// The links' event (`eventShort`).
    pub event: String,
    /// The rankings list to start from (any season), for its token.
    pub from_list: i64,
}

/// The note for a State without an Area level configured.
fn unconfigured_note(state: &str) -> String {
    format!(
        "No Area level is configured for {state}, so it lists no Areas \
         (configured: {}; add an areas/<ST>.json to configure one).",
        configured_states()
            .iter()
            .map(|s| format!("{} {}", s.state, s.level))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// `ath areas <STATE> --season <YEAR>`: the State's list and its Areas'
/// lists for the season.
pub async fn areas<S: DataSource + ?Sized>(source: &S, q: &AreasQuery<'_>) -> Result<Areas> {
    let state = state_code(q.state)?;
    let season = q.season;
    let mut tree = season_tree(source, season, q.from_list).await?;
    complete_areas(source, &mut tree, &[&state]).await?;
    let state_region = state_list(&tree, &state)?;
    let level = area_level(&state);
    let (gender, event) = (q.gender, q.event.as_str());
    let areas = level
        .map(|level| {
            tree.areas(&state, level)
                .into_iter()
                .map(|r| r.list(gender, event))
                .collect()
        })
        .unwrap_or_default();
    Ok(Areas {
        note: level.is_none().then(|| unconfigured_note(&state)),
        state_list: state_region.list(gender, event),
        level: level.map(str::to_string),
        areas,
        state,
        season,
        gender,
        event: event.to_string(),
    })
}

/// A two-letter State code, upper case.
pub fn state_code(state: &str) -> Result<String> {
    let state = state.trim().to_ascii_uppercase();
    if state.len() != 2 || !state.chars().all(|c| c.is_ascii_alphabetic()) {
        bail!("{state:?} is not a two-letter State code (e.g. CA)");
    }
    Ok(state)
}

/// The high school division tree for `season`, found from `from_list`, or
/// an error saying Athletic.net has no such season yet.
pub async fn season_tree<S: DataSource + ?Sized>(
    source: &S,
    season: i64,
    from_list: i64,
) -> Result<AreaTree> {
    source.area_tree(season, from_list).await?.with_context(|| {
        format!("Athletic.net has no {season} season yet (no such season in its rankings)")
    })
}

/// A State's own list in `tree` (`state` a code from [`state_code`]), or an
/// error that says when the tree may just not show it.
pub fn state_list<'t>(tree: &'t AreaTree, state: &str) -> Result<&'t Region> {
    tree.state(state).with_context(|| {
        let hint = if tree.national || !US_STATES.contains(&state) {
            String::new()
        } else {
            format!(
                "{}; try --from-list with a State's high school list",
                tree.partial_note()
            )
        };
        format!(
            "Athletic.net's {} high school lists have no State {state}{hint}",
            tree.season
        )
    })
}

/// Add the Areas of each of `states` that has an Area file but no Areas in
/// `tree`, from the State's own subtree ([`DataSource::state_tree`]); the
/// first State that cannot be completed is an error ([`complete_area`]).
pub async fn complete_areas<S: DataSource + ?Sized>(
    source: &S,
    tree: &mut AreaTree,
    states: &[&str],
) -> Result<()> {
    for st in states {
        complete_area(source, tree, st).await?;
    }
    Ok(())
}

/// Add `state`'s Areas to `tree` from the State's own subtree when it has an
/// Area file but `tree` shows none of its Areas. Athletic.net's national
/// `regions` carry some States' levels (California's Sections) and not
/// others (Maine's Classes), so a State whose Areas are missing is asked for
/// its own. Only the national tree is completed: a partial one may just not
/// show a State's Areas ([`AreaTree::partial_areas_note`]). A State the tree
/// does not show, or one without an Area file, is left alone.
pub async fn complete_area<S: DataSource + ?Sized>(
    source: &S,
    tree: &mut AreaTree,
    state: &str,
) -> Result<()> {
    let Some(c) = configured(state) else {
        return Ok(());
    };
    let Some(list_id) = tree.state(&c.state).map(|r| r.id) else {
        return Ok(());
    };
    if !tree.national || !tree.areas(&c.state, &c.level).is_empty() {
        return Ok(());
    }
    let sub = source.state_tree(tree.season, list_id).await?;
    let areas = state_areas(&sub, &c.state, &c.level, list_id, tree.season)?;
    tree.regions.extend(areas);
    Ok(())
}

/// The Area `name` names ([`area_named`]), completing the tree first only
/// when it does not already know the name: with `state`, that State's
/// Areas; without, every configured State's (a California abbreviation
/// never waits on Maine's subtree).
pub async fn find_area_named<'t, S: DataSource + ?Sized>(
    source: &S,
    tree: &'t mut AreaTree,
    state: Option<&str>,
    name: &str,
) -> Result<NamedArea<'t>> {
    if area_named(tree, state, name).is_err() {
        let states: Vec<&str> = match state {
            Some(s) => vec![s],
            None => configured_codes(),
        };
        complete_areas(source, tree, &states).await?;
    }
    area_named(tree, state, name)
}

/// Every State with an Area file: what [`complete_areas`] needs when any
/// State's Area may be named.
pub fn configured_codes() -> Vec<&'static str> {
    configured_states()
        .iter()
        .map(|s| s.state.as_str())
        .collect()
}

/// The Areas in a State's own subtree (`sub`, from a token of its list
/// `list_id`): the divisions right below the State. Each must be of the
/// State's configured `level`; anything else means Athletic.net changed the
/// State's hierarchy, so it is an error rather than a guess.
fn state_areas(
    sub: &[Region],
    state: &str,
    level: &str,
    list_id: i64,
    season: i64,
) -> Result<Vec<Region>> {
    let root = sub
        .iter()
        .find(|r| r.div_type == "State" && r.id == list_id)
        .with_context(|| {
            format!(
                "Athletic.net's {season} tree for {state}'s list {list_id} does not start at it"
            )
        })?;
    let depth = root.depth.unwrap_or(0) + 1;
    let children: Vec<Region> = sub
        .iter()
        .filter(|r| r.in_state(state) && r.depth == Some(depth) && !r.is_custom())
        .cloned()
        .collect();
    let other: Vec<String> = children
        .iter()
        .filter(|r| r.div_type != level)
        .map(|r| format!("{} ({})", r.name.trim(), r.div_type))
        .collect();
    if !other.is_empty() {
        bail!(
            "Athletic.net's {season} lists below {state} are not of the level areas/{state}.json \
             names ({level}): {}",
            other.join(", ")
        );
    }
    Ok(children)
}

/// An Area found by [`area_named`].
#[derive(Debug, Clone, Copy)]
pub struct NamedArea<'t> {
    pub state: &'static str,
    /// The State's Area level (`Section`).
    pub level: &'static str,
    pub region: &'t Region,
}

/// Every Area of `state` in `tree`, by name; `None` when the State has no
/// Area level configured.
pub fn areas_of<'t>(tree: &'t AreaTree, state: &str) -> Option<Vec<NamedArea<'t>>> {
    let c = configured(state)?;
    Some(
        tree.areas(&c.state, &c.level)
            .into_iter()
            .map(|region| NamedArea {
                state: &c.state,
                level: &c.level,
                region,
            })
            .collect(),
    )
}

/// The Area `name` names in `tree`: its name or its abbreviation (`Central
/// Coast`, `CCS`; any case), within `state` when given, otherwise in any State
/// with an Area level configured. Fails with the Areas there are when none
/// matches, and asks for a State when the name is in more than one.
pub fn area_named<'t>(
    tree: &'t AreaTree,
    state: Option<&str>,
    name: &str,
) -> Result<NamedArea<'t>> {
    let wanted = name.trim();
    if let Some(st) = state
        && area_level(st).is_none()
    {
        bail!("{}", unconfigured_note(st));
    }
    let mut found = Vec::new();
    let mut known = Vec::new();
    for config in configured_states() {
        let (st, level) = (config.state.as_str(), config.level.as_str());
        if state.is_some_and(|s| !s.trim().eq_ignore_ascii_case(st)) {
            continue;
        }
        for region in tree.areas(st, level) {
            let area = region.name.trim();
            let abbr = area_abbreviation(st, area);
            if area.eq_ignore_ascii_case(wanted)
                || abbr.is_some_and(|a| a.eq_ignore_ascii_case(wanted))
            {
                found.push(NamedArea {
                    state: st,
                    level,
                    region,
                });
            }
            known.push(match abbr {
                Some(a) => format!("{area} ({a})"),
                None => area.to_string(),
            });
        }
    }
    match found.as_slice() {
        [one] => Ok(*one),
        [] => bail!(
            "no {} Area is named {wanted:?}{}; the {} Areas are: {}",
            tree.season,
            state.map_or_else(String::new, |s| format!(
                " in {}",
                s.trim().to_ascii_uppercase()
            )),
            tree.season,
            if known.is_empty() { "none".to_string() } else { known.join(", ") }
        ),
        many => bail!(
            "{wanted:?} names an Area in more than one State ({}); add --state",
            many.iter().map(|a| a.state).collect::<Vec<_>>().join(", ")
        ),
    }
}

impl Report for Areas {
    const SCHEMA: u32 = 1;

    fn table(&self) -> String {
        let mut out = format!(
            "{} ({}) {} - Area level: {}\n",
            self.state_list.name,
            self.state,
            self.season,
            self.level.as_deref().unwrap_or("none")
        );
        if let Some(note) = &self.note {
            out.push_str(note);
            out.push('\n');
        }
        out.push('\n');
        let mut rows = vec![vec![
            "List".to_string(),
            "Kind".into(),
            "Id".into(),
            "URL".into(),
        ]];
        rows.push(list_row(&self.state_list, "state"));
        rows.extend(self.areas.iter().map(|a| list_row(a, "area")));
        out.push_str(&columns(&rows));
        out
    }
}

fn list_row(l: &ListRef, kind: &str) -> Vec<String> {
    vec![l.name.clone(), kind.into(), l.id.to_string(), l.url.clone()]
}

// ---------------------------------------------------------------------------
// A Profile's Area

/// A Profile's Area for one season, decided by its high school.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AreaSummary {
    /// The Area's list id for the season.
    pub id: i64,
    pub name: String,
    /// The State's Area level (`Section`).
    pub level: String,
    pub state: String,
    pub season: i64,
    /// The Area's Athletic.net rankings list page, on the Profile's `--event`
    /// when it is a flat race, otherwise the 100m.
    pub url: String,
}

/// How a Profile's Area was looked up, and why it is `null` when it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AreaLookup {
    pub status: AreaStatus,
    pub season: i64,
    /// The high school that decides the Area (the Profile's school for the
    /// season, the same one its School shows).
    pub school_id: Option<i64>,
    /// The school's State, from Athletic.net's team record.
    pub state: Option<String>,
    /// What to know about the answer: why there is no Area, or that the
    /// school is not listed on the Profile for this season.
    pub note: Option<String>,
    /// The note says the Profile lists no school for the season (the school
    /// is a fallback or a guess). Not part of the JSON: `ath profile` warns
    /// on it.
    #[serde(skip)]
    pub school_not_listed: bool,
}

/// Whether a Profile has an Area, and if not, why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AreaStatus {
    /// The school is on one of the season's Area lists.
    Ok,
    /// The Profile has no high school for the season (clubs never decide an
    /// Area).
    NoHighSchool,
    /// The school's State has no Area level configured.
    StateNotConfigured,
    /// The school is on no Athletic.net lists for the season (it did not
    /// compete, or the season does not exist yet).
    NotListed,
    /// The school is on none of the season's Area lists for its State.
    NotInAnArea,
    /// Athletic.net could not be asked (the note has the error); the rest of
    /// the Profile is still good.
    LookupFailed,
}

/// The school a Profile's Area is looked up for, from [`Bio::school_in`]:
/// the same school the Profile's School row shows for the season.
fn area_school(pick: Option<SchoolPick<'_>>) -> Option<SchoolPick<'_>> {
    const HIGH_SCHOOL: i64 = 4;
    pick.filter(|p| p.team.level == HIGH_SCHOOL)
}

/// Look up the Area of a Profile's school for `season`. Never fails: an error
/// talking to Athletic.net is a [`AreaStatus::LookupFailed`] lookup.
///
/// Requests: the school's chain, then (only for a configured State) the
/// season's tree, read through the chain's own State list (none once cached).
/// `link_event` is the event the Area link opens on.
pub async fn profile_area<S: DataSource + ?Sized>(
    source: &S,
    bio: &Bio,
    season: i64,
    link_event: &str,
) -> (Option<AreaSummary>, AreaLookup) {
    let pick = bio.school_in(season);
    let mut notes = Vec::new();
    // About the Profile's School for the season, whether or not it is a high
    // school that can decide an Area.
    if let Some(p) = pick.filter(|p| !p.in_season()) {
        let (name, year) = (
            p.team.name.trim(),
            p.team
                .year
                .map_or_else(|| "with no year".to_string(), |y| y.to_string()),
        );
        notes.push(match p.basis {
            SchoolBasis::Guess => format!(
                "no school on this Profile is listed for {season}, which is before every \
                 school's last listed season; guessed {name} (last listed {year})"
            ),
            _ => format!(
                "no school on this Profile is listed for {season}; used {name} (last listed \
                 {year})"
            ),
        });
    }
    let school_not_listed = !notes.is_empty();
    let pick = area_school(pick);
    let school_id = pick.map(|p| p.team.id);
    let gender = bio
        .athlete
        .gender
        .as_deref()
        .and_then(Gender::from_bio)
        .unwrap_or(Gender::F);
    let mut state = None;
    let (area, status) = match school_id {
        None => (None, AreaStatus::NoHighSchool),
        Some(school) => {
            match find_area(source, school, season, gender, link_event, &mut state).await {
                Ok(found) => found,
                Err(e) => {
                    notes.push(format!("{e:#}"));
                    (None, AreaStatus::LookupFailed)
                }
            }
        }
    };
    let why = match status {
        AreaStatus::Ok | AreaStatus::LookupFailed => None,
        AreaStatus::NoHighSchool => Some(format!(
            "no high school on this Profile for {season} (a club never decides the Area)"
        )),
        AreaStatus::StateNotConfigured => Some(format!(
            "{} has no Area level configured in ath",
            state.as_deref().unwrap_or("the school's State")
        )),
        AreaStatus::NotListed => Some(format!(
            "the school is on no Athletic.net lists for {season} (Athletic.net may not have the \
             season yet)"
        )),
        AreaStatus::NotInAnArea => Some(format!(
            "the school is on none of the {season} Area lists for {}",
            state.as_deref().unwrap_or("its State")
        )),
    };
    if let Some(why) = why {
        notes.insert(0, why);
    }
    let lookup = AreaLookup {
        status,
        season,
        school_id,
        state,
        note: (!notes.is_empty()).then(|| notes.join("; ")),
        school_not_listed,
    };
    (area, lookup)
}

/// The Area of `school` for `season` (or why there is none). Sets `state` as
/// soon as the school's State is known.
async fn find_area<S: DataSource + ?Sized>(
    source: &S,
    school: i64,
    season: i64,
    gender: Gender,
    link_event: &str,
    state: &mut Option<String>,
) -> Result<(Option<AreaSummary>, AreaStatus)> {
    let nav = source.team_divisions(school, season).await?;
    *state = nav
        .team
        .state
        .as_deref()
        .map(|s| s.trim().to_ascii_uppercase())
        .filter(|s| !s.is_empty());
    let Some((st, level)) = state.as_deref().and_then(|s| area_level(s).map(|l| (s, l))) else {
        return Ok((None, AreaStatus::StateNotConfigured));
    };
    if nav.divisions.is_empty() {
        return Ok((None, AreaStatus::NotListed));
    }
    let state_list = nav.state_list_id().with_context(|| {
        format!("school {school}'s {season} division chain has no State below High School")
    })?;
    let mut tree = source.season_area_tree(season, state_list).await?;
    match tree.state(st) {
        Some(r) if r.id == state_list => {}
        _ => bail!(
            "school {school}'s {season} State list {state_list} is not {st}'s list{}",
            tree.partial_note()
        ),
    }
    complete_area(source, &mut tree, st).await?;
    let areas = tree.areas(st, level);
    let found = nav
        .divisions
        .iter()
        .find_map(|d| areas.iter().find(|a| a.id == d.id));
    let Some(area) = found else {
        return Ok((None, AreaStatus::NotInAnArea));
    };
    let summary = AreaSummary {
        id: area.id,
        name: area.name.trim().to_string(),
        level: level.to_string(),
        state: st.to_string(),
        season,
        url: rankings_list_url(area.id, gender, link_event),
    };
    Ok((Some(summary), AreaStatus::Ok))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn area_files_are_valid() {
        let states = configured_states();
        assert_eq!(states.len(), files::AREA_FILES.len());
        let mut seen = HashSet::new();
        for ((file, _), s) in files::AREA_FILES.iter().zip(states) {
            assert_eq!(
                *file,
                format!("{}.json", s.state),
                "areas/{file} is named for its State"
            );
            assert!(
                US_STATES.contains(&s.state.as_str()),
                "areas/{file}: {}",
                s.state
            );
            assert!(seen.insert(&s.state), "areas/{file}: {} twice", s.state);
            assert!(!s.level.trim().is_empty(), "areas/{file}: no level");
            assert!(!s.areas.is_empty(), "areas/{file}: no areas");
            let mut keys = HashSet::new();
            for a in &s.areas {
                // An abbreviation that is the name itself (Maine's `A`) adds no key.
                let abbr = a.abbr.as_ref().filter(|b| !b.eq_ignore_ascii_case(&a.name));
                for key in std::iter::once(&a.name).chain(abbr) {
                    assert!(
                        keys.insert(key.to_ascii_lowercase()),
                        "areas/{file}: {key} names two Areas"
                    );
                }
            }
        }
    }

    #[test]
    fn california_areas_are_sections() {
        assert_eq!(area_level("CA"), Some("Section"));
        assert_eq!(area_level(" ca "), Some("Section"));
        assert_eq!(area_level("ME"), Some("Class"));
        assert_eq!(area_level("NY"), None);
        assert_eq!(area_level("TX"), None);
    }

    #[test]
    fn season_list_id_reads_numbers_and_strings() {
        let nav: NavInfo = serde_json::from_str(
            r#"{"seasons": {"2025": 900499, "2026": "900500"}, "regions": []}"#,
        )
        .unwrap();
        assert_eq!(nav.season_list_id(2025), Some(900499));
        assert_eq!(nav.season_list_id(2026), Some(900500));
        assert_eq!(nav.season_list_id(2027), None);
    }

    #[test]
    fn custom_divisions_are_never_areas() {
        let nav: NavInfo = serde_json::from_str(
            r#"{"regions": [
                {"id": 1, "name": "Real", "state": "CA", "divType": "Section", "depth": 2, "customDivision": false},
                {"id": 2, "name": "Custom", "state": "CA", "divType": "Section", "depth": 2, "customDivision": true}
            ]}"#,
        )
        .unwrap();
        let tree = AreaTree::from_nav(2026, nav);
        let ids: Vec<i64> = tree.areas("CA", "Section").iter().map(|r| r.id).collect();
        assert_eq!(ids, [1]);
    }

    fn ca_tree() -> AreaTree {
        let nav: NavInfo = serde_json::from_str(
            r#"{"regions": [
                {"id": 10, "name": "California", "state": "CA", "divType": "State", "depth": 1},
                {"id": 11, "name": " Central Coast", "state": "CA", "divType": "Section", "depth": 2},
                {"id": 12, "name": " Central", "state": "CA", "divType": "Section", "depth": 2},
                {"id": 20, "name": "Texas", "state": "TX", "divType": "State", "depth": 1},
                {"id": 21, "name": "Central", "state": "TX", "divType": "League", "depth": 2}
            ]}"#,
        )
        .unwrap();
        AreaTree::from_nav(2026, nav)
    }

    #[test]
    fn areas_are_found_by_name_or_abbreviation() {
        let tree = ca_tree();
        for (state, name, id) in [
            (None, "Central Coast", 11),
            (None, "central coast ", 11),
            (Some("ca"), "CCS", 11),
            (None, "ccs", 11),
            // The exact name, not a prefix of a longer one.
            (None, "Central", 12),
            (None, "CS", 12),
        ] {
            let found = area_named(&tree, state, name).unwrap();
            assert_eq!(
                (found.region.id, found.state, found.level),
                (id, "CA", "Section"),
                "{name}"
            );
        }
    }

    #[test]
    fn an_unknown_area_or_unconfigured_state_says_what_there_is() {
        let tree = ca_tree();
        let err = area_named(&tree, None, "Coast").unwrap_err().to_string();
        assert!(
            err.contains("the 2026 Areas are: Central (CS), Central Coast (CCS)"),
            "{err}"
        );
        // Texas has a "Central" league, but no Area level configured.
        let err = area_named(&tree, Some("TX"), "Central")
            .unwrap_err()
            .to_string();
        assert!(err.contains("No Area level is configured for TX"), "{err}");
    }

    /// The national tree as Athletic.net has it, with California's Sections
    /// (Central Coast, 11) and Maine (30) without its Classes; and Maine's own
    /// subtree.
    fn me_trees(class_type: &str) -> (NavInfo, NavInfo) {
        let mut regions = vec![
            serde_json::json!({"id": 1, "name": "High School", "divType": "Level", "depth": 0}),
            serde_json::json!({"id": 11, "name": " Central Coast", "state": "CA", "divType": "Section", "depth": 2}),
        ];
        regions.extend(US_STATES.iter().zip(0..).map(|(st, i)| {
            let id = match *st {
                "CA" => 10,
                "ME" => 30,
                _ => 100 + i,
            };
            serde_json::json!({"id": id, "name": st, "state": st, "divType": "State", "depth": 1})
        }));
        let national: NavInfo =
            serde_json::from_value(serde_json::json!({ "regions": regions })).unwrap();
        assert!(national.is_national());
        let sub = serde_json::json!({"regions": [], "tree": [
            {"id": 30, "name": "Maine", "state": "ME", "divType": "State", "depth": 0},
            {"id": 31, "name": "A", "state": "ME", "divType": class_type, "depth": 1},
            {"id": 32, "name": "B", "state": "ME", "divType": class_type, "depth": 1},
            {"id": 33, "name": "C", "state": "ME", "divType": "Class", "depth": 1},
            {"id": 34, "name": "Custom", "state": "ME", "divType": "Class", "depth": 1, "customDivision": true},
            {"id": 35, "name": "A North", "state": "ME", "divType": "Division", "depth": 2}
        ]});
        (national, serde_json::from_value(sub).unwrap())
    }

    #[tokio::test]
    async fn areas_missing_from_the_national_tree_come_from_the_states_own() {
        let (national, sub) = me_trees("Class");
        let src = crate::source::FixtureSource::new().with_state_tree(2026, 30, sub);
        let mut tree = AreaTree::from_nav(2026, national);
        assert!(tree.areas("ME", "Class").is_empty());
        // California has its Sections already, and Texas no file or list:
        // neither is asked for (the source has no tree for them).
        complete_areas(&src, &mut tree, &["CA", "TX", "ME"])
            .await
            .unwrap();
        let ids: Vec<i64> = tree.areas("ME", "Class").iter().map(|r| r.id).collect();
        assert_eq!(ids, [31, 32, 33]);
        assert_eq!(area_named(&tree, None, "b").unwrap().region.id, 32);
        // Again: nothing missing, nothing asked.
        let empty = crate::source::FixtureSource::new();
        complete_areas(&empty, &mut tree, &["ME"]).await.unwrap();
        assert_eq!(tree.areas("ME", "Class").len(), 3);
    }

    #[tokio::test]
    async fn a_partial_tree_is_not_completed() {
        let (_, sub) = me_trees("Class");
        let partial: NavInfo = serde_json::from_str(
            r#"{"regions": [{"id": 30, "name": "Maine", "state": "ME", "divType": "State", "depth": 1}]}"#,
        )
        .unwrap();
        let src = crate::source::FixtureSource::new().with_state_tree(2026, 30, sub);
        let mut tree = AreaTree::from_nav(2026, partial);
        complete_areas(&src, &mut tree, &["ME"]).await.unwrap();
        assert!(
            tree.areas("ME", "Class").is_empty(),
            "it may just not show them"
        );
    }

    #[tokio::test]
    async fn a_name_the_tree_knows_never_waits_on_another_states_subtree() {
        let (national, _) = me_trees("Class");
        // No subtree for Maine: asking for it would fail.
        let src = crate::source::FixtureSource::new();
        let mut tree = AreaTree::from_nav(2026, national);
        let ccs = find_area_named(&src, &mut tree, None, "CCS").await.unwrap();
        assert_eq!(ccs.region.id, 11);
        let err = find_area_named(&src, &mut tree, None, "B")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no fixture navinfo/2026-30.json"), "{err}");
    }

    #[tokio::test]
    async fn areas_of_another_level_are_an_error() {
        let (national, sub) = me_trees("Region");
        let src = crate::source::FixtureSource::new().with_state_tree(2026, 30, sub);
        let mut tree = AreaTree::from_nav(2026, national);
        let err = complete_areas(&src, &mut tree, &["ME"])
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(
                "below ME are not of the level areas/ME.json names (Class): A (Region), B (Region)"
            ),
            "{err}"
        );
    }

    #[tokio::test]
    async fn a_subtree_of_another_list_is_an_error() {
        let (national, mut sub) = me_trees("Class");
        sub.tree[0].id = 99;
        let src = crate::source::FixtureSource::new().with_state_tree(2026, 30, sub);
        let mut tree = AreaTree::from_nav(2026, national);
        let err = complete_areas(&src, &mut tree, &["ME"])
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("ME's list 30 does not start at it"), "{err}");
    }

    #[tokio::test]
    async fn ath_areas_lists_maines_classes() {
        let (national, sub) = me_trees("Class");
        let src = crate::source::FixtureSource::new()
            .with_tree(2026, national)
            .with_state_tree(2026, 30, sub);
        let q = AreasQuery {
            state: "me",
            season: 2026,
            gender: Gender::F,
            event: "3200m".into(),
            from_list: 1,
        };
        let a = areas(&src, &q).await.unwrap();
        assert_eq!(a.level.as_deref(), Some("Class"));
        let names: Vec<&str> = a.areas.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["A", "B", "C"]);
        assert_eq!(
            a.areas[0].url,
            "https://www.athletic.net/TrackAndField/rankings/list/31/f/3200m"
        );
    }

    #[test]
    fn list_urls_open_the_rankings_list_on_an_event() {
        assert_eq!(
            rankings_list_url(900502, Gender::F, "400m"),
            "https://www.athletic.net/TrackAndField/rankings/list/900502/f/400m"
        );
    }

    #[test]
    fn flat_races_have_a_short_form() {
        for (given, short) in [
            ("400m", Some("400m")),
            ("400 Meters", Some("400m")),
            (" 1600 meters ", Some("1600m")),
            ("100M", Some("100m")),
            ("Long Jump", None),
            ("m", None),
            ("100mH", None),
        ] {
            assert_eq!(flat_race_short(given).as_deref(), short, "{given}");
        }
    }

    #[test]
    fn link_events_are_flat_races_or_codes() {
        assert_eq!(link_event("400 Meters").unwrap(), "400m");
        assert_eq!(link_event("Shot").unwrap(), "Shot");
        assert!(link_event("Long Jump").is_err());
        assert!(link_event("").is_err());
        let err = link_event(" 400 ").unwrap_err().to_string();
        assert!(err.contains("did you mean 400m?"), "{err}");
    }

    #[test]
    fn the_state_link_is_right_below_high_school() {
        let nav: TeamNav = serde_json::from_str(
            r#"{"team": {"ID": 5103}, "divisions": [
                {"id": 900010, "b": 901, "name": "   United States"},
                {"id": 900020, "b": 902, "name": "High School"},
                {"id": 900500, "b": 903, "name": "California"},
                {"id": 900502, "b": 904, "name": "Central Coast"}
            ]}"#,
        )
        .unwrap();
        assert_eq!(nav.state_list_id(), Some(900500));
    }
}
