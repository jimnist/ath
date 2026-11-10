//! `ath rank`: an Athlete's place on State and Area Rankings lists, for each
//! Grade group, with the Neighborhood around them (`CONTEXT.md`).
//!
//! The Athlete's mark is their own row on their home State's list (their
//! Season best as Athletic.net ranks it), looked up on their grade's list
//! first and on the every-grade list when Athletic.net lists them a grade off.
//! Every other list is compared against that one mark: rank is 1 plus the
//! number of people strictly faster (`SortIntCalc`), and equal marks are
//! ties (`T3`). Each list is read only as far as placing the Athlete and
//! filling the Neighborhood needs ([`read_pages`]).

use anyhow::{Context, Result};
use chrono::NaiveDate;
use serde::Serialize;

use crate::area::{
    AreaTree, Gender, NamedArea, US_STATES, area_level, area_named, areas_of, complete_area,
    configured_codes, flat_race_short, profile_area, season_tree, state_code,
};
use crate::contract::{Report, columns};
use crate::list::{
    Capped, Grades, ListInfo, ListKind, ListPages, ListRow, RankingsRow, list_url, places,
    read_pages,
};
use crate::mark::ResolvedEvent;
use crate::mark::{AsOf, bests_as_of, resolve_event, slowest_season_mark};
use crate::model::athlete_url;
use crate::source::DataSource;

/// At this rank or better (ties count: T50 is 50 or better) the Neighborhood
/// shows everyone ahead; past it, the leader and the few just ahead.
pub const EVERYONE_AHEAD_THROUGH: usize = 50;

/// How many people just ahead the Neighborhood shows past
/// [`EVERYONE_AHEAD_THROUGH`].
const JUST_AHEAD: usize = 3;

/// How many people just behind the Neighborhood shows.
const JUST_BEHIND: usize = 3;

/// Who a Neighborhood shows ahead of the Athlete (`--ahead`).
// Plain comments on the variants: doc comments would become `--help` text and
// switch `ath rank --help` to clap's long layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Ahead {
    // Everyone ahead at rank 50 or better; past it, the leader and the 3 just
    // ahead (`CONTEXT.md`).
    #[default]
    Neighborhood,
    // Everyone ahead, at any rank (`skipped` is then always 0). Costs no extra
    // reads: placing the Athlete reads everyone ahead anyway.
    All,
}

// ---------------------------------------------------------------------------
// Placing a mark on a list

/// Where a mark places on one list.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Place {
    /// 1 plus the number of people strictly faster.
    pub rank: usize,
    /// Whether someone else has exactly the mark.
    pub tied: bool,
    /// The rank as people say it: `13`, or `T103` for a tie.
    pub label: String,
    /// People strictly faster.
    pub ahead: usize,
    /// Other people with exactly the mark.
    pub ties: usize,
    pub neighborhood: Neighborhood,
}

/// The people a report shows around the Athlete on one list (`CONTEXT.md`):
/// at rank 50 or better everyone ahead, past 50 the leader and the 3 just
/// ahead (everyone, with [`Ahead::All`]); then everyone tied and the 3 just
/// behind. The Athlete is never in it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Neighborhood {
    /// Best first.
    pub ahead: Vec<ListRow>,
    /// People ahead not shown, between the leader and the 3 just ahead (0 at
    /// rank 50 or better, and with [`Ahead::All`]).
    pub skipped: usize,
    pub tied: Vec<ListRow>,
    /// The 3 just behind (fewer at the end of the list), best first.
    pub behind: Vec<ListRow>,
}

/// Place `mark` among `rows` (a list in `SortIntCalc` order), leaving out the
/// Athlete's own row; `ahead` says who the Neighborhood shows ahead.
pub fn place(rows: &[RankingsRow], athlete_id: i64, mark: i64, ahead_shown: Ahead) -> Place {
    let others: Vec<&RankingsRow> = rows.iter().filter(|r| r.athlete_id != athlete_id).collect();
    // Places on the list with the Athlete's mark in it (their own row on their
    // home list, or where their mark would go on any other list), so a
    // Neighborhood reads in one order: on the home list these are the list's own.
    let mut marks: Vec<i64> = others.iter().map(|r| r.sort_int).collect();
    let at = marks.partition_point(|m| *m < mark);
    marks.insert(at, mark);
    let mut placed = places(&marks);
    placed.remove(at);
    let others: Vec<ListRow> = others
        .into_iter()
        .zip(placed)
        .map(|(r, p)| ListRow::placed(r.clone(), p))
        .collect();
    let ahead = &others[..at];
    let tied: Vec<ListRow> = others[at..]
        .iter()
        .take_while(|r| r.sort_int == mark)
        .cloned()
        .collect();
    let behind = others[at + tied.len()..]
        .iter()
        .take(JUST_BEHIND)
        .cloned()
        .collect();
    let rank = ahead.len() + 1;
    let shown: Vec<ListRow> = if rank <= EVERYONE_AHEAD_THROUGH || ahead_shown == Ahead::All {
        ahead.to_vec()
    } else {
        let mut shown = vec![ahead[0].clone()];
        shown.extend_from_slice(&ahead[ahead.len() - JUST_AHEAD..]);
        shown
    };
    Place {
        rank,
        tied: !tied.is_empty(),
        label: if tied.is_empty() { rank.to_string() } else { format!("T{rank}") },
        ahead: ahead.len(),
        ties: tied.len(),
        neighborhood: Neighborhood {
            skipped: ahead.len() - shown.len(),
            ahead: shown,
            tied,
            behind,
        },
    }
}

/// Whether `rows` (in `SortIntCalc` order) already place `mark` and fill its
/// Neighborhood: everyone faster or tied is read once someone is behind, and
/// the Neighborhood needs the 3 just behind.
fn placed(rows: &[RankingsRow], athlete_id: i64, mark: i64) -> bool {
    rows.iter()
        .filter(|r| r.athlete_id != athlete_id && r.sort_int > mark)
        .count()
        >= JUST_BEHIND
}

// ---------------------------------------------------------------------------
// Grade groups

/// The Grade groups for an Athlete in `grade` (`CONTEXT.md`): 9th through
/// their grade, then one more class at a time up to all of high school. A 9th
/// grader also gets 9th alone. An unknown grade gets all of high school only.
pub fn grade_groups(grade: Option<i64>) -> Vec<Grades> {
    let Some(grade) = grade else {
        return vec![Grades::high_school(12)];
    };
    (grade.clamp(9, 12)..=12).map(Grades::high_school).collect()
}

/// How a Grade group reads: `9th`, `9th-10th`, `9th-11th`, `All HS`.
pub fn group_label(grades: &Grades) -> String {
    match grades.as_slice() {
        [9, 10, 11, 12] => "All HS".to_string(),
        [] => "All grades".to_string(),
        [one] => ordinal(*one),
        [first, .., last] if last - first + 1 == grades.as_slice().len() as i64 => {
            format!("{}-{}", ordinal(*first), ordinal(*last))
        }
        some => some
            .iter()
            .map(|g| ordinal(*g))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

fn ordinal(n: i64) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

// ---------------------------------------------------------------------------
// `ath rank`

/// One item of `--in`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// `home`: the Athlete's home State, and its Area when the State has Areas.
    Home,
    /// `all`: every State's list, the 50 States and DC.
    AllStates,
    /// `areas`: every Area of the Athlete's home State (none, with a note,
    /// when the State has no Areas configured).
    HomeAreas,
    /// `areas:CA`: every Area of that State.
    AreasOf(String),
    /// A State code (`CA`), or an Area's name or abbreviation (`CCS`).
    Named(String),
}

/// Which lists `ath rank` places the Athlete on, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Targets(pub Vec<Target>);

impl Targets {
    /// The States whose Areas these targets need in `tree`, for
    /// [`complete_area`]: the home State for `areas`, the State of each
    /// `areas:ST`, and every configured State for a name that is neither a
    /// State nor an Area `tree` already knows.
    fn area_states(&self, tree: &AreaTree, home_state: &str) -> Vec<String> {
        let mut states = Vec::new();
        for target in &self.0 {
            match target {
                Target::Home | Target::AllStates => {}
                Target::HomeAreas => states.push(home_state.to_string()),
                Target::AreasOf(st) => states.extend(state_code(st).ok()),
                Target::Named(name) => {
                    if !US_STATES.contains(&name.trim().to_ascii_uppercase().as_str())
                        && area_named(tree, None, name).is_err()
                    {
                        states.extend(configured_codes().into_iter().map(str::to_string));
                    }
                }
            }
        }
        let mut seen = std::collections::HashSet::new();
        states.retain(|s| seen.insert(s.clone()));
        states
    }

    /// The default: the home State and its Area.
    pub fn home() -> Targets {
        Targets(vec![Target::Home])
    }

    /// `--in`, comma-separated: `all`, `home`, `areas`, `areas:CA`, State
    /// codes and Area names or abbreviations (`all,areas`, `CA,CCS,TX`).
    pub fn parse(s: &str) -> Result<Targets, String> {
        let items: Vec<Target> = s
            .split(',')
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(|n| match n.to_ascii_lowercase().as_str() {
                "all" => Target::AllStates,
                "home" => Target::Home,
                "areas" => Target::HomeAreas,
                lower => match lower.strip_prefix("areas:") {
                    Some(st) => Target::AreasOf(st.trim().to_ascii_uppercase()),
                    None => Target::Named(n.to_string()),
                },
            })
            .collect();
        if items.is_empty() {
            return Err(
                "--in takes `all`, `home`, `areas`, `areas:CA`, State codes and Area \
                        names, separated by commas (all,areas or CA,CCS)"
                    .to_string(),
            );
        }
        Ok(Targets(items))
    }

    /// Whether the home Area is asked for (the school is then looked up even
    /// with --home): for `home`, and for `areas`, whose report says which of
    /// the home State's Areas is the Athlete's own (`home_area`).
    fn wants_home_area(&self) -> bool {
        self.0.contains(&Target::Home) || self.0.contains(&Target::HomeAreas)
    }
}

/// What `ath rank` is asked for.
#[derive(Debug, Clone)]
pub struct RankQuery<'a> {
    pub athlete_id: i64,
    /// Athletic.net's `eventShort`.
    pub event: String,
    pub season: i64,
    pub targets: Targets,
    /// The Athlete's grade, when the Profile does not say (or says wrong).
    pub grade: Option<i64>,
    /// The Athlete's home State, when their school does not say.
    pub home: Option<&'a str>,
    /// Who each Neighborhood shows ahead of the Athlete.
    pub ahead: Ahead,
    /// The rankings list (any season) to find the season's lists from.
    pub from_list: i64,
    pub today: NaiveDate,
}

/// The Athlete a report is about.
#[derive(Debug, Clone, Serialize)]
pub struct AthleteRef {
    pub id: i64,
    /// First and last name.
    pub name: String,
    /// As the Profile has it, for a report that says "Jamie's".
    pub first_name: String,
    /// Their Athletic.net Profile.
    pub url: String,
}

/// The mark the Athlete is placed with: their own row on their home State's
/// list.
#[derive(Debug, Clone, Serialize)]
pub struct AthleteMark {
    /// As Athletic.net shows it.
    pub mark: String,
    /// `SortIntCalc`: lower is better.
    pub sort_int: i64,
    /// The grade Athletic.net lists the Athlete in.
    pub grade: Option<i64>,
    /// The team the row lists them under.
    pub team: Option<String>,
    pub team_id: Option<i64>,
    /// The list the row is on (with the Grade group it was found in).
    pub list: crate::area::ListRef,
}

/// Whether a result has a place, and if not, why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RankStatus {
    Ok,
    /// The Athlete has no mark in the event this season yet.
    NoMark,
    /// The list could not be read (the note has the error); the other lists
    /// are still good.
    LookupFailed,
}

/// The Athlete's place on one list for one Grade group.
#[derive(Debug, Clone, Serialize)]
pub struct RankResult {
    pub list: ListInfo,
    /// The Grade group.
    pub grades: Vec<i64>,
    /// The Grade group as people say it (`9th-10th`, `All HS`).
    pub group: String,
    /// The Athletic.net page for this list and Grade group.
    pub url: String,
    pub status: RankStatus,
    /// `null` unless `status` is `ok`.
    pub place: Option<Place>,
    /// Pages read.
    pub pages: u32,
    pub note: Option<String>,
}

/// An Athlete's places, as `ath rank` prints them.
#[derive(Debug, Clone, Serialize)]
pub struct Ranks {
    pub athlete: AthleteRef,
    pub season: i64,
    pub gender: Gender,
    /// Athletic.net's `eventShort` (`400m`).
    pub event: String,
    /// The event's full name (`400 Meters`), when known.
    pub event_name: Option<String>,
    /// The grade the Grade groups are for: the Profile's for the season,
    /// `--grade`, or the one Athletic.net lists the Athlete in.
    pub grade: Option<i64>,
    /// `ok`, or `no_mark` when the Athlete has no mark this season yet.
    pub status: RankStatus,
    /// `null` when `status` is `no_mark`.
    pub mark: Option<AthleteMark>,
    /// The Grade groups, narrowest first.
    pub groups: Vec<Vec<i64>>,
    /// The Athlete's own Area in the home State (their school's), when `home`
    /// or `areas` asked for it and the school is in one; `null` otherwise.
    pub home_area: Option<crate::area::AreaSummary>,
    pub note: Option<String>,
    /// List by list (in `--in` order), Grade group by Grade group.
    pub results: Vec<RankResult>,
}

/// A note when a State with Areas configured has none in this season's tree
/// (an older season, or a partial tree), so an empty `areas` is explained.
fn no_areas_note(tree: &AreaTree, state: &str, areas: &[NamedArea<'_>], notes: &mut Vec<String>) {
    if areas.is_empty() {
        notes.push(format!(
            "Athletic.net's {} tree has no {state} {} lists{}",
            tree.season,
            area_level(state).unwrap_or("Area"),
            tree.partial_areas_note(),
        ));
    }
}

/// The lists `targets` name in `tree`; `home` is the home State and its Area,
/// `home_state` the home State's code.
fn target_lists(
    tree: &AreaTree,
    targets: &Targets,
    home: &[ListInfo],
    home_state: &str,
    notes: &mut Vec<String>,
) -> Result<Vec<ListInfo>> {
    let mut lists: Vec<ListInfo> = Vec::new();
    for target in &targets.0 {
        match target {
            Target::Home => lists.extend_from_slice(home),
            Target::AllStates => {
                for st in US_STATES {
                    match tree.state(st) {
                        Some(_) => lists.push(ListInfo::state(tree, st)?),
                        None => notes.push(format!(
                            "Athletic.net's {} high school lists have no State {st}{}",
                            tree.season,
                            tree.partial_note()
                        )),
                    }
                }
            }
            Target::HomeAreas => match areas_of(tree, home_state) {
                Some(areas) => {
                    no_areas_note(tree, home_state, &areas, notes);
                    lists.extend(areas.iter().map(ListInfo::area));
                }
                None => notes.push(format!(
                    "home State {home_state} has no Areas configured, so `areas` adds no lists"
                )),
            },
            Target::AreasOf(st) => {
                let st = state_code(st)?;
                let areas = areas_of(tree, &st).with_context(|| {
                    format!("{st} has no Areas configured (add an areas/{st}.json)")
                })?;
                no_areas_note(tree, &st, &areas, notes);
                lists.extend(areas.iter().map(ListInfo::area));
            }
            Target::Named(name) => {
                let code = name.trim().to_ascii_uppercase();
                if US_STATES.contains(&code.as_str()) {
                    lists.push(ListInfo::state(tree, &code)?);
                } else {
                    lists.push(ListInfo::area(&area_named(tree, None, name)?));
                }
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    lists.retain(|l| seen.insert(l.id));
    Ok(lists)
}

/// The Athlete's own row on `list`: on their grade's list first, then on the
/// every-grade list (Athletic.net sometimes lists people a grade off). Reads
/// only until the row turns up, or until `past` says the rows read are past
/// anywhere the Athlete could be.
async fn find_own_row<S: DataSource + ?Sized>(
    source: &S,
    list: ListPages<'_>,
    athlete_id: i64,
    grade: Option<i64>,
    past: &dyn Fn(&[RankingsRow]) -> bool,
) -> Result<Option<(RankingsRow, Grades)>> {
    let mut tries = Vec::new();
    if let Some(g) = grade.filter(|g| (9..=12).contains(g)) {
        tries.push(Grades::only(g));
    }
    tries.push(Grades::all());
    for grades in tries {
        let pages = ListPages {
            grades: &grades,
            ..list
        };
        let read = read_pages(source, &pages, |rows, _| {
            rows.iter().any(|r| r.athlete_id == athlete_id) || past(rows)
        })
        .await?;
        if let Some(row) = read.rows.into_iter().find(|r| r.athlete_id == athlete_id) {
            return Ok(Some((row, grades)));
        }
    }
    Ok(None)
}

/// `ath rank`: the Athlete's place on each list `--in` names (their home State
/// and Area by default), for each Grade group.
pub async fn rank<S: DataSource + ?Sized>(source: &S, q: &RankQuery<'_>) -> Result<Ranks> {
    rank_with_progress(source, q, &|_, _| {}).await
}

/// [`rank`], calling `progress(done, total)` after each list and Grade group
/// is placed (an `--in all` run is about 200 of them).
pub async fn rank_with_progress<S: DataSource + ?Sized>(
    source: &S,
    q: &RankQuery<'_>,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Result<Ranks> {
    let (season, event) = (q.season, q.event.as_str());
    let bio = source.get_bio(q.athlete_id).await?;
    let name = bio.athlete.full_name();
    let gender = bio
        .athlete
        .gender
        .as_deref()
        .and_then(Gender::from_bio)
        .with_context(|| {
            format!("{name}'s Profile does not say whether to read the girls' or boys' lists")
        })?;
    let mut notes = Vec::new();
    let mut tree = season_tree(source, season, q.from_list).await?;

    // Home: the State (and Area) of the Athlete's school for the season. The
    // school is only looked up when it is needed: for the State without
    // --home, or for the home Area.
    let (home_state, area) = match q.home {
        Some(home) => {
            let area = if q.targets.wants_home_area() {
                profile_area(source, &bio, season, event).await.0
            } else {
                None
            };
            (state_code(home)?, area)
        }
        None => {
            let (area, lookup) = profile_area(source, &bio, season, event).await;
            let state = lookup.state.clone().with_context(|| {
                format!(
                    "cannot tell {name}'s State for {season}{}; pass --home with a two-letter \
                     State code",
                    lookup
                        .note
                        .as_deref()
                        .map_or_else(String::new, |n| format!(" ({n})"))
                )
            })?;
            // A school taken from another season may be in another State.
            if lookup.school_not_listed
                && let Some(school) = bio.school_in(season)
            {
                let last = school.team.year.map_or_else(
                    || "with no year".to_string(),
                    |y| format!("last listed {y}"),
                );
                notes.push(format!(
                    "home State {state} is from {} ({last}), as {name}'s Profile lists no \
                     school for {season}; pass --home to change it",
                    school.team.name.trim()
                ));
            }
            (state, area)
        }
    };
    let mut home = vec![ListInfo::state(&tree, &home_state)?];
    // Looked up without --home to find the State, but reported (and placed)
    // only when `home` or `areas` asks for it.
    let home_area = area
        .filter(|a| a.state == home_state)
        .filter(|_| q.targets.wants_home_area());
    if let Some(area) = home_area.clone() {
        home.push(ListInfo {
            id: area.id,
            name: area.name,
            kind: ListKind::Area,
            state: Some(area.state),
            level: Some(area.level),
        });
    }

    // The mark: the Athlete's own row on the home State's list. A Profile
    // with no Season best in the event (a running event it can read) has no
    // row, so its lists are not searched; nor does one that never ran a flat
    // race asked for. An event code the Profile cannot be read for (`Shot`)
    // is always searched.
    let profile_grade = q.grade.or_else(|| bio.grade_in(season));
    let event_def = resolve_event(&bio, event);
    let no_season_best = match &event_def {
        Some(e) => {
            !e.field
                && bests_as_of(&bio, &e.ids, season, AsOf::today(q.today))
                    .season_best
                    .is_none()
        }
        None => flat_race_short(event).is_some(),
    };
    let home_list = ListPages {
        list_id: home[0].id,
        season,
        gender,
        event,
        grades: &Grades::all(),
    };
    // The rows are past the Athlete once they are slower than every
    // Qualifying mark the Profile has this season. Unknown for a field event
    // (the Profile's field marks are not read), and for an event code until
    // the list's first rows name the event.
    let as_of = AsOf::today(q.today);
    let bound =
        |e: &ResolvedEvent| (!e.field).then(|| slowest_season_mark(&bio, &e.ids, season, as_of));
    let known_bound = event_def.as_ref().map(bound);
    let stopped_early = std::cell::Cell::new(false);
    let past = |rows: &[RankingsRow]| -> bool {
        let slowest = match known_bound {
            Some(b) => b,
            None => rows
                .first()
                .and_then(|r| r.event.as_deref())
                .and_then(|n| resolve_event(&bio, n))
                .and_then(|e| bound(&e)),
        };
        let past = match slowest {
            // A running event: past once slower than the slowest mark.
            Some(Some(slowest)) => rows.last().is_some_and(|r| r.sort_int > slowest),
            // No Qualifying mark this season: they are on no list.
            Some(None) => true,
            None => false,
        };
        stopped_early.set(stopped_early.get() || past);
        past
    };
    let own = if no_season_best {
        None
    } else {
        find_own_row(source, home_list, q.athlete_id, profile_grade, &past).await?
    };
    let grade = profile_grade.or(own.as_ref().and_then(|(r, _)| r.grade));
    match grade {
        None => notes.push(format!(
            "{name}'s grade for {season} is unknown (pass --grade), so only all of high school \
             is compared"
        )),
        Some(g) if g < 9 => notes.push(format!(
            "{name} is in grade {g} in {season}; high school lists start at 9th grade, so the \
             Grade groups do too"
        )),
        _ => {}
    }
    let groups = grade_groups(grade);
    // A State's Areas that cannot be read are a note, as missing ones were:
    // the run keeps every list it can place.
    let mut unread = Vec::new();
    for st in q.targets.area_states(&tree, &home_state) {
        if let Err(e) = complete_area(source, &mut tree, &st).await {
            unread.push(format!("could not read {st}'s Areas: {e:#}"));
        }
    }
    notes.extend(unread.iter().cloned());
    // A name that is not found may be one of the Areas that could not be
    // read: the error says so rather than only listing the Areas there are.
    let lists = target_lists(&tree, &q.targets, &home, &home_state, &mut notes).map_err(|e| {
        if unread.is_empty() { e } else { e.context(unread.join("; ")) }
    })?;
    let mark = own.as_ref().map(|(row, grades)| AthleteMark {
        mark: row.display.trim().to_string(),
        sort_int: row.sort_int,
        grade: row.grade,
        team: row.team_name.as_deref().map(|t| t.trim().to_string()),
        team_id: row.team_id,
        list: crate::area::ListRef {
            id: home[0].id,
            name: home[0].name.clone(),
            url: list_url(home[0].id, gender, event, grades),
        },
    });
    if mark.is_none() {
        notes.push(format!(
            "{name} has no {season} {event} mark on the {} list yet",
            home[0].name
        ));
        if stopped_early.get() {
            notes.push(format!(
                "the {} list was read only down past the slowest {season} mark on {name}'s \
                 Profile; if a mark was corrected since, re-run with --refresh",
                home[0].name
            ));
        }
    }

    let mut results = Vec::new();
    for list in &lists {
        for grades in &groups {
            let mut result = RankResult {
                list: list.clone(),
                grades: grades.as_slice().to_vec(),
                group: group_label(grades),
                url: list_url(list.id, gender, event, grades),
                status: RankStatus::NoMark,
                place: None,
                pages: 0,
                note: None,
            };
            if let Some((own, _)) = &own {
                let pages = ListPages {
                    list_id: list.id,
                    season,
                    gender,
                    event,
                    grades,
                };
                let mark = own.sort_int;
                match read_pages(source, &pages, |rows, _| placed(rows, q.athlete_id, mark)).await {
                    Ok(read) => {
                        result.status = RankStatus::Ok;
                        result.place = Some(place(&read.rows, q.athlete_id, mark, q.ahead));
                        result.pages = read.pages;
                        result.note = (!read.notes.is_empty()).then(|| read.notes.join("; "));
                    }
                    // A capped list is no one list's trouble: the session
                    // sees only the first rows of every list, so stop.
                    Err(e) if e.downcast_ref::<Capped>().is_some() => return Err(e),
                    Err(e) => {
                        result.status = RankStatus::LookupFailed;
                        result.note = Some(format!("{e:#}"));
                    }
                }
            }
            results.push(result);
            progress(results.len(), lists.len() * groups.len());
        }
    }
    if results.iter().any(|r| r.status == RankStatus::LookupFailed) {
        notes.push("some lists could not be read; see their notes".to_string());
    }
    Ok(Ranks {
        athlete: AthleteRef {
            id: q.athlete_id,
            name,
            first_name: bio.athlete.first_name.trim().to_string(),
            url: athlete_url(q.athlete_id),
        },
        season,
        gender,
        event: event.to_string(),
        event_name: own
            .as_ref()
            .and_then(|(r, _)| r.event.as_deref())
            .map(|e| e.trim().to_string())
            .or_else(|| event_def.map(|e| e.name)),
        grade,
        status: if mark.is_some() { RankStatus::Ok } else { RankStatus::NoMark },
        mark,
        groups: groups.iter().map(|g| g.as_slice().to_vec()).collect(),
        home_area,
        note: (!notes.is_empty()).then(|| notes.join("; ")),
        results,
    })
}

impl Report for Ranks {
    const SCHEMA: u32 = 1;

    fn table(&self) -> String {
        let gender = self.gender.noun();
        let event = self.event_name.as_deref().unwrap_or(&self.event);
        let grade = self
            .grade
            .map_or_else(String::new, |g| format!(", grade {g}"));
        let mark = match &self.mark {
            Some(m) => format!("{} on the {} list", m.mark, m.list.name),
            None => "no mark yet".to_string(),
        };
        let mut out = format!(
            "{} {} {gender} {event}{grade}: {mark}\n{}\n",
            self.athlete.name, self.season, self.athlete.url
        );
        if let Some(note) = &self.note {
            out.push_str(note);
            out.push('\n');
        }
        out.push('\n');
        // Results are list by list, a run of one per Grade group.
        let per_list = self.groups.len().max(1);
        let mut header = vec!["List".to_string()];
        header.extend(
            self.groups
                .iter()
                .map(|g| group_label(&Grades::from_slice(g))),
        );
        let mut rows = vec![header];
        for mine in self.results.chunks(per_list) {
            let l = &mine[0].list;
            let mut row = vec![match &l.state {
                Some(st) if l.kind == ListKind::State => format!("{} ({st})", l.name),
                Some(st) => format!("{} ({st} {})", l.name, l.level.as_deref().unwrap_or("Area")),
                None => l.name.clone(),
            }];
            row.extend(mine.iter().map(|r| match (&r.status, &r.place) {
                (RankStatus::Ok, Some(p)) => p.label.clone(),
                (RankStatus::LookupFailed, _) => "?".to_string(),
                _ => "-".to_string(),
            }));
            rows.push(row);
        }
        out.push_str(&columns(&rows));
        for r in self
            .results
            .iter()
            .filter(|r| r.status == RankStatus::LookupFailed)
        {
            out.push_str(&format!(
                "{} {}: {}\n",
                r.list.name,
                r.group,
                r.note.as_deref().unwrap_or("could not be read")
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row for `athlete_id` with mark `sort_int`.
    fn row(athlete_id: i64, sort_int: i64) -> RankingsRow {
        serde_json::from_value(serde_json::json!({
            "AthleteID": athlete_id, "AthleteName": format!("P{athlete_id}"),
            "display": sort_int.to_string(), "SortIntCalc": sort_int,
        }))
        .unwrap()
    }

    /// `n` people strictly faster than 60_000 (ids 1..=n, marks rising),
    /// then `tail` as given.
    fn list(n: i64, tail: &[(i64, i64)]) -> Vec<RankingsRow> {
        let mut rows: Vec<RankingsRow> = (1..=n).map(|i| row(i, 50_000 + i)).collect();
        rows.extend(tail.iter().map(|&(id, mark)| row(id, mark)));
        rows
    }

    const ME: i64 = 777;
    const MARK: i64 = 60_000;

    fn ids(rows: &[ListRow]) -> Vec<i64> {
        rows.iter().map(|r| r.athlete_id).collect()
    }

    #[test]
    fn first_on_the_list() {
        let rows = list(
            0,
            &[
                (ME, MARK),
                (1, 61_000),
                (2, 62_000),
                (3, 63_000),
                (4, 64_000),
            ],
        );
        let p = place(&rows, ME, MARK, Ahead::Neighborhood);
        assert_eq!(
            (p.rank, p.tied, p.label.as_str(), p.ahead),
            (1, false, "1", 0)
        );
        assert!(p.neighborhood.ahead.is_empty());
        assert_eq!(ids(&p.neighborhood.behind), [1, 2, 3]);
        assert!(p.neighborhood.tied.is_empty());
    }

    #[test]
    fn a_tie_shares_the_rank() {
        // Off the Athlete's own list (another State): nobody is left out.
        let rows = list(2, &[(500, MARK), (501, MARK), (502, 61_000)]);
        let p = place(&rows, ME, MARK, Ahead::Neighborhood);
        assert_eq!((p.rank, p.label.as_str(), p.ties), (3, "T3", 2));
        assert_eq!(ids(&p.neighborhood.ahead), [1, 2]);
        assert_eq!(ids(&p.neighborhood.tied), [500, 501]);
        assert_eq!(ids(&p.neighborhood.behind), [502]);
        // On their own list, their own row is no tie.
        let rows = list(2, &[(ME, MARK), (502, 61_000)]);
        let p = place(&rows, ME, MARK, Ahead::Neighborhood);
        assert_eq!((p.label.as_str(), p.ties), ("3", 0));
    }

    #[test]
    fn neighborhood_places_count_the_athletes_mark() {
        let places = |rows: &[ListRow]| -> Vec<(usize, bool)> {
            rows.iter().map(|r| (r.place, r.tied)).collect()
        };
        // Off her list: the two tied with her share her place, and the one
        // behind is 6th with her mark counted, not 5th as on the list itself.
        let p = place(
            &list(2, &[(500, MARK), (501, MARK), (502, 61_000)]),
            ME,
            MARK,
            Ahead::Neighborhood,
        );
        let n = &p.neighborhood;
        assert_eq!(places(&n.ahead), [(1, false), (2, false)]);
        assert_eq!(places(&n.tied), [(3, true), (3, true)]);
        assert_eq!(places(&n.behind), [(6, false)]);
        // On her own list the places are the list's own; equal marks ahead of
        // her share a place, which Athletic.net's row numbers do not.
        let rows = vec![
            row(1, 50_000),
            row(2, 50_000),
            row(ME, MARK),
            row(3, 61_000),
        ];
        let p = place(&rows, ME, MARK, Ahead::Neighborhood);
        let n = &p.neighborhood;
        assert_eq!(
            (p.label.as_str(), places(&n.ahead)),
            ("3", vec![(1, true), (1, true)])
        );
        assert_eq!(places(&n.behind), [(4, false)]);
    }

    #[test]
    fn at_exactly_50_everyone_ahead_shows() {
        let p = place(
            &list(49, &[(ME, MARK), (900, 61_000)]),
            ME,
            MARK,
            Ahead::Neighborhood,
        );
        assert_eq!((p.rank, p.label.as_str()), (50, "50"));
        assert_eq!(p.neighborhood.ahead.len(), 49);
        assert_eq!(p.neighborhood.skipped, 0);
        // T50 is "50 or better" too.
        let p = place(&list(49, &[(900, MARK)]), ME, MARK, Ahead::Neighborhood);
        assert_eq!((p.label.as_str(), p.neighborhood.ahead.len()), ("T50", 49));
    }

    #[test]
    fn past_50_the_leader_and_the_3_just_ahead() {
        let rows = list(
            60,
            &[
                (ME, MARK),
                (900, 61_000),
                (901, 62_000),
                (902, 63_000),
                (903, 64_000),
            ],
        );
        let p = place(&rows, ME, MARK, Ahead::Neighborhood);
        assert_eq!((p.rank, p.ahead), (61, 60));
        assert_eq!(ids(&p.neighborhood.ahead), [1, 58, 59, 60]);
        assert_eq!(p.neighborhood.skipped, 56);
        assert_eq!(ids(&p.neighborhood.behind), [900, 901, 902]);
        // Rank 51 exactly: past 50.
        let p = place(&list(50, &[]), ME, MARK, Ahead::Neighborhood);
        assert_eq!(
            (p.rank, p.neighborhood.ahead.len(), p.neighborhood.skipped),
            (51, 4, 46)
        );
    }

    #[test]
    fn ahead_all_shows_everyone_ahead_at_any_rank() {
        let rows = list(60, &[(ME, MARK), (500, MARK), (900, 61_000)]);
        let p = place(&rows, ME, MARK, Ahead::All);
        assert_eq!((p.rank, p.label.as_str(), p.ahead), (61, "T61", 60));
        let n = &p.neighborhood;
        assert_eq!(ids(&n.ahead), (1..=60).collect::<Vec<_>>());
        assert_eq!(n.skipped, 0);
        // The rest of the Neighborhood is as always.
        assert_eq!((ids(&n.tied), ids(&n.behind)), (vec![500], vec![900]));
        let usual = place(&rows, ME, MARK, Ahead::Neighborhood);
        assert_eq!((usual.rank, usual.label), (p.rank, p.label));
    }

    #[test]
    fn placed_once_three_are_behind() {
        let rows = list(5, &[(ME, MARK), (900, MARK), (901, 61_000), (902, 62_000)]);
        assert!(!placed(&rows, ME, MARK));
        let mut rows = rows;
        rows.push(row(903, 63_000));
        assert!(placed(&rows, ME, MARK));
    }

    #[test]
    fn in_takes_a_mix_of_lists() {
        assert_eq!(
            Targets::parse(" all , Areas ,areas:ca, home,CCS,TX").unwrap(),
            Targets(vec![
                Target::AllStates,
                Target::HomeAreas,
                Target::AreasOf("CA".into()),
                Target::Home,
                Target::Named("CCS".into()),
                Target::Named("TX".into()),
            ])
        );
        assert!(Targets::parse(" , ").is_err());
        assert!(Targets::home().wants_home_area());
        assert!(Targets::parse("all,areas").unwrap().wants_home_area());
        // Each State once, whatever its place in --in.
        let tree = AreaTree {
            season: 2026,
            regions: Vec::new(),
            national: true,
        };
        let states = Targets::parse("areas,areas:CA,areas:me")
            .unwrap()
            .area_states(&tree, "ME");
        assert_eq!(states, ["ME", "CA"]);
        assert!(!Targets::parse("all").unwrap().wants_home_area());
    }

    #[test]
    fn areas_follows_the_home_state() {
        let nav: crate::area::NavInfo = serde_json::from_str(
            r#"{"regions": [
                {"id": 10, "name": "California", "state": "CA", "divType": "State", "depth": 1},
                {"id": 11, "name": " Central Coast", "state": "CA", "divType": "Section", "depth": 2},
                {"id": 12, "name": " Central", "state": "CA", "divType": "Section", "depth": 2},
                {"id": 20, "name": "Texas", "state": "TX", "divType": "State", "depth": 1},
                {"id": 21, "name": "Central", "state": "TX", "divType": "League", "depth": 2}
            ]}"#,
        )
        .unwrap();
        let tree = AreaTree::from_nav(2026, nav);
        let areas = Targets::parse("areas").unwrap();
        let mut notes = Vec::new();
        let ca = target_lists(&tree, &areas, &[], "CA", &mut notes).unwrap();
        let got: Vec<(i64, &str, ListKind)> =
            ca.iter().map(|l| (l.id, l.name.as_str(), l.kind)).collect();
        assert_eq!(
            got,
            [
                (12, "Central", ListKind::Area),
                (11, "Central Coast", ListKind::Area)
            ]
        );
        assert!(notes.is_empty());
        // Texas has no Areas configured: nothing added, and a note says so.
        let tx = target_lists(&tree, &areas, &[], "TX", &mut notes).unwrap();
        assert!(tx.is_empty());
        assert_eq!(
            notes,
            ["home State TX has no Areas configured, so `areas` adds no lists"]
        );
        // California has Areas configured, but a season's tree may list none.
        let bare: crate::area::NavInfo = serde_json::from_str(
            r#"{"regions": [{"id": 10, "name": "California", "state": "CA", "divType": "State", "depth": 1}]}"#,
        )
        .unwrap();
        let bare = AreaTree::from_nav(2015, bare);
        let mut notes = Vec::new();
        assert!(
            target_lists(&bare, &areas, &[], "CA", &mut notes)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            notes,
            [
                "Athletic.net's 2015 tree has no CA Section lists (Athletic.net answered with only part \
              of the national high school tree, so they may still exist; --refresh asks again)"
            ]
        );
    }

    #[test]
    fn grade_groups_widen_to_all_of_high_school() {
        let labels = |g| grade_groups(g).iter().map(group_label).collect::<Vec<_>>();
        assert_eq!(labels(Some(9)), ["9th", "9th-10th", "9th-11th", "All HS"]);
        assert_eq!(labels(Some(10)), ["9th-10th", "9th-11th", "All HS"]);
        assert_eq!(labels(Some(12)), ["All HS"]);
        // An 8th grader on a high school list compares like a 9th grader.
        assert_eq!(labels(Some(8)), labels(Some(9)));
        assert_eq!(labels(None), ["All HS"]);
        assert_eq!(group_label(&Grades::parse("9,11").unwrap()), "9th, 11th");
    }
}
