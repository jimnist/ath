//! `ath list`: a whole Rankings list (a State's or an Area's) for one event,
//! season, gender and Grade group.
//!
//! Athletic.net serves a list a page at a time (`tfRankings/GetRankings` with
//! `qParams.page`), read through the [`DataSource`] seam one page per call.
//! Page 1 holds rows 1 to 101 and page `p` rows `100(p-1)` to `100p+1`, so
//! neighboring pages share two rows; rows are deduped by `AthleteID`. Paging
//! stops at a short page. `minCount` is no stop signal: it is only a lower
//! bound on the list's length that grows as the pages go deeper (it reaches
//! the length near the end, where it is checked against the rows read).
//!
//! Every page is checked, because Athletic.net answers some mistakes with a
//! different list instead of an error: an event code it does not know gets
//! the 100m list, a list id from another season gets that season's list, and
//! a list id it does not have gets no division at all. Relay lists (rows are
//! teams) are refused: relays are out of v1 scope.
//!
//! Past what the session may see (5 rows signed out, 25 signed in without a
//! paid subscription), Athletic.net puts Masked rows on the page instead of
//! people (taken out when the page is parsed), or may just stop at the cap
//! short of the length it states. Such a page is capped, not the end of the
//! list ([`RankingsPage::is_capped_after`]): its first rows still answer what
//! they can (`--limit 10`, an Athlete placed in the top 20), and anything
//! that needs rows past the cap is an error ([`Capped`]), never a short list
//! (ADR 0001).
//!
//! Pacing (a delay between the pages actually fetched, slowed when
//! Athletic.net pushes back) belongs to the live data source, which knows
//! which pages come from the cache.

use std::collections::{HashMap, HashSet};
use std::fmt;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::area::{
    AreaTree, Gender, NamedArea, RankingsDivision, find_area_named, rankings_list_url, season_tree,
    state_code, state_list,
};
use crate::cache::Cacheable;
use crate::contract::{Report, columns};
use crate::model::athlete_url;
use crate::source::DataSource;

/// Rows on a full page past its overlap: a page with fewer is the last.
pub const PAGE_ROWS: usize = 100;

/// Pages read at most for one list (100,000 rows; California's girls' 400m
/// list, every grade, was 11,389 rows in 2026), so a list that never ends
/// cannot page forever.
pub const MAX_PAGES: u32 = 1000;

/// The grades high school lists can be filtered to.
const HIGH_SCHOOL_GRADES: std::ops::RangeInclusive<i64> = 9..=12;

/// A Grade group: the grades a list is filtered to, in order. Empty is every
/// grade (no filter).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Grades(Vec<i64>);

impl Grades {
    /// Every grade: no filter.
    pub fn all() -> Self {
        Self::default()
    }

    /// `9-10`, `9,10,11`, `11`, or `all`. Grades are 9 to 12.
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if s.eq_ignore_ascii_case("all") {
            return Ok(Self::all());
        }
        let grade = |g: &str| -> Result<i64, String> {
            let n: i64 = g
                .trim()
                .parse()
                .map_err(|_| format!("{g:?} is not a grade"))?;
            if HIGH_SCHOOL_GRADES.contains(&n) {
                Ok(n)
            } else {
                Err(format!("grade {n} is not a high school grade (9 to 12)"))
            }
        };
        let mut grades = Vec::new();
        for part in s.split(',') {
            match part.split_once('-') {
                Some((lo, hi)) => {
                    let (lo, hi) = (grade(lo)?, grade(hi)?);
                    if lo > hi {
                        return Err(format!("{part:?} runs backwards; did you mean {hi}-{lo}?"));
                    }
                    grades.extend(lo..=hi);
                }
                None => grades.push(grade(part)?),
            }
        }
        grades.sort_unstable();
        grades.dedup();
        Ok(Self(grades))
    }

    /// One grade.
    pub fn only(grade: i64) -> Self {
        Self(vec![grade])
    }

    /// 9th grade through `top` (a Grade group in `CONTEXT.md`).
    pub fn high_school(top: i64) -> Self {
        Self((9..=top).collect())
    }

    /// The grades as given (sorted, once each).
    pub fn from_slice(grades: &[i64]) -> Self {
        let mut grades = grades.to_vec();
        grades.sort_unstable();
        grades.dedup();
        Self(grades)
    }

    pub fn as_slice(&self) -> &[i64] {
        &self.0
    }

    pub fn is_all(&self) -> bool {
        self.0.is_empty()
    }

    /// The grades as Athletic.net's web page and fixture names write them:
    /// `9,10`; empty for every grade.
    pub fn joined(&self, sep: &str) -> String {
        self.0
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(sep)
    }
}

impl fmt::Display for Grades {
    /// `grades 9-10`, `grades 9, 11`, `grade 11` or `all grades`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.as_slice() {
            [] => write!(f, "all grades"),
            [g] => write!(f, "grade {g}"),
            [first, .., last] if last - first + 1 == self.0.len() as i64 => {
                write!(f, "grades {first}-{last}")
            }
            _ => write!(f, "grades {}", self.joined(", ")),
        }
    }
}

/// The Athletic.net rankings list page for a list on one event, filtered to
/// a Grade group the way the page's own filter does it (`?grades=9,10`).
pub fn list_url(list_id: i64, gender: Gender, event: &str, grades: &Grades) -> String {
    let url = rankings_list_url(list_id, gender, event);
    if grades.is_all() {
        url
    } else {
        format!("{url}?grades={}", grades.joined(","))
    }
}

// ---------------------------------------------------------------------------
// The data source's page

/// One page of a rankings list, as [`DataSource::rankings_page`] asks for it.
#[derive(Debug, Clone, Copy)]
pub struct PageQuery<'a> {
    pub list_id: i64,
    /// The season the list is expected in. Only decides how long the page is
    /// cached; [`rankings_list`] checks the answer's own season.
    pub season: i64,
    pub gender: Gender,
    /// Athletic.net's `eventShort` (`400m`, `Shot`).
    pub event: &'a str,
    pub grades: &'a Grades,
    /// From 1.
    pub page: u32,
}

/// `POST tfRankings/GetRankings` for a list page (only the fields we use).
/// The response also carries `defaultSettings, includeDivisions,
/// sportRankings, eventId, gender, eventType, eventTypes, jwtTFTopReport,
/// reportTitle, level, ...`.
///
/// Masked rows ([`RankingsRow::is_masked`]) are taken out of
/// `grouped_rankings` when the page is parsed, and what they say about a cap
/// is worked out then, once ([`RankingsPage::masked`] and the fields after
/// it), so nothing downstream can take one for a person.
#[derive(Debug, Clone, Deserialize)]
#[serde(from = "RawRankingsPage")]
pub struct RankingsPage {
    /// The event Athletic.net answered with: the 100m for a code it does not
    /// know.
    pub event_short: Option<String>,
    /// The list's division; its `SeasonID` is the list's season. Null for a
    /// list id Athletic.net does not have.
    pub division: Option<RankingsDivision>,
    /// The members of each relay team on a relay list; empty otherwise.
    pub relay_teams: serde_json::Value,
    /// The page's people, in one or more groups, without its Masked rows.
    pub grouped_rankings: Vec<Vec<RankingsRow>>,
    /// A lower bound on the list's length, which grows as the pages go
    /// deeper; on the last pages it is the length.
    pub min_count: Option<i64>,
    /// The filters Athletic.net applied (`fat`, `official`, `grades`, ...).
    pub settings: Option<ListSettings>,
    /// Masked rows Athletic.net sent on the page (taken out above).
    pub masked: usize,
    /// Whether a person's row came after a Masked row: Masked rows are only
    /// ever seen as a tail, so this is a shape `ath` does not know.
    pub masked_mid: bool,
    /// People on the page (`grouped_rankings`, all groups).
    pub shown: usize,
    /// The last person's `rowNum` on the page; `None` when a row has none.
    pub last_row: Option<i64>,
}

/// [`RankingsPage`] as Athletic.net sends it, Masked rows and all.
#[derive(Deserialize)]
struct RawRankingsPage {
    #[serde(rename = "eventShort", default)]
    event_short: Option<String>,
    #[serde(default)]
    division: Option<RankingsDivision>,
    #[serde(rename = "relayTeams", default)]
    relay_teams: serde_json::Value,
    #[serde(rename = "groupedRankings", default)]
    grouped_rankings: Vec<Vec<RankingsRow>>,
    #[serde(rename = "minCount", default)]
    min_count: Option<i64>,
    #[serde(default)]
    settings: Option<ListSettings>,
}

impl From<RawRankingsPage> for RankingsPage {
    fn from(raw: RawRankingsPage) -> Self {
        let (mut masked, mut masked_mid, mut shown) = (0, false, 0);
        let mut row_nums = Some(Vec::new());
        let grouped_rankings = raw
            .grouped_rankings
            .into_iter()
            .map(|group| {
                group
                    .into_iter()
                    .filter(|r| {
                        if r.is_masked() {
                            masked += 1;
                            return false;
                        }
                        masked_mid |= masked > 0;
                        shown += 1;
                        match (r.row_num, row_nums.as_mut()) {
                            (Some(n), Some(nums)) => nums.push(n),
                            _ => row_nums = None,
                        }
                        true
                    })
                    .collect()
            })
            .collect();
        RankingsPage {
            event_short: raw.event_short,
            division: raw.division,
            relay_teams: raw.relay_teams,
            grouped_rankings,
            min_count: raw.min_count,
            settings: raw.settings,
            masked,
            masked_mid,
            shown,
            last_row: row_nums.and_then(|nums| nums.into_iter().max()),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListSettings {
    #[serde(default)]
    pub fat: serde_json::Value,
    #[serde(default)]
    pub official: serde_json::Value,
    /// The Grade group applied; absent for every grade.
    #[serde(default)]
    pub grades: serde_json::Value,
}

/// A JSON flag Athletic.net may send as a bool or a 0/1 number.
fn flag(v: &serde_json::Value) -> Option<bool> {
    match v {
        serde_json::Value::Bool(b) => Some(*b),
        serde_json::Value::Number(n) => n.as_i64().map(|n| n != 0),
        _ => None,
    }
}

/// Rows of a list a signed-out session sees (ADR 0001, checked 2026-11-06).
pub const SIGNED_OUT_ROWS: i64 = 5;

/// Rows of a list a session signed in without a paid subscription sees
/// (ADR 0001, checked 2026-11-06).
pub const UNPAID_ROWS: i64 = 25;

impl RankingsPage {
    /// Whether the page shows the session only part of the list (ADR 0001),
    /// given the people read from the list's earlier pages:
    ///
    /// - it had a Masked row;
    /// - it is empty while the length it states is more than was read (a
    ///   signed-out page 1, or a page past a cap); or
    /// - its people end exactly at a known cap ([`SIGNED_OUT_ROWS`],
    ///   [`UNPAID_ROWS`]) on a short page, below the length it states.
    ///
    /// Any other short page below its stated length is not called capped: a
    /// paid list's count can lag its rows by a row or two, which
    /// [`read_pages`] only notes.
    pub fn is_capped_after(&self, read_before: usize) -> bool {
        if self.masked > 0 {
            return true;
        }
        let stated = self.min_count.unwrap_or(0);
        if self.shown == 0 {
            return stated > read_before as i64;
        }
        self.shown < PAGE_ROWS
            && self
                .last_row
                .is_some_and(|last| [SIGNED_OUT_ROWS, UNPAID_ROWS].contains(&last) && stated > last)
    }

    /// Athletic.net's name for the list (`California`), when the page says.
    pub fn name(&self) -> Option<String> {
        self.division
            .as_ref()?
            .name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_string)
    }
}

/// A page that may be capped is never cached, nor served from the cache, so
/// it cannot stand in for the whole page in a later run that could read it.
/// Judged as if nothing came before it, so an empty page that states a
/// length is not kept either (it only costs that page a refetch).
impl Cacheable for RankingsPage {
    fn cacheable(&self) -> bool {
        !self.is_capped_after(0)
    }
}

/// One row of a rankings list (only the fields we use). Rows have no plain
/// `SortInt`: `SortIntCalc` is the comparable value, in the same encoding.
#[derive(Debug, Clone, Deserialize)]
pub struct RankingsRow {
    /// 0 (or null) on a Masked row. Required: a list whose rows lost the key is a changed API, not a
    /// paywall.
    #[serde(rename = "AthleteID", deserialize_with = "null_as_zero")]
    pub athlete_id: i64,
    #[serde(rename = "AthleteName", default)]
    pub athlete_name: String,
    /// The mark as Athletic.net shows it (`55.48`).
    #[serde(default)]
    pub display: String,
    #[serde(rename = "SortIntCalc")]
    pub sort_int: i64,
    /// Athletic.net's place on the list; tied marks share it.
    #[serde(default)]
    pub rank: Option<i64>,
    /// The row's position on the list, from 1 (what pages overlap on).
    #[serde(rename = "rowNum", default)]
    pub row_num: Option<i64>,
    #[serde(rename = "GradeID", default)]
    pub grade: Option<i64>,
    #[serde(rename = "TeamID", default)]
    pub team_id: Option<i64>,
    #[serde(rename = "TeamName", default)]
    pub team_name: Option<String>,
    /// `2026-05-16T00:00:00`.
    #[serde(rename = "ResultDate", default)]
    pub result_date: Option<String>,
    #[serde(rename = "MeetID", default)]
    pub meet_id: Option<i64>,
    #[serde(rename = "MeetName", default)]
    pub meet_name: Option<String>,
    /// The event's full name (`400 Meters`).
    #[serde(rename = "Event", default)]
    pub event: Option<String>,
}

impl RankingsRow {
    /// A Masked row: Athletic.net's placeholder past what the session may see
    /// (`CONTEXT.md`). Seen live (2026-11-06, signed in without a paid
    /// subscription) as `AthleteID` 0, `AthleteName` `Xxxxxx Xxxxxx`, an empty
    /// `display`, `TeamID` and `MeetID` 0 and `GradeID` 99, but a real-looking
    /// `SortIntCalc`. No Profile has id 0, so that alone is enough; a row with
    /// a name of only `X`s and no mark is one too, in case the id ever comes
    /// back filled in.
    pub fn is_masked(&self) -> bool {
        let name = self.athlete_name.trim();
        let x_name = !name.is_empty()
            && name
                .chars()
                .all(|c| c.eq_ignore_ascii_case(&'x') || c.is_whitespace());
        self.athlete_id <= 0 || (x_name && self.display.trim().is_empty())
    }
}

/// A JSON id that may be null: null is 0 (no such id).
fn null_as_zero<'de, D: serde::Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    Ok(Option::<i64>::deserialize(d)?.unwrap_or(0))
}

// ---------------------------------------------------------------------------
// `ath list`

/// Which list `ath list` reads.
#[derive(Debug, Clone, Copy)]
pub enum Which<'a> {
    /// A State's list, by two-letter code.
    State(&'a str),
    /// An Area's list, by name or abbreviation, within `state` when given.
    Area {
        name: &'a str,
        state: Option<&'a str>,
    },
    /// Any list, by id.
    List(i64),
}

/// What `ath list` is asked for.
#[derive(Debug, Clone)]
pub struct ListQuery<'a> {
    pub which: Which<'a>,
    pub season: i64,
    pub gender: Gender,
    /// Athletic.net's `eventShort`.
    pub event: String,
    pub grades: Grades,
    /// Only the first this many rows, without paging past them.
    pub limit: Option<usize>,
    /// The rankings list (any season) to find the season's lists from, for
    /// `--state` and `--area`.
    pub from_list: i64,
}

/// What kind of list was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ListKind {
    State,
    Area,
    /// Given by id: `ath` does not say what it is.
    List,
}

/// The list a [`RankingsList`] is.
#[derive(Debug, Clone, Serialize)]
pub struct ListInfo {
    /// The list (division) id for the season.
    pub id: i64,
    /// Athletic.net's name for the list.
    pub name: String,
    pub kind: ListKind,
    /// The State (two-letter code) for a State or Area list.
    pub state: Option<String>,
    /// The Area level (`Section`) for an Area list.
    pub level: Option<String>,
}

/// One person on the list.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ListRow {
    /// Athletic.net's row number: equal marks get consecutive numbers (checked
    /// 2026-11-02), so it is not a place.
    pub rank: Option<i64>,
    /// The place as `CONTEXT.md` counts it: 1 plus the number of people
    /// strictly faster on the rows read, which are everyone faster.
    pub place: usize,
    /// Whether someone else on the rows read has exactly this mark (`T3`).
    /// Unknown past the last row read, so a tie there reads as no tie.
    pub tied: bool,
    pub athlete_id: i64,
    pub name: String,
    /// The mark as Athletic.net shows it.
    pub mark: String,
    /// The numeric sort key (`SortIntCalc`): lower is better.
    pub sort_int: i64,
    pub grade: Option<i64>,
    pub team: Option<String>,
    pub team_id: Option<i64>,
    /// The day the mark was set (`YYYY-MM-DD`).
    pub date: Option<String>,
    pub meet: Option<String>,
    pub meet_id: Option<i64>,
    /// The person's Athletic.net Profile.
    pub url: String,
}

/// Each mark's place and whether it is tied, for `SortIntCalc` marks in order:
/// 1 plus the number of marks strictly faster, and whether another mark is
/// the same.
pub fn places(marks: &[i64]) -> Vec<(usize, bool)> {
    let mut out = Vec::with_capacity(marks.len());
    let mut start = 0;
    while start < marks.len() {
        let end = start
            + marks[start..]
                .iter()
                .take_while(|m| **m == marks[start])
                .count();
        out.extend(std::iter::repeat_n(
            (start + 1, end - start > 1),
            end - start,
        ));
        start = end;
    }
    out
}

impl ListRow {
    /// A row with its place and tie from [`places`].
    pub fn placed(r: RankingsRow, (place, tied): (usize, bool)) -> Self {
        ListRow {
            rank: r.rank,
            place,
            tied,
            url: athlete_url(r.athlete_id),
            athlete_id: r.athlete_id,
            name: r.athlete_name.trim().to_string(),
            mark: r.display.trim().to_string(),
            sort_int: r.sort_int,
            grade: r.grade,
            team: r.team_name.map(|t| t.trim().to_string()),
            team_id: r.team_id,
            date: r.result_date.and_then(|d| d.get(..10).map(str::to_string)),
            meet: r.meet_name.map(|m| m.trim().to_string()),
            meet_id: r.meet_id,
        }
    }
}

/// A Rankings list, as `ath list` prints it.
#[derive(Debug, Clone, Serialize)]
pub struct RankingsList {
    pub list: ListInfo,
    pub season: i64,
    pub gender: Gender,
    /// Athletic.net's `eventShort` (`400m`).
    pub event: String,
    /// The event's full name (`400 Meters`), when the list has a row.
    pub event_name: Option<String>,
    /// The Grade group, in order; `null` for every grade.
    pub grades: Option<Vec<i64>>,
    /// Whether only fully automatic timed marks count (Athletic.net's
    /// default, which `ath` keeps).
    pub fat: Option<bool>,
    /// Whether only official marks count (the default, kept).
    pub official: Option<bool>,
    /// The list's length, when every page was read (`null` when `--limit`
    /// stopped early).
    pub total: Option<usize>,
    /// Rows printed.
    pub count: usize,
    /// The `--limit` asked for.
    pub limit: Option<usize>,
    /// Whether `rows` is the whole list (false when `--limit` cut it short).
    pub complete: bool,
    /// Something to know about the answer, several joined with `; `: two
    /// pages disagree where they overlap (they show the list at different
    /// moments), or the rows read disagree with the length Athletic.net gave
    /// on the last page.
    pub note: Option<String>,
    /// The Athletic.net rankings list page for the same view.
    pub url: String,
    /// By `sort_int`, best first; Athletic.net's order among ties.
    pub rows: Vec<ListRow>,
}

impl ListInfo {
    /// A State's own list in the season's tree (`state` a two-letter code).
    pub fn state(tree: &AreaTree, state: &str) -> Result<ListInfo> {
        let region = state_list(tree, state)?;
        Ok(ListInfo {
            id: region.id,
            name: region.name.trim().to_string(),
            kind: ListKind::State,
            state: Some(state.to_string()),
            level: None,
        })
    }

    /// An Area's list, as [`area_named`] found it.
    pub fn area(found: &NamedArea<'_>) -> ListInfo {
        ListInfo {
            id: found.region.id,
            name: found.region.name.trim().to_string(),
            kind: ListKind::Area,
            state: Some(found.state.to_string()),
            level: Some(found.level.to_string()),
        }
    }

    /// A list given by id: `ath` does not say what it is.
    pub fn by_id(id: i64) -> ListInfo {
        ListInfo {
            id,
            name: format!("List {id}"),
            kind: ListKind::List,
            state: None,
            level: None,
        }
    }
}

/// The list `which` names, for `season`.
async fn resolve_list<S: DataSource + ?Sized>(source: &S, q: &ListQuery<'_>) -> Result<ListInfo> {
    let tree = || season_tree(source, q.season, q.from_list);
    Ok(match q.which {
        Which::List(id) => ListInfo::by_id(id),
        Which::State(state) => ListInfo::state(&tree().await?, &state_code(state)?)?,
        Which::Area { name, state } => {
            let state = state.map(state_code).transpose()?;
            let mut tree = tree().await?;
            ListInfo::area(&find_area_named(source, &mut tree, state.as_deref(), name).await?)
        }
    })
}

/// A list longer than [`MAX_PAGES`] pages: [`read_pages`] gives up on it.
#[derive(Debug)]
pub struct TooLong {
    pub list_id: i64,
    /// Rows read before giving up.
    pub rows: usize,
}

impl fmt::Display for TooLong {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "rankings list {} is longer than {MAX_PAGES} pages ({} rows so far)",
            self.list_id, self.rows
        )
    }
}

impl std::error::Error for TooLong {}

/// A list Athletic.net capped ([`RankingsPage::is_capped_after`]): the
/// session sees only its first rows, because full lists need a paid
/// Athletic.net subscription (ADR 0001). [`read_pages`] fails with it when
/// the answer needs rows past the cap, so nothing is ranked against part of
/// a list.
#[derive(Debug)]
pub struct Capped {
    /// The list, as the message names it (`California 2026 girls 400m (all
    /// grades)`).
    pub list: String,
    /// The rows shown before the cap, as Athletic.net numbers them (the last
    /// `rowNum` read; the people read when rows have no `rowNum`).
    pub shown: usize,
    /// Athletic.net's `minCount` on the capped page: a lower bound on the
    /// list's length.
    pub at_least: Option<i64>,
}

impl fmt::Display for Capped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let of = match self.at_least {
            Some(n) if n > self.shown as i64 => format!(" of at least {n}"),
            _ => String::new(),
        };
        write!(
            f,
            "Athletic.net returned only {}{of} rows for {}; full lists need a paid \
             Athletic.net subscription, signed in where ath reads Athletic.net (the relay tab's \
             browser, or the browser the cookie came from)",
            self.shown, self.list
        )
    }
}

impl std::error::Error for Capped {}

/// Which list [`read_pages`] reads: one list, event, gender and Grade group,
/// expected in `season`.
#[derive(Debug, Clone, Copy)]
pub struct ListPages<'a> {
    pub list_id: i64,
    pub season: i64,
    pub gender: Gender,
    /// Athletic.net's `eventShort`.
    pub event: &'a str,
    pub grades: &'a Grades,
}

impl ListPages<'_> {
    /// How messages name the list: `California 2026 girls 400m (all grades)`,
    /// with `name` Athletic.net's name for it when known.
    fn describe(&self, name: Option<String>) -> String {
        let name = name.unwrap_or_else(|| format!("rankings list {}", self.list_id));
        format!(
            "{name} {} {} {} ({})",
            self.season,
            self.gender.noun(),
            self.event,
            self.grades
        )
    }
}

/// What [`read_pages`] read of a list.
#[derive(Debug, Clone)]
pub struct Pages {
    /// Everyone read, once each, by `SortIntCalc` (Athletic.net's order among
    /// ties).
    pub rows: Vec<RankingsRow>,
    /// Whether the list's last page was read (rather than stopping early).
    pub ended: bool,
    /// Whether reading stopped at a cap ([`Capped`]), with `enough` answered
    /// from the rows shown.
    pub capped: bool,
    /// Pages read.
    pub pages: u32,
    /// Athletic.net's name for the list.
    pub name: Option<String>,
    /// The filters Athletic.net applied, from the first page.
    pub settings: ListSettings,
    /// Something to know about what was read (see [`RankingsList::note`]).
    pub notes: Vec<String>,
}

/// Read a list's pages in turn, each one checked, until the list ends or
/// `enough(rows, at_cap)` says the rows read so far (deduped by Profile, in
/// page order, which is `SortIntCalc` order) are enough. `at_cap` is true
/// when the page just read is capped ([`RankingsPage::is_capped_after`]): the
/// rows are all this session may see, so `enough` answers from them what it
/// can; when it cannot, reading fails with [`Capped`].
pub async fn read_pages<S: DataSource + ?Sized>(
    source: &S,
    list: &ListPages<'_>,
    mut enough: impl FnMut(&[RankingsRow], bool) -> bool,
) -> Result<Pages> {
    let ListPages {
        list_id,
        season,
        gender,
        event,
        grades,
    } = *list;
    let mut rows: Vec<RankingsRow> = Vec::new();
    let mut seen = HashSet::new();
    let mut first: Option<RankingsPage> = None;
    let mut last_min_count = None;
    let mut last_row = None;
    let mut ended = false;
    let mut capped = false;
    let mut pages = 0;
    // The previous page's marks by position, and the first page whose overlap
    // with it disagrees (its pages are from different moments: cached hours
    // apart, or the list changed while it was read). Marks, not people, so
    // tied people trading places between requests is no disagreement.
    let mut previous: HashMap<i64, i64> = HashMap::new();
    let mut torn_at = None;
    for page in 1..=MAX_PAGES {
        let query = PageQuery {
            list_id,
            season,
            gender,
            event,
            grades,
            page,
        };
        let mut answer = source
            .rankings_page(&query)
            .await
            .with_context(|| format!("reading page {page} of rankings list {list_id}"))?;
        pages = page;
        check_page(&answer, list_id, season, event, grades, page)?;
        let name = || first.as_ref().unwrap_or(&answer).name();
        if answer.masked_mid {
            bail!(
                "unexpected masked row in the middle of {}: Athletic.net masks only the rows \
                 after what a session may see, so ath stops rather than rank around it",
                list.describe(name())
            );
        }
        // A capped page is the end of what this session may see, not of the
        // list (see below). Its Masked rows are already gone.
        let cap = answer
            .is_capped_after(rows.len())
            .then(|| (name(), answer.min_count));
        let page_rows: Vec<RankingsRow> = std::mem::take(&mut answer.grouped_rankings)
            .into_iter()
            .flatten()
            .collect();
        // An empty page past the end says nothing about the list.
        if !page_rows.is_empty() {
            last_min_count = answer.min_count;
        }
        last_row = answer.last_row.max(last_row);
        let torn = page_rows.iter().any(|r| {
            r.row_num
                .and_then(|n| previous.get(&n))
                .is_some_and(|&mark| mark != r.sort_int)
        });
        if torn && torn_at.is_none() {
            torn_at = Some(page);
        }
        previous = page_rows
            .iter()
            .filter_map(|r| Some((r.row_num?, r.sort_int)))
            .collect();
        if first.is_none() {
            first = Some(answer);
        }
        let on_page = page_rows.len();
        let before = rows.len();
        rows.extend(page_rows.into_iter().filter(|r| seen.insert(r.athlete_id)));
        // Past a cap, the rows shown are still the list's first rows: when
        // they already answer, that answer stands; when it needs rows past
        // the cap, nothing is ranked against part of the list.
        if let Some((name, at_least)) = cap {
            if enough(&rows, true) {
                capped = true;
                break;
            }
            return Err(Capped {
                list: list.describe(name),
                // As Athletic.net numbers them; people only without rowNums.
                shown: last_row.map_or(rows.len(), |n| n.max(0) as usize),
                at_least,
            }
            .into());
        }
        // A short page (past the end they are empty) or one with nobody new
        // is the last. `minCount` cannot say so: it is only a lower bound
        // that grows as the pages go deeper, reaching the list's length near
        // its end.
        if on_page < PAGE_ROWS || rows.len() == before {
            ended = true;
            break;
        }
        if enough(&rows, false) {
            break;
        }
        if page == MAX_PAGES {
            return Err(TooLong {
                list_id,
                rows: rows.len(),
            }
            .into());
        }
    }
    let first = first.context("no pages read")?;
    // Pages come sorted; a stable sort keeps Athletic.net's order among ties.
    rows.sort_by_key(|r| r.sort_int);
    let mut notes = Vec::new();
    if let Some(page) = torn_at {
        notes.push(format!(
            "pages {} and {page} disagree where they overlap, so they show the list at \
             different moments (cached at different times, or it changed while it was read); \
             re-run with --refresh",
            page - 1
        ));
    }
    if ended
        && let Some(stated) = last_min_count
        && rows.len() as i64 != stated
    {
        notes.push(format!(
            "Athletic.net says the list has {stated} rows but its pages gave {}; it may have \
             changed while it was read (re-run with --refresh)",
            rows.len()
        ));
    }
    Ok(Pages {
        rows,
        ended,
        capped,
        pages,
        name: first.name(),
        settings: first.settings.unwrap_or_default(),
        notes,
    })
}

/// Whether the rows read show the tie of row `limit` (from 1): some row
/// after it has another mark.
fn tie_known(rows: &[RankingsRow], limit: usize) -> bool {
    rows.len() > limit
        && rows[limit..]
            .iter()
            .any(|r| r.sort_int != rows[limit - 1].sort_int)
}

/// `ath list`: the whole Rankings list (or its first `limit` rows), every
/// page read in turn and checked, rows deduped by Profile and ordered by
/// `SortIntCalc`.
pub async fn rankings_list<S: DataSource + ?Sized>(
    source: &S,
    q: &ListQuery<'_>,
) -> Result<RankingsList> {
    let info = resolve_list(source, q).await?;
    let list_id = info.id;
    let (season, event) = (q.season, q.event.as_str());
    let list = ListPages {
        list_id,
        season,
        gender: q.gender,
        event,
        grades: &q.grades,
    };
    // With --limit, read until some row past the limit has another mark than
    // the last one kept, so the last row's tie is known: usually the page that
    // reaches the limit, one more when that page ends inside the tie. At a
    // cap, the first `limit` rows shown are enough, tie known or not.
    let read = read_pages(source, &list, |rows, at_cap| {
        q.limit
            .is_some_and(|limit| tie_known(rows, limit) || (at_cap && rows.len() >= limit))
    })
    .await
    .map_err(|e| match e.downcast_ref::<TooLong>() {
        Some(_) => e.context("to read only the first rows, use --limit"),
        None => e,
    })?;
    let mut rows = read.rows;
    let mut notes = read.notes;
    let total = read.ended.then_some(rows.len());
    // Places over every row read, not just the first --limit, which reach past
    // the last row kept (see above), so its tie shows.
    let mut placed = places(&rows.iter().map(|r| r.sort_int).collect::<Vec<_>>());
    if let Some(limit) = q.limit {
        if read.capped && !tie_known(&rows, limit) {
            notes.push(format!(
                "Athletic.net shows this session only the first rows of the list (full lists \
                 need a paid Athletic.net subscription), so whether row {limit} ties the row \
                 after it is unknown"
            ));
        }
        rows.truncate(limit);
        placed.truncate(limit);
    }
    let note = (!notes.is_empty()).then(|| notes.join("; "));
    let settings = read.settings;
    Ok(RankingsList {
        list: ListInfo {
            name: read.name.unwrap_or(info.name),
            ..info
        },
        season,
        gender: q.gender,
        event: event.to_string(),
        event_name: rows
            .first()
            .and_then(|r| r.event.as_deref())
            .map(|e| e.trim().to_string()),
        grades: (!q.grades.is_all()).then(|| q.grades.as_slice().to_vec()),
        fat: flag(&settings.fat),
        official: flag(&settings.official),
        total,
        count: rows.len(),
        limit: q.limit,
        complete: total == Some(rows.len()),
        note,
        url: list_url(list_id, q.gender, event, &q.grades),
        rows: rows
            .into_iter()
            .zip(placed)
            .map(|(r, p)| ListRow::placed(r, p))
            .collect(),
    })
}

/// Refuse a page that is not the list asked for: Athletic.net answers an
/// event code it does not know with the 100m list, a list id from another
/// season with that season's list, and a list id it does not have with no
/// division (on page 1: a page past the end of a real list may not say). A
/// relay list is refused too (its rows are teams).
fn check_page(
    page: &RankingsPage,
    list_id: i64,
    season: i64,
    event: &str,
    grades: &Grades,
    number: u32,
) -> Result<()> {
    if let Some(got) = page.event_short.as_deref()
        && !got.trim().eq_ignore_ascii_case(event)
    {
        bail!(
            "Athletic.net does not know the event code {event:?}: it answered with the {got} \
             list instead. --event takes a flat race (\"400m\") or an Athletic.net event code \
             such as Shot, LJ or HJ (the last part of a rankings list page's URL)"
        );
    }
    if let Some(s) = page.division.as_ref().and_then(|d| d.season_id)
        && s != season
    {
        bail!("rankings list {list_id} is in season {s}, not {season} (see --season)");
    }
    if page.division.is_none() && number == 1 {
        bail!("Athletic.net has no rankings list {list_id}");
    }
    if let Some(id) = page.division.as_ref().and_then(|d| d.id)
        && id != list_id
    {
        bail!("Athletic.net answered for rankings list {list_id} with list {id}");
    }
    let relay = match &page.relay_teams {
        serde_json::Value::Object(teams) => !teams.is_empty(),
        serde_json::Value::Array(teams) => !teams.is_empty(),
        _ => false,
    };
    if relay {
        bail!(
            "{event} is a relay: its list ranks teams, not people, and ath reads individual \
             events only (relays are out of v1 scope)"
        );
    }
    if let Some(applied) = applied_grades(page)
        && applied != grades.as_slice()
    {
        bail!(
            "Athletic.net filtered rankings list {list_id} to {} instead of {grades}",
            Grades(applied)
        );
    }
    Ok(())
}

/// The Grade group Athletic.net says it applied, in order; `None` when the
/// page does not say (as for every grade) or says it in a way `ath` cannot
/// read (then nothing is checked).
fn applied_grades(page: &RankingsPage) -> Option<Vec<i64>> {
    let serde_json::Value::Array(list) = &page.settings.as_ref()?.grades else {
        return None;
    };
    let mut grades = list
        .iter()
        .map(|g| match g {
            serde_json::Value::Number(n) => n.as_i64(),
            serde_json::Value::String(s) => s.trim().parse().ok(),
            _ => None,
        })
        .collect::<Option<Vec<i64>>>()?;
    grades.sort_unstable();
    grades.dedup();
    Some(grades)
}

impl Report for RankingsList {
    const SCHEMA: u32 = 1;

    fn table(&self) -> String {
        let l = &self.list;
        let what = match (l.kind, l.state.as_deref(), l.level.as_deref()) {
            (ListKind::Area, Some(st), Some(level)) => format!(" ({st} {level})"),
            (ListKind::State, Some(st), _) => format!(" ({st})"),
            _ => String::new(),
        };
        let grades = self
            .grades
            .as_ref()
            .map_or_else(Grades::all, |g| Grades(g.clone()));
        let filters: Vec<&str> = [
            (self.fat == Some(true)).then_some("FAT"),
            (self.official == Some(true)).then_some("official"),
        ]
        .into_iter()
        .flatten()
        .collect();
        let n = self.count;
        let how_many = match self.total {
            _ if self.complete => format!("{n} people"),
            Some(total) => format!("first {n} of {total}"),
            None => format!("first {n} (--limit)"),
        };
        let mut out = format!(
            "{}{what} {} {} {}, {grades}: {how_many}{}\n{}\n",
            l.name,
            self.season,
            self.gender.noun(),
            self.event_name.as_deref().unwrap_or(&self.event),
            if filters.is_empty() {
                String::new()
            } else {
                format!(" ({} marks)", filters.join(", "))
            },
            self.url
        );
        if let Some(note) = &self.note {
            out.push_str(note);
            out.push('\n');
        }
        out.push('\n');
        let opt = |v: Option<i64>| v.map(|n| n.to_string()).unwrap_or_default();
        let mut table = vec![vec![
            "Place".to_string(),
            "Name".into(),
            "Mark".into(),
            "Grade".into(),
            "Team".into(),
            "Athlete".into(),
        ]];
        table.extend(self.rows.iter().map(|r| {
            vec![
                format!("{}{}", if r.tied { "T" } else { "" }, r.place),
                r.name.clone(),
                r.mark.clone(),
                opt(r.grade),
                r.team.clone().unwrap_or_default(),
                r.athlete_id.to_string(),
            ]
        }));
        out.push_str(&columns(&table));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_marks_share_a_place() {
        assert_eq!(
            places(&[50, 54, 54, 54, 60, 61, 61]),
            [
                (1, false),
                (2, true),
                (2, true),
                (2, true),
                (5, false),
                (6, true),
                (6, true)
            ]
        );
        assert!(places(&[]).is_empty());
    }

    #[test]
    fn grade_groups_parse_ranges_lists_and_all() {
        let g = |s: &str| Grades::parse(s).map(|g| g.as_slice().to_vec());
        assert_eq!(g("9-10").unwrap(), [9, 10]);
        assert_eq!(g(" 9 - 12 ").unwrap(), [9, 10, 11, 12]);
        assert_eq!(g("11,9,11").unwrap(), [9, 11]);
        assert_eq!(g("9,11-12").unwrap(), [9, 11, 12]);
        assert_eq!(g("ALL").unwrap(), Vec::<i64>::new());
        assert!(g("8").unwrap_err().contains("9 to 12"));
        assert!(g("10-9").unwrap_err().contains("did you mean 9-10?"));
        assert!(g("").is_err());
        assert!(g("nine").is_err());
    }

    #[test]
    fn grade_groups_read_naturally() {
        let shown = |s: &str| Grades::parse(s).unwrap().to_string();
        assert_eq!(shown("9-10"), "grades 9-10");
        assert_eq!(shown("9,11"), "grades 9, 11");
        assert_eq!(shown("11"), "grade 11");
        assert_eq!(shown("all"), "all grades");
    }

    #[test]
    fn list_urls_carry_the_grade_filter() {
        let grades = Grades::parse("9-10").unwrap();
        assert_eq!(
            list_url(900502, Gender::F, "400m", &grades),
            "https://www.athletic.net/TrackAndField/rankings/list/900502/f/400m?grades=9,10"
        );
        assert_eq!(
            list_url(900500, Gender::M, "Shot", &Grades::all()),
            "https://www.athletic.net/TrackAndField/rankings/list/900500/m/Shot"
        );
    }

    #[test]
    fn flags_read_bools_and_numbers() {
        assert_eq!(flag(&serde_json::json!(true)), Some(true));
        assert_eq!(flag(&serde_json::json!(0)), Some(false));
        assert_eq!(flag(&serde_json::Value::Null), None);
    }
}
