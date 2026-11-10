//! Domain types for Athletic.net bio data and seed output.
//!
//! Field names mirror the bio API JSON (see `docs/agents/systems/athletic-net.md`
//! and `CONTEXT.md`). We deserialize only the fields the pipeline needs; unknown
//! fields are ignored.

use std::collections::HashMap;

use serde::{Deserialize, Deserializer, Serialize};

/// A full athlete bio, as returned by `GetAthleteBioData`.
#[derive(Debug, Clone, Deserialize)]
pub struct Bio {
    pub athlete: Athlete,
    /// Every team the athlete has been affiliated with, keyed by SchoolID (as a
    /// string, matching the JSON object keys).
    #[serde(rename = "allTeams", default)]
    pub all_teams: HashMap<String, Team>,
    /// Event definitions for events this athlete has competed in.
    #[serde(rename = "eventsTF", default)]
    pub events_tf: Vec<EventDef>,
    /// Individual and relay results.
    #[serde(rename = "resultsTF", default)]
    pub results_tf: Vec<ResultRow>,
    /// The meets the results were set at, keyed by MeetID (as a string). Real
    /// bios carry meet names here; each result's own `meetName` is null.
    #[serde(default)]
    pub meets: HashMap<String, Meet>,
    /// Which team the athlete was on in which outdoor season, from the bio's
    /// `grades` map (`"<schoolId>_<season>"` -> grade), as `(team id,
    /// season)` -> the grade when it is a number. The map lists clubs too
    /// (whose values can be odd, such as 402), and indoor seasons (`12026`),
    /// which are left out here.
    #[serde(rename = "grades", default, deserialize_with = "team_seasons")]
    pub team_seasons: TeamGrades,
}

/// A bio's `grades` map: `(team id, outdoor season)` -> the grade, when it
/// is a number.
pub type TeamGrades = HashMap<(i64, i64), Option<i64>>;

/// Indoor seasons are numbered from here (`12026` is the 2026 indoor season).
const INDOOR_SEASONS: i64 = 10_000;

/// A bio's `grades` map as `(team id, outdoor season)` -> grade. Anything
/// that is not a map of `"<id>_<season>"` keys reads as empty: the map only
/// sharpens which school (and grade) was the athlete's, it never fails a bio.
fn team_seasons<'de, D: Deserializer<'de>>(d: D) -> Result<TeamGrades, D::Error> {
    let raw = Option::<serde_json::Value>::deserialize(d)?;
    let Some(serde_json::Value::Object(map)) = raw else {
        return Ok(HashMap::new());
    };
    Ok(map
        .iter()
        .filter_map(|(k, v)| Some((parse_grade_key(k)?, v.as_i64())))
        .collect())
}

/// `"13107_2026"` -> `(13107, 2026)`; `None` for an indoor season or a key
/// that is not `<id>_<season>`.
fn parse_grade_key(key: &str) -> Option<(i64, i64)> {
    let (team, season) = key.rsplit_once('_')?;
    let (team, season) = (team.trim().parse().ok()?, season.trim().parse().ok()?);
    (season < INDOOR_SEASONS).then_some((team, season))
}

#[derive(Debug, Clone, Deserialize)]
pub struct Meet {
    #[serde(rename = "MeetName", default)]
    pub name: Option<String>,
}

impl Bio {
    /// The name of the meet a result was set at: the result's own `meetName`
    /// when present, otherwise the bio's `meets` entry for its MeetID.
    pub fn meet_name(&self, r: &ResultRow) -> Option<String> {
        r.meet_name.clone().or_else(|| {
            r.meet_id
                .and_then(|id| self.meets.get(&id.to_string()))
                .and_then(|m| m.name.clone())
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Athlete {
    #[serde(rename = "IDAthlete")]
    pub id: i64,
    /// Current/primary school. May be absent for sparse athletes.
    #[serde(rename = "SchoolID", default)]
    pub school_id: Option<i64>,
    #[serde(rename = "FirstName", default)]
    pub first_name: String,
    #[serde(rename = "LastName", default)]
    pub last_name: String,
    #[serde(rename = "Gender", default)]
    pub gender: Option<String>,
}

impl Athlete {
    pub fn full_name(&self) -> String {
        format!("{} {}", self.first_name.trim(), self.last_name.trim())
            .trim()
            .to_string()
    }
}

/// A team affiliation. `level` classifies the team: 2 = middle school,
/// 4 = high school, 16 = club (higher ≈ college).
#[derive(Debug, Clone, Deserialize)]
pub struct Team {
    #[serde(rename = "IDSchool")]
    pub id: i64,
    #[serde(rename = "SchoolName", default)]
    pub name: String,
    #[serde(rename = "Level", default)]
    pub level: i64,
    #[serde(rename = "Year", default)]
    pub year: Option<i64>,
}

impl Team {
    pub fn is_school(&self) -> bool {
        matches!(self.level, 2 | 4)
    }
    pub fn is_club(&self) -> bool {
        self.level == 16
    }
    pub fn url(&self) -> String {
        team_url(self.id)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct EventDef {
    #[serde(rename = "Event", default)]
    pub name: String,
    #[serde(rename = "IDEvent")]
    pub id_event: i64,
    #[serde(rename = "IDEventType", default)]
    pub id_event_type: i64,
    #[serde(rename = "Type", default)]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResultRow {
    /// The athlete (or, for relay results, the relay's TeamID) this result
    /// belongs to. Individual marks match the bio's own athlete id.
    #[serde(rename = "AthleteID")]
    pub athlete_id: i64,
    #[serde(rename = "Result", default)]
    pub result: String,
    #[serde(rename = "SortInt")]
    pub sort_int: i64,
    #[serde(rename = "EventID")]
    pub event_id: i64,
    /// 0 = standard individual mark; 98 = relay split; other = implement weight
    /// for field events.
    #[serde(rename = "EventTypeID", default)]
    pub event_type_id: i64,
    #[serde(rename = "SeasonID")]
    pub season_id: i64,
    #[serde(rename = "Exhibition", default)]
    pub exhibition: bool,
    /// When the mark was set, as Athletic.net sends it (`2026-05-16T00:00:00`).
    /// Use [`ResultRow::date`] for the calendar date.
    #[serde(rename = "ResultDate", default)]
    pub result_date: Option<String>,
    /// The team the mark was set under (not necessarily the current school).
    #[serde(rename = "SchoolID", default)]
    pub school_id: Option<i64>,
    #[serde(rename = "MeetID", default)]
    pub meet_id: Option<i64>,
    /// Usually null on real bios; prefer [`Bio::meet_name`].
    #[serde(rename = "meetName", default)]
    pub meet_name: Option<String>,
    // `PersonalBest` / `SeasonBest` are deliberately not parsed: they are
    // bitmasks marking the results that are the best *today*, not results that
    // were a best when set, so they cannot answer "as of a date" (CONTEXT.md).
}

impl ResultRow {
    /// The calendar date the mark was set, or `None` when the result carries no
    /// (parseable) `ResultDate`.
    pub fn date(&self) -> Option<chrono::NaiveDate> {
        let raw = self.result_date.as_deref()?.trim();
        let day = raw.get(..10)?;
        chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()
    }
}

/// A candidate athlete returned by search (used by Mode B resolution).
#[derive(Debug, Clone, Deserialize)]
pub struct Candidate {
    pub athlete_id: i64,
    pub name: String,
    #[serde(default)]
    pub school: Option<String>,
    #[serde(default)]
    pub city: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    /// Search-engine relevance score (higher = better textual match).
    #[serde(default)]
    pub score: f64,
}

/// One row of a seed sheet. Serialized to CSV with these exact column names.
#[derive(Debug, Clone, Serialize)]
pub struct SeedRow {
    pub rank: usize,
    pub athlete: String,
    pub athlete_id: i64,
    pub athlete_url: String,
    /// The Seed mark: the Personal record in the event as of the seed date
    /// (today, or the day before a meet held today), displayed
    /// with the trailing FAT `a` stripped; empty when no Qualifying mark.
    pub pr: String,
    /// Numeric sort key (seconds); empty when no Qualifying mark.
    pub pr_numeric: Option<f64>,
    pub school: String,
    pub school_url: String,
    pub club: String,
    pub club_url: String,
    /// The season the Personal record was set in (`SeasonID`); empty when no
    /// Qualifying mark. Last, so existing columns keep their positions.
    pub pr_season: Option<i64>,
    /// The Personal record's `SortInt`, the key the sheet is sorted on; empty
    /// when no Qualifying mark. Not a CSV column (`pr_numeric` is its CSV
    /// form); `ath seed`'s JSON carries it (`seed::Entrant`).
    #[serde(skip)]
    pub sort_int: Option<i64>,
}

pub fn athlete_url(athlete_id: i64) -> String {
    format!("https://www.athletic.net/athlete/{athlete_id}/track-and-field")
}

pub fn team_url(school_id: i64) -> String {
    format!("https://www.athletic.net/team/{school_id}/track-and-field-outdoor")
}

/// The school [`Bio::school_in`] picked, and how it knows.
#[derive(Debug, Clone, Copy)]
pub struct SchoolPick<'a> {
    pub team: &'a Team,
    pub basis: SchoolBasis,
}

/// How [`Bio::school_in`] knows a school was the athlete's for a season.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchoolBasis {
    /// The bio's `grades` map lists the athlete on the school that season.
    Grades,
    /// No `grades` entry for the season; the school's `Year` covers it.
    Year,
    /// No school's `Year` reaches the season: the current school.
    Latest,
    /// The season is before every school's `Year`: the earliest school, a
    /// guess (the athlete may not have been on it, or on any school, yet).
    Guess,
}

impl SchoolPick<'_> {
    /// Whether the Profile lists the school for the season (`grades` or
    /// `Year`), rather than it being a fallback or a guess.
    pub fn in_season(&self) -> bool {
        matches!(self.basis, SchoolBasis::Grades | SchoolBasis::Year)
    }
}

impl Bio {
    /// The athlete's current school, for a seed sheet's School column: a high
    /// school over a middle school, then the latest `Year`, then the
    /// Profile's own `SchoolID`, then the id. A stale middle school never
    /// shows while the Profile has a high school.
    pub fn school(&self) -> Option<&Team> {
        let own = |t: &Team| self.athlete.school_id == Some(t.id);
        self.schools()
            // HashMap order is nondeterministic, hence the id.
            .max_by_key(|t| (t.level, t.year.unwrap_or(0), own(t), t.id))
    }

    /// The athlete's school (middle or high, never a club) for an outdoor
    /// season: the one rule for the Profile's School and the school that
    /// decides its Area.
    ///
    /// 1. The `grades` map: the school with a `<id>_<season>` key. Several
    ///    prefer a high school, then the Profile's own `SchoolID`, then the
    ///    lowest id.
    /// 2. Only without such a key, each school's `Year`, the last season the
    ///    athlete was on it (no `Year` counts as current): the school with the
    ///    earliest `Year` at or after the season (a transfer's later school
    ///    only from its first season on). Ties prefer the Profile's own
    ///    `SchoolID`, then a high school, then the lowest id. When the season
    ///    is before every school's `Year`, that is only a
    ///    [`SchoolBasis::Guess`].
    /// 3. When no school's `Year` reaches the season, the current school: the
    ///    Profile's own `SchoolID` when that is a school, otherwise the most
    ///    recent one ([`SchoolBasis::Latest`]).
    pub fn school_in(&self, season: i64) -> Option<SchoolPick<'_>> {
        use std::cmp::Reverse;
        let own = |t: &Team| self.athlete.school_id == Some(t.id);
        let pick = |team, basis| Some(SchoolPick { team, basis });

        let graded = self
            .schools()
            .filter(|t| self.team_seasons.contains_key(&(t.id, season)))
            .max_by_key(|t| (t.level, own(t), Reverse(t.id)));
        if let Some(team) = graded {
            return pick(team, SchoolBasis::Grades);
        }

        let year = |t: &Team| t.year.unwrap_or(i64::MAX);
        let covering = self
            .schools()
            .filter(|t| year(t) >= season)
            .min_by_key(|t| (year(t), Reverse(own(t)), Reverse(t.level), t.id));
        if let Some(team) = covering {
            let before_every_year = self.schools().all(|t| t.year.is_some_and(|y| y > season));
            let basis = if before_every_year { SchoolBasis::Guess } else { SchoolBasis::Year };
            return pick(team, basis);
        }

        let latest = self
            .schools()
            .max_by_key(|t| (own(t), year(t), t.level, t.id))?;
        pick(latest, SchoolBasis::Latest)
    }

    /// The athlete's grade in an outdoor season (9 to 12 in high school): the
    /// `grades` map's value for the season's school ([`Bio::school_in`]),
    /// only when the map lists that school for the season. `None` when the
    /// Profile does not say.
    pub fn grade_in(&self, season: i64) -> Option<i64> {
        let pick = self.school_in(season)?;
        if pick.basis != SchoolBasis::Grades {
            return None;
        }
        let grade = (*self.team_seasons.get(&(pick.team.id, season))?)?;
        (1..=12).contains(&grade).then_some(grade)
    }

    /// The athlete's schools (middle and high), never clubs.
    fn schools(&self) -> impl Iterator<Item = &Team> {
        self.all_teams.values().filter(|t| t.is_school())
    }

    /// The athlete's club affiliation, most recent year first.
    pub fn club(&self) -> Option<&Team> {
        self.all_teams
            .values()
            .filter(|t| t.is_club())
            // Recency, then id as a stable tiebreak.
            .max_by_key(|t| (t.year.unwrap_or(0), t.id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bio with `teams` (`(id, name, level, year)`), its own `SchoolID`
    /// and a `grades` map.
    fn bio(own: i64, teams: &[(i64, &str, i64, Option<i64>)], grades: serde_json::Value) -> Bio {
        let all_teams: serde_json::Map<String, serde_json::Value> = teams
            .iter()
            .map(|&(id, name, level, year)| {
                let mut t = serde_json::json!({"IDSchool": id, "SchoolName": name, "Level": level});
                if let Some(y) = year {
                    t["Year"] = y.into();
                }
                (id.to_string(), t)
            })
            .collect();
        serde_json::from_value(serde_json::json!({
            "athlete": {"IDAthlete": 1, "SchoolID": own, "FirstName": "A", "LastName": "B"},
            "allTeams": all_teams,
            "grades": grades,
        }))
        .unwrap()
    }

    fn school_in(b: &Bio, season: i64) -> (i64, SchoolBasis) {
        let p = b.school_in(season).unwrap();
        (p.team.id, p.basis)
    }

    #[test]
    fn grade_keys_are_team_and_outdoor_season() {
        assert_eq!(parse_grade_key("13107_2026"), Some((13107, 2026)));
        assert_eq!(parse_grade_key("79911_12026"), None, "indoor");
        assert_eq!(parse_grade_key("13107"), None);
        assert_eq!(parse_grade_key("x_2026"), None);
        let b = bio(
            1,
            &[],
            serde_json::json!({"7_2025": 8, "9_12026": 402, "8_2026": "x"}),
        );
        assert_eq!(
            b.team_seasons,
            HashMap::from([((7, 2025), Some(8)), ((8, 2026), None)])
        );
        // A missing or odd `grades` never fails a bio.
        for odd in [serde_json::Value::Null, serde_json::json!([]), "x".into()] {
            assert!(bio(1, &[], odd).team_seasons.is_empty());
        }
    }

    #[test]
    fn the_grade_is_the_seasons_schools() {
        let teams = [
            (35958, "Middle", 2, None),
            (13107, "High", 4, Some(2026)),
            (79911, "Club", 16, Some(2026)),
        ];
        let grades = serde_json::json!({
            "35958_2025": 8, "13107_2026": 9, "79911_2026": 402, "13107_2027": 99
        });
        let b = bio(13107, &teams, grades);
        assert_eq!(b.grade_in(2026), Some(9));
        assert_eq!(b.grade_in(2025), Some(8));
        // Not listed for the season (a school by `Year` only): unknown.
        assert_eq!(b.grade_in(2024), None);
        // Not a grade.
        assert_eq!(b.grade_in(2027), None);
    }

    #[test]
    fn grades_decide_a_transfer() {
        // Both schools' `Year` reaches 2025; the grades say which it was.
        let b = bio(
            20,
            &[
                (10, "Old High", 4, Some(2026)),
                (20, "New High", 4, Some(2026)),
            ],
            serde_json::json!({"10_2025": 10, "20_2026": 11}),
        );
        assert_eq!(school_in(&b, 2025), (10, SchoolBasis::Grades));
        assert_eq!(school_in(&b, 2026), (20, SchoolBasis::Grades));
    }

    #[test]
    fn grades_decide_middle_school_then_high_school() {
        // Like a real Profile: the middle school's `Year` is missing, so
        // `Year` alone would say the high school for 2025 too.
        let b = bio(
            5103,
            &[
                (5103, "City High", 4, Some(2026)),
                (701, "Hilltop Middle", 2, None),
                (9201, "Bayside TC", 16, Some(2026)),
            ],
            serde_json::json!({"5103_2026": 9, "701_2025": 8, "701_2024": 7, "9201_2026": 9}),
        );
        assert_eq!(school_in(&b, 2026), (5103, SchoolBasis::Grades));
        assert_eq!(school_in(&b, 2025), (701, SchoolBasis::Grades));
        assert_eq!(school_in(&b, 2024), (701, SchoolBasis::Grades));
    }

    #[test]
    fn grades_never_pick_a_club() {
        // Only the club is graded; the school comes from `Year`.
        let b = bio(
            9201,
            &[
                (9201, "Bayside TC", 16, Some(2026)),
                (5103, "City High", 4, Some(2026)),
            ],
            serde_json::json!({"9201_2025": 402, "9201_2026": 402}),
        );
        assert_eq!(school_in(&b, 2026), (5103, SchoolBasis::Year));
        assert_eq!(school_in(&b, 2025), (5103, SchoolBasis::Guess));
        // Club only: no school at all.
        let b = bio(
            9201,
            &[(9201, "Bayside TC", 16, Some(2026))],
            serde_json::json!({"9201_2026": 9}),
        );
        assert!(b.school_in(2026).is_none());
    }

    #[test]
    fn several_graded_schools_prefer_high_school_then_own() {
        let b = bio(
            30,
            &[
                (10, "A Middle", 2, Some(2026)),
                (20, "B High", 4, Some(2026)),
                (30, "C High", 4, Some(2026)),
            ],
            serde_json::json!({"10_2026": 8, "20_2026": 9, "30_2026": 9}),
        );
        assert_eq!(school_in(&b, 2026), (30, SchoolBasis::Grades));
    }

    #[test]
    fn a_school_without_a_year_is_current() {
        let b = bio(5103, &[(5103, "City High", 4, None)], serde_json::json!({}));
        assert_eq!(school_in(&b, 2026), (5103, SchoolBasis::Year));
    }

    #[test]
    fn a_season_before_every_year_is_a_guess() {
        let b = bio(
            5103,
            &[
                (5103, "City High", 4, Some(2026)),
                (701, "Hilltop Middle", 2, Some(2024)),
            ],
            serde_json::json!({}),
        );
        assert_eq!(school_in(&b, 2023), (701, SchoolBasis::Guess));
        assert!(!b.school_in(2023).unwrap().in_season());
        assert_eq!(school_in(&b, 2024), (701, SchoolBasis::Year));
        assert_eq!(school_in(&b, 2025), (5103, SchoolBasis::Year));
        assert_eq!(school_in(&b, 2027), (5103, SchoolBasis::Latest));
    }

    #[test]
    fn the_seed_school_prefers_a_high_school_over_a_stale_middle_school() {
        // The Profile's own team is still the middle school; the high school
        // has the later `Year`.
        let b = bio(
            701,
            &[
                (701, "Hilltop Middle", 2, Some(2025)),
                (5103, "City High", 4, Some(2026)),
            ],
            serde_json::json!({"701_2025": 8, "5103_2026": 9}),
        );
        assert_eq!(b.school().unwrap().id, 5103);
        // Two high schools: the later `Year`, then the own one.
        let b = bio(
            10,
            &[
                (10, "Old High", 4, Some(2025)),
                (20, "New High", 4, Some(2026)),
            ],
            serde_json::json!({}),
        );
        assert_eq!(b.school().unwrap().id, 20);
        let b = bio(
            10,
            &[
                (10, "Old High", 4, Some(2026)),
                (20, "New High", 4, Some(2026)),
            ],
            serde_json::json!({}),
        );
        assert_eq!(b.school().unwrap().id, 10);
    }
}
