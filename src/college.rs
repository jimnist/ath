//! `ath colleges`: where an Athlete's Personal record in an event would rank
//! on college teams' season lists.
//!
//! A college team's season bests come from `TeamHome/GetTeamEventRecords`
//! (one row per athlete and event: that athlete's best of the season), its
//! name and level from `TeamHome/GetTeamCore`. Only the rows of the Athlete's
//! gender in the event count, and only FAT ones, as on the State lists. The
//! event is matched by name, never by id: a team's `Event` is the Athlete's
//! event name, or that name with the implement spec college lists add after
//! " - " (`100m Hurdles - 33" / 0.838m`). Rank is 1 plus the number of the
//! team's marks strictly faster (`SortInt`), and an equal mark is a tie
//! (`T2`), as on a Rankings list.

use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::area::Gender;
use crate::cache::Cacheable;
use crate::contract::{Report, columns};
use crate::mark::{
    AsOf, event_name_for, is_hand_timed, is_real_time, personal_record_as_of, resolve_event,
    strip_timing_suffix,
};
use crate::model::{athlete_url, team_url};
use crate::rank::AthleteRef;
use crate::source::DataSource;

/// A college team's `Level` on Athletic.net (`GetTeamCore`; high school is 4).
pub const COLLEGE_LEVEL: i64 = 8;

// ---------------------------------------------------------------------------
// Athletic.net's answers

/// `GET TeamHome/GetTeamEventRecords?teamId=<id>&seasonId=<year>` (only the
/// fields we use). The answer also carries `relayMembers` and `preferences`.
#[derive(Debug, Clone, Deserialize)]
pub struct TeamRecords {
    /// One row per athlete and event: the athlete's season best. Required, so
    /// an error object in place of the data is an error, never an empty team.
    #[serde(rename = "eventRecords")]
    pub event_records: Vec<TeamRecord>,
}

/// One athlete's season best in one event, on a team's records.
#[derive(Debug, Clone, Deserialize)]
pub struct TeamRecord {
    #[serde(rename = "Gender", default)]
    pub gender: Option<String>,
    /// The event's name (`400 Meters`, `100m Hurdles - 33" / 0.838m`).
    #[serde(rename = "Event", default)]
    pub event: Option<String>,
    #[serde(rename = "Type", default)]
    pub kind: Option<String>,
    /// False for a relay.
    #[serde(rename = "PersonalEvent", default)]
    pub personal_event: Option<bool>,
    /// 0 for a standard individual mark, as on a bio. A relay split (a
    /// 400 Meters leg of a 4x400) is 98, usually not FAT, its `SortInt`
    /// rounded up to the tenth: never an open mark, so it never counts.
    #[serde(rename = "EventTypeID", default)]
    pub event_type_id: Option<i64>,
    /// As Athletic.net shows it (`10.31a`).
    #[serde(rename = "Result", default)]
    pub result: Option<String>,
    /// Lower is better (running: milliseconds).
    #[serde(rename = "SortInt", default)]
    pub sort_int: Option<i64>,
    /// 1 for a fully automatic time, 0 for a hand time.
    #[serde(rename = "FAT", default)]
    pub fat: Option<serde_json::Value>,
    #[serde(rename = "IDAthlete", default)]
    pub athlete_id: Option<i64>,
    #[serde(rename = "FirstName", default)]
    pub first_name: Option<String>,
    #[serde(rename = "LastName", default)]
    pub last_name: Option<String>,
    /// Athletic.net's grade code for the athlete that season.
    #[serde(rename = "GradeID", default)]
    pub grade_id: Option<i64>,
    #[serde(rename = "MeetName", default)]
    pub meet_name: Option<String>,
    #[serde(rename = "IDMeet", default)]
    pub meet_id: Option<i64>,
    /// The meet's last day (`2026-05-16T00:00:00`).
    #[serde(rename = "EndDate", default)]
    pub end_date: Option<String>,
}

impl TeamRecord {
    /// Whether the row is a FAT time: `FAT` 1 (or true), and no `h` on the
    /// display.
    fn is_fat(&self) -> bool {
        let fat = match &self.fat {
            Some(serde_json::Value::Number(n)) => n.as_i64() == Some(1),
            Some(serde_json::Value::Bool(b)) => *b,
            _ => false,
        };
        fat && !self.result.as_deref().is_some_and(is_hand_timed)
    }

    /// The row's mark when it counts for `gender` in the event named any of
    /// `names`: the gender's, the event's (by name), an individual mark, a
    /// real time and FAT.
    fn counted(&self, gender: Gender, names: &[String]) -> Option<i64> {
        let sort_int = self.sort_int.filter(|s| *s > 0)?;
        let ok = self.gender.as_deref().and_then(Gender::from_bio) == Some(gender)
            && self.event.as_deref().is_some_and(|e| same_event(e, names))
            && self.personal_event != Some(false)
            && self.event_type_id.unwrap_or(0) == 0
            && self.result.as_deref().is_some_and(is_real_time)
            && self.is_fat();
        ok.then_some(sort_int)
    }

    fn name(&self) -> String {
        let first = self.first_name.as_deref().unwrap_or("").trim();
        let last = self.last_name.as_deref().unwrap_or("").trim();
        format!("{first} {last}").trim().to_string()
    }
}

/// `GET TeamHome/GetTeamCore?teamId=<id>&sport=tfo&year=<year>` (only the
/// fields we use).
#[derive(Debug, Clone, Deserialize)]
pub struct TeamCore {
    /// Required, so an error object in place of the data is an error.
    pub team: TeamCoreInfo,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TeamCoreInfo {
    #[serde(rename = "Name", default)]
    pub name: Option<String>,
    /// [`COLLEGE_LEVEL`] for a college team.
    #[serde(rename = "Level", default)]
    pub level: Option<i64>,
}

impl Cacheable for TeamRecords {}
impl Cacheable for TeamCore {}

/// An event name as compared: trimmed, lower case, each run of whitespace one
/// space.
fn normalized(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Whether a team's event name (`team_event`) is the event named any of
/// `names`: the same name (any case), or the same with the implement spec
/// Athletic.net's college lists add after " - " (`100m Hurdles - 33" /
/// 0.838m` is the `100m Hurdles`).
pub fn same_event(team_event: &str, names: &[String]) -> bool {
    let event = normalized(team_event);
    let base = event.split_once(" - ").map(|(b, _)| b.trim().to_string());
    names
        .iter()
        .map(|n| normalized(n))
        .any(|n| !n.is_empty() && (n == event || base.as_deref() == Some(n.as_str())))
}

// ---------------------------------------------------------------------------
// The report

/// What `ath colleges` is asked for.
#[derive(Debug, Clone)]
pub struct CollegeQuery<'a> {
    pub athlete_id: i64,
    /// The event as given: a flat race (`400m`, `400 Meters`) or an event's
    /// full name (`100m Hurdles`).
    pub event: &'a str,
    /// Athletic.net team ids, in the order to report them.
    pub teams: &'a [i64],
    /// Outdoor seasons, in the order to report them.
    pub seasons: &'a [i64],
    pub today: NaiveDate,
}

/// Whether a result has a place, and if not, why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CollegeStatus {
    Ok,
    /// The Athlete has no Personal record in the event.
    NoMark,
    /// The team's records could not be read (the note has the error).
    LookupFailed,
}

/// The mark the Athlete is placed with: their Personal record in the event
/// as of today, from their Profile (outdoor, FAT, `CONTEXT.md`).
#[derive(Debug, Clone, Serialize)]
pub struct PersonalRecord {
    /// As Athletic.net shows it, without its timing letter (`58.98`).
    pub mark: String,
    pub sort_int: i64,
    /// The outdoor season it was set in.
    pub season: i64,
    /// The day it was set (`2099-05-14`), when the Profile says.
    pub date: Option<String>,
    pub meet: Option<String>,
    pub meet_id: Option<i64>,
}

/// A college team.
#[derive(Debug, Clone, Serialize)]
pub struct TeamRef {
    pub id: i64,
    /// Athletic.net's name for it, when `GetTeamCore` could be read.
    pub name: Option<String>,
    /// Athletic.net's level ([`COLLEGE_LEVEL`] for a college team).
    pub level: Option<i64>,
}

/// A team mark the Athlete is behind or tied with.
#[derive(Debug, Clone, Serialize)]
pub struct CollegeMark {
    pub athlete_id: Option<i64>,
    pub name: String,
    /// Athletic.net's grade code for the athlete that season, as given.
    pub grade_id: Option<i64>,
    /// As Athletic.net shows it, without its timing letter.
    pub mark: String,
    pub sort_int: i64,
    pub meet: Option<String>,
    pub meet_id: Option<i64>,
    /// The meet's last day (`2099-05-16`).
    pub date: Option<String>,
    /// Their Athletic.net Profile.
    pub url: Option<String>,
}

/// Where the Athlete's mark places among a team's marks.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CollegePlace {
    /// 1 plus the number of the team's marks strictly faster.
    pub rank: usize,
    /// Whether one of the team's marks is exactly the Athlete's.
    pub tied: bool,
    /// The rank as people say it: `2`, or `T2` for a tie.
    pub label: String,
    /// The team's marks strictly faster.
    pub ahead: usize,
    /// The team's marks exactly the Athlete's.
    pub ties: usize,
}

/// The Athlete against one team in one season.
#[derive(Debug, Clone, Serialize)]
pub struct CollegeResult {
    pub team: TeamRef,
    pub season: i64,
    /// The team's Athletic.net page for the season.
    pub url: String,
    pub status: CollegeStatus,
    /// The team's counted marks in the event (FAT, the Athlete's gender); 0
    /// when it has none, or when it could not be read.
    pub marks: usize,
    /// `null` unless `status` is `ok`.
    pub place: Option<CollegePlace>,
    /// The team's marks faster than the Athlete's, best first.
    pub ahead: Vec<CollegeMark>,
    /// The team's marks exactly the Athlete's.
    pub tied: Vec<CollegeMark>,
    /// The team's marks slower than the Athlete's, best first, so a reader
    /// has the whole list (rankmap merges hand-entered marks into it).
    pub behind: Vec<CollegeMark>,
    pub note: Option<String>,
}

/// An Athlete against college teams, as `ath colleges` prints it.
#[derive(Debug, Clone, Serialize)]
pub struct Colleges {
    pub athlete: AthleteRef,
    pub gender: Gender,
    /// The event as asked for (`--event`).
    pub event: String,
    /// The Profile's name for it (`400 Meters`), when the Profile has it.
    pub event_name: Option<String>,
    /// `ok`, or `no_mark` when the Athlete has no Personal record in it.
    pub status: CollegeStatus,
    /// `null` when `status` is `no_mark`.
    pub mark: Option<PersonalRecord>,
    /// The seasons asked for, in order.
    pub seasons: Vec<i64>,
    pub note: Option<String>,
    /// Team by team (in `--teams` order), season by season.
    pub results: Vec<CollegeResult>,
}

impl Report for Colleges {
    const SCHEMA: u32 = 2;

    fn table(&self) -> String {
        let mut out = match &self.mark {
            Some(m) => format!(
                "{} {} {} (Personal record, {}), against college season bests (FAT)\n\n",
                self.athlete.name,
                self.event_name.as_deref().unwrap_or(&self.event),
                m.mark,
                m.season
            ),
            None => format!(
                "{} has no {} Personal record\n\n",
                self.athlete.name,
                self.event_name.as_deref().unwrap_or(&self.event)
            ),
        };
        let mut rows = vec![vec![
            "Team".to_string(),
            "Season".to_string(),
            "Rank".to_string(),
            "Marks".to_string(),
            "Fastest".to_string(),
        ]];
        for r in &self.results {
            let name = r
                .team
                .name
                .clone()
                .unwrap_or_else(|| format!("team {}", r.team.id));
            let rank = match (&r.status, &r.place) {
                (CollegeStatus::LookupFailed, _) => "not read".to_string(),
                _ if r.marks == 0 => "no marks".to_string(),
                (_, Some(p)) if p.tied => p.label.clone(),
                (_, Some(p)) => format!("#{}", p.label),
                (_, None) => "-".to_string(),
            };
            let fastest = r
                .ahead
                .first()
                .map(|a| format!("{} {}", a.mark, a.name))
                .unwrap_or_default();
            rows.push(vec![
                name,
                r.season.to_string(),
                rank,
                r.marks.to_string(),
                fastest,
            ]);
        }
        out.push_str(&columns(&rows));
        let notes = self
            .note
            .iter()
            .chain(self.results.iter().filter_map(|r| r.note.as_ref()));
        for note in notes {
            out.push_str(&format!("\nNote: {note}"));
        }
        out
    }
}

/// The team's Records by Event page for an outdoor season: each athlete's
/// season best per event, the list the ranks are read from.
pub fn team_season_url(team_id: i64, season: i64) -> String {
    format!("{}/{season}/event-records", team_url(team_id))
}

/// A display without its timing letter (`10.31a` is `10.31`).
fn shown(result: &str) -> String {
    strip_timing_suffix(result).trim().to_string()
}

/// The day of an Athletic.net timestamp (`2026-05-16T00:00:00`).
fn day(raw: Option<&str>) -> Option<String> {
    let d = raw?.trim().get(..10)?;
    NaiveDate::parse_from_str(d, "%Y-%m-%d")
        .ok()
        .map(|d| d.to_string())
}

/// Place `mark` among a team's rows: the counted ones (`names`, `gender`),
/// leaving out the Athlete's own. Returns the counted marks, the place, and
/// the marks ahead, tied and behind.
pub fn place_on_team(
    records: &TeamRecords,
    athlete_id: i64,
    gender: Gender,
    names: &[String],
    mark: Option<i64>,
) -> (
    usize,
    Option<CollegePlace>,
    Vec<CollegeMark>,
    Vec<CollegeMark>,
    Vec<CollegeMark>,
) {
    let mut counted: Vec<(i64, &TeamRecord)> = records
        .event_records
        .iter()
        .filter(|r| r.athlete_id != Some(athlete_id))
        .filter_map(|r| r.counted(gender, names).map(|s| (s, r)))
        .collect();
    counted.sort_by(|(a, ra), (b, rb)| a.cmp(b).then_with(|| ra.name().cmp(&rb.name())));
    let Some(mark) = mark else {
        return (counted.len(), None, Vec::new(), Vec::new(), Vec::new());
    };
    let row = |&(sort_int, r): &(i64, &TeamRecord)| CollegeMark {
        athlete_id: r.athlete_id,
        name: r.name(),
        grade_id: r.grade_id,
        mark: shown(r.result.as_deref().unwrap_or("")),
        sort_int,
        meet: r.meet_name.as_deref().map(|m| m.trim().to_string()),
        meet_id: r.meet_id,
        date: day(r.end_date.as_deref()),
        url: r.athlete_id.map(athlete_url),
    };
    let ahead: Vec<CollegeMark> = counted.iter().filter(|(s, _)| *s < mark).map(row).collect();
    let tied: Vec<CollegeMark> = counted
        .iter()
        .filter(|(s, _)| *s == mark)
        .map(row)
        .collect();
    let behind: Vec<CollegeMark> = counted.iter().filter(|(s, _)| *s > mark).map(row).collect();
    let rank = ahead.len() + 1;
    let place = CollegePlace {
        rank,
        tied: !tied.is_empty(),
        label: if tied.is_empty() { rank.to_string() } else { format!("T{rank}") },
        ahead: ahead.len(),
        ties: tied.len(),
    };
    (counted.len(), Some(place), ahead, tied, behind)
}

/// `ath colleges`: the Athlete's Personal record in the event against each
/// team's season bests, season by season.
pub async fn colleges<S: DataSource + ?Sized>(
    source: &S,
    q: &CollegeQuery<'_>,
) -> Result<Colleges> {
    if q.teams.is_empty() {
        bail!("--teams names no team");
    }
    if q.seasons.is_empty() {
        bail!("--seasons names no season");
    }
    let bio = source.get_bio(q.athlete_id).await?;
    let name = bio.athlete.full_name();
    let gender = bio
        .athlete
        .gender
        .as_deref()
        .and_then(Gender::from_bio)
        .with_context(|| {
            format!("{name}'s Profile does not say whether to compare against women or men")
        })?;
    let mut notes = Vec::new();
    let event = resolve_event(&bio, q.event);
    if let Some(e) = &event
        && e.field
    {
        bail!(
            "{} is a field event; ath compares running events only, since it reads no field \
             Personal records",
            e.name
        );
    }
    let mut names = vec![event_name_for(q.event)];
    names.extend(event.as_ref().map(|e| e.name.clone()));
    let as_of = AsOf::today(q.today);
    let pr = event
        .as_ref()
        .and_then(|e| personal_record_as_of(&bio, &e.ids, as_of))
        .map(|r| PersonalRecord {
            mark: shown(&r.result),
            sort_int: r.sort_int,
            season: r.season_id,
            date: r.date().map(|d| d.to_string()),
            meet: bio.meet_name(r).map(|m| m.trim().to_string()),
            meet_id: r.meet_id,
        });
    if pr.is_none() {
        notes.push(format!(
            "{name} has no {} Personal record ({})",
            event.as_ref().map_or(q.event, |e| e.name.as_str()),
            crate::mark::NEVER_COUNT
        ));
    }
    let newest = *q.seasons.iter().max().expect("seasons is not empty");
    let mut results = Vec::new();
    for &team_id in q.teams {
        // The team's name and level: a note, never a failure, when unread.
        let (team, team_note) = match source.team_core(team_id, newest).await {
            Ok(core) => {
                let level = core.team.level;
                let shown_name = core
                    .team
                    .name
                    .as_deref()
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .map(str::to_string);
                let note = level.filter(|l| *l != COLLEGE_LEVEL).map(|l| {
                    format!(
                        "team {team_id} ({}) is level {l} on Athletic.net, not a college team \
                         ({COLLEGE_LEVEL})",
                        shown_name.as_deref().unwrap_or("no name")
                    )
                });
                (
                    TeamRef {
                        id: team_id,
                        name: shown_name,
                        level,
                    },
                    note,
                )
            }
            Err(e) => (
                TeamRef {
                    id: team_id,
                    name: None,
                    level: None,
                },
                Some(format!("could not read team {team_id}'s name: {e:#}")),
            ),
        };
        notes.extend(team_note);
        for &season in q.seasons {
            let mut result = CollegeResult {
                team: team.clone(),
                season,
                url: team_season_url(team_id, season),
                status: CollegeStatus::NoMark,
                marks: 0,
                place: None,
                ahead: Vec::new(),
                tied: Vec::new(),
                behind: Vec::new(),
                note: None,
            };
            match source.team_records(team_id, season).await {
                Ok(records) => {
                    let (marks, place, ahead, tied, behind) = place_on_team(
                        &records,
                        q.athlete_id,
                        gender,
                        &names,
                        pr.as_ref().map(|m| m.sort_int),
                    );
                    result.marks = marks;
                    result.status =
                        if place.is_some() { CollegeStatus::Ok } else { CollegeStatus::NoMark };
                    result.place = place;
                    result.ahead = ahead;
                    result.tied = tied;
                    result.behind = behind;
                }
                Err(e) => {
                    result.status = CollegeStatus::LookupFailed;
                    result.note = Some(format!("{e:#}"));
                }
            }
            results.push(result);
        }
    }
    Ok(Colleges {
        athlete: AthleteRef {
            id: bio.athlete.id,
            name,
            first_name: bio.athlete.first_name.trim().to_string(),
            url: athlete_url(bio.athlete.id),
        },
        gender,
        event: q.event.trim().to_string(),
        event_name: event.map(|e| e.name),
        status: if pr.is_some() { CollegeStatus::Ok } else { CollegeStatus::NoMark },
        mark: pr,
        seasons: q.seasons.to_vec(),
        note: (!notes.is_empty()).then(|| notes.join("; ")),
        results,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A team's records from `(gender, event, result, sort_int, fat, athlete
    /// id, last name)` rows.
    fn records(rows: &[(&str, &str, &str, i64, i64, i64, &str)]) -> TeamRecords {
        let rows: Vec<_> = rows
            .iter()
            .map(|&(g, event, result, sort_int, fat, id, last)| {
                serde_json::json!({
                    "Gender": g, "Event": event, "Type": "T", "PersonalEvent": true,
                    "EventTypeID": 0, "Result": result, "SortInt": sort_int, "FAT": fat,
                    "IDAthlete": id, "FirstName": "Pat", "LastName": last, "GradeID": 22,
                    "MeetName": " Lakeshore Invitational ", "IDMeet": 7,
                    "EndDate": "2099-04-12T00:00:00",
                })
            })
            .collect();
        serde_json::from_value(serde_json::json!({"eventRecords": rows, "relayMembers": []}))
            .unwrap()
    }

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn event_names_match_with_or_without_the_implement_spec() {
        let four = names(&["400 meters", "400 Meters"]);
        assert!(same_event("400 Meters", &four));
        assert!(same_event("  400   meters ", &four));
        assert!(!same_event("4000 Meters", &four));
        assert!(!same_event("400m Hurdles - 30\" / 0.762m", &four));
        let hurdles = names(&["100m Hurdles"]);
        assert!(same_event("100m Hurdles - 33\" / 0.838m", &hurdles));
        assert!(same_event("100m Hurdles", &hurdles));
        assert!(!same_event("100 Meters", &hurdles));
        // A spec on the asked-for name has to be the team's spec.
        let spec = names(&["110m Hurdles - 39\""]);
        assert!(!same_event("110m Hurdles - 42\" / 1.067m", &spec));
        assert!(!same_event("", &names(&[""])));
    }

    #[test]
    fn a_relay_split_never_counts() {
        // Seen live: a 4x400 leg on the team's 400 Meters list, type 98,
        // not FAT, timed to the thousandth and its SortInt rounded up.
        let mut r = records(&[
            ("F", "400 Meters", "51.455", 51550, 0, 21, "Hollin"),
            ("F", "400 Meters", "52.40a", 52400, 1, 22, "Ivers"),
            ("F", "400 Meters", "53.10a", 53100, 1, 23, "Jessop"),
        ]);
        r.event_records[0].event_type_id = Some(98);
        // Even a FAT split.
        r.event_records[2].event_type_id = Some(98);
        let ev = names(&["400 Meters"]);
        let (marks, place, ahead, _, _) = place_on_team(&r, 1, Gender::F, &ev, Some(55_000));
        assert_eq!(marks, 1);
        assert_eq!(place.unwrap().label, "2");
        assert_eq!(
            ahead.iter().map(|a| a.mark.as_str()).collect::<Vec<_>>(),
            ["52.40"]
        );
    }

    #[test]
    fn rank_counts_faster_fat_marks_of_the_gender_and_event() {
        let r = records(&[
            ("F", "400 Meters", "52.10a", 52100, 1, 11, "Ashgrove"),
            ("F", "400 Meters", "54.00a", 54000, 1, 12, "Birchley"),
            // A hand time, even a fast one, never counts.
            ("F", "400 Meters", "51.0h", 51000, 0, 13, "Cotter"),
            ("F", "400 Meters", "51.20a", 51200, 0, 14, "Dunmore"),
            // The men's, and another event.
            ("M", "400 Meters", "47.00a", 47000, 1, 15, "Elwood"),
            ("F", "800 Meters", "2:05.00a", 125_000, 1, 16, "Fenwick"),
            // Slower.
            ("F", "400 Meters", "57.50a", 57500, 1, 17, "Garrow"),
        ]);
        let ev = names(&["400 meters"]);
        let (marks, place, ahead, tied, behind) =
            place_on_team(&r, 1, Gender::F, &ev, Some(55_000));
        assert_eq!(marks, 3);
        let place = place.unwrap();
        assert_eq!(
            (place.rank, place.tied, place.label.as_str()),
            (3, false, "3")
        );
        assert_eq!(
            ahead.iter().map(|a| a.mark.as_str()).collect::<Vec<_>>(),
            ["52.10", "54.00"]
        );
        assert!(tied.is_empty());
        assert_eq!(
            behind.iter().map(|a| a.mark.as_str()).collect::<Vec<_>>(),
            ["57.50"]
        );
        let a = &ahead[0];
        assert_eq!(a.name, "Pat Ashgrove");
        assert_eq!(a.meet.as_deref(), Some("Lakeshore Invitational"));
        assert_eq!(a.date.as_deref(), Some("2099-04-12"));
        assert_eq!(a.grade_id, Some(22));
        assert_eq!(
            a.url.as_deref(),
            Some("https://www.athletic.net/athlete/11/track-and-field")
        );
    }

    #[test]
    fn an_equal_mark_shares_the_rank() {
        let r = records(&[
            ("F", "400 Meters", "52.10a", 52100, 1, 11, "Ashgrove"),
            ("F", "400 Meters", "55.00a", 55000, 1, 12, "Birchley"),
            ("F", "400 Meters", "55.00a", 55000, 1, 13, "Cotter"),
        ]);
        let (marks, place, ahead, tied, behind) =
            place_on_team(&r, 1, Gender::F, &names(&["400 meters"]), Some(55_000));
        let place = place.unwrap();
        assert_eq!(marks, 3);
        assert_eq!((place.rank, place.label.as_str(), place.ties), (2, "T2", 2));
        assert_eq!(ahead.len(), 1);
        assert_eq!(tied.len(), 2);
        assert!(behind.is_empty());
    }

    #[test]
    fn a_team_without_marks_places_first_of_none_and_the_athlete_is_left_out() {
        let r = records(&[("F", "400 Meters", "52.10a", 52100, 1, 1, "Self")]);
        let (marks, place, ahead, _, _) =
            place_on_team(&r, 1, Gender::F, &names(&["400 meters"]), Some(55_000));
        assert_eq!(marks, 0);
        assert_eq!(place.unwrap().rank, 1);
        assert!(ahead.is_empty());
        // Without a mark, the count alone.
        let (marks, place, _, _, _) =
            place_on_team(&r, 2, Gender::F, &names(&["400 meters"]), None);
        assert_eq!((marks, place), (1, None));
    }

    #[test]
    fn an_error_object_is_not_a_team() {
        assert!(
            serde_json::from_str::<TeamRecords>(r#"{"Message": "An error has occurred."}"#)
                .is_err()
        );
        assert!(
            serde_json::from_str::<TeamCore>(r#"{"Message": "An error has occurred."}"#).is_err()
        );
        let core: TeamCore =
            serde_json::from_str(r#"{"team": {"Name": "Lakeshore", "Level": 8, "Extra": 1}}"#)
                .unwrap();
        assert_eq!(core.team.name.as_deref(), Some("Lakeshore"));
        assert_eq!(core.team.level, Some(COLLEGE_LEVEL));
    }
}
