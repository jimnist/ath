//! Seeding a relay (`docs/HANDOFF.md`, Relays): one row per team, seeded on
//! its aggregate, the sum of its four runners' Personal records in the
//! Seeding event (`CONTEXT.md`).
//!
//! The runners are a meet's relay entries (Mode A, [`crate::meet::RelayTeam`]):
//! for each of legs 1 to 4, the first entry with that leg number. Another
//! entry for a leg already filled, or one with no leg number (an alternate),
//! never displaces it, and a leg with no entry leaves the team a runner
//! short. Every runner is seeded exactly
//! as an individual Entrant is ([`crate::seed::seed`]): their Personal
//! record in the Seeding event, outdoor Qualifying marks only (relay splits,
//! `EventTypeID` 98, never count), as of the same date. Sums are of
//! `SortInt`, never of display strings.
//!
//! A team with fewer than four runners, or with a runner who has no mark in
//! the Seeding event (or whose Profile could not be read), has no aggregate
//! and goes to the bottom, in entry order, still showing every runner and
//! whichever marks they have.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use chrono::NaiveDate;
use serde::Serialize;

use crate::contract::{Report, columns};
use crate::mark::{format_running, running_seconds_from_sortint};
use crate::meet::{RELAY_LEGS, RelayLeg, RelayTeam, left_out_line};
use crate::model::{SeedRow, team_url};
use crate::roster::RosterRow;
use crate::seed::{
    FetchFailure, FetchOpts, NotResolved, SeedDate, SheetMeet, Skipped, seed_events,
};
use crate::source::DataSource;

/// One runner of a seeded team, with their Personal record in the Seeding
/// event.
#[derive(Debug, Clone, Serialize)]
pub struct SeededLeg {
    /// The leg, 1 to 4 (`A3` is leg 3).
    pub leg: u32,
    /// The runner's name as the meet's entries have it.
    pub athlete: String,
    pub athlete_id: i64,
    pub athlete_url: String,
    pub grade: Option<i64>,
    /// The Personal record in the Seeding event, as displayed (`1:59.10`);
    /// empty with no Qualifying mark.
    pub pr: String,
    /// Its numeric key in seconds; `null` with no mark.
    pub pr_numeric: Option<f64>,
    /// The season it was set in; `null` with no mark.
    pub pr_season: Option<i64>,
    /// Its `SortInt` (milliseconds); `null` with no mark.
    pub sort_int: Option<i64>,
    /// True when the mark is an Estimated mark (`CONTEXT.md`): on a 4xMile,
    /// the runner's mile converted to the 1600m because it was the faster.
    pub estimated: bool,
    /// What an estimated mark was made from (`1 Mile`, as the Profile names
    /// it); `null` for a real mark.
    pub estimated_from: Option<String>,
    /// The mark it was made from, as displayed (`5:02.10`); `null` for a
    /// real mark.
    pub estimated_from_pr: Option<String>,
    /// That mark's `SortInt`; `null` for a real mark.
    pub estimated_from_sort_int: Option<i64>,
}

/// The event a 4xMile runner's mile mark is read from.
pub const MILE_EVENT: &str = "1 Mile";

/// A mile in meters, for converting a mile time to the 1600m.
const MILE_METERS: f64 = 1609.344;

/// A mile `SortInt` (milliseconds) as a 1600m Estimated mark, at even pace:
/// `mile x 1600 / 1609.344`, rounded up to the next hundredth (10 ms), so an
/// estimate never looks faster than the conversion and the stored `SortInt`
/// is exactly the mark displayed (an aggregate is then the sum of the legs
/// shown).
pub fn mile_to_1600m(mile: i64) -> i64 {
    let exact = mile as f64 * 1600.0 / MILE_METERS;
    ((exact / 10.0).ceil() as i64) * 10
}

/// One relay team on the seed sheet.
#[derive(Debug, Clone, Serialize)]
pub struct SeededTeam {
    /// 1 to n in seed order, teams without an aggregate last.
    pub rank: usize,
    /// The school (`Pinecrest Academy`).
    pub team: String,
    /// `A`, or `B` for a school's second team.
    pub letter: String,
    /// The school's page; empty when the entries name no school id.
    pub team_url: String,
    /// The sum of the four runners' Personal records (`7:58.46`); empty
    /// when a runner is missing or has no mark.
    pub aggregate: String,
    /// The aggregate in seconds; `null` without one.
    pub aggregate_seconds: Option<f64>,
    /// The aggregate's `SortInt` (the sum of the legs' `SortInt`s, in
    /// milliseconds): the key the sheet is sorted on; `null` without one.
    pub sort_int: Option<i64>,
    /// The relay team's own id (`TeamID`).
    pub team_id: Option<i64>,
    pub school_id: Option<i64>,
    /// The seed the team was entered with, as its `SortInt`.
    pub entered_sort_int: Option<i64>,
    /// The legs (1 to 4) with no runner entered, in order.
    pub missing_legs: Vec<u32>,
    /// Legs run by the same athlete, each group in leg order (`[[1, 3]]`): a
    /// runner can run one leg, so such a team has no aggregate.
    pub same_runner_legs: Vec<Vec<u32>>,
    /// Why the team has no aggregate, in words, leg by leg number ("Leg 4
    /// has no 800 Meters mark; Same runner on legs 1 and 3"); `null` with
    /// an aggregate. The CSV and the page show it as is.
    pub no_aggregate_reason: Option<String>,
    /// True when any runner's mark is an Estimated mark, so the aggregate
    /// includes estimated legs.
    pub estimated: bool,
    /// The runners of its legs that have one, in leg order.
    pub legs: Vec<SeededLeg>,
}

impl SeededTeam {
    /// Legs 1 to 4 in order, each with its runner or `None` when it has
    /// none.
    pub fn slots(&self) -> impl Iterator<Item = (u32, Option<&SeededLeg>)> {
        (1..=RELAY_LEGS).map(|n| (n, self.legs.iter().find(|l| l.leg == n)))
    }
}

/// Legs run by the same athlete, each group in leg order.
fn same_runner_legs(legs: &[SeededLeg]) -> Vec<Vec<u32>> {
    let mut groups: Vec<Vec<u32>> = Vec::new();
    for (i, l) in legs.iter().enumerate() {
        if legs[..i].iter().any(|p| p.athlete_id == l.athlete_id) {
            continue; // already in the group of its first leg
        }
        let group: Vec<u32> = legs[i..]
            .iter()
            .filter(|o| o.athlete_id == l.athlete_id)
            .map(|o| o.leg)
            .collect();
        if group.len() > 1 {
            groups.push(group);
        }
    }
    groups
}

/// `Leg 4 has ...`, `Legs 2, 4 have ...`.
fn legs_phrase(legs: &[u32], one: &str, many: &str) -> String {
    match legs {
        [n] => format!("Leg {n} {one}"),
        _ => format!(
            "Legs {} {many}",
            legs.iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// `1 and 3`, `1, 2 and 4`.
fn and_list(legs: &[u32]) -> String {
    let words: Vec<String> = legs.iter().map(u32::to_string).collect();
    match words.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => words.join(""),
    }
}

/// Why a team has no aggregate, leg by leg number: runners with no mark in
/// the Seeding event (`event`), runners whose Profile could not be read
/// (`unread`), legs with no runner, and one runner on several legs.
fn why_no_aggregate(
    legs: &[SeededLeg],
    missing: &[u32],
    same: &[Vec<u32>],
    unread: &HashSet<i64>,
    event: &str,
) -> String {
    let no_mark = |read: bool| -> Vec<u32> {
        legs.iter()
            .filter(|l| l.sort_int.is_none() && unread.contains(&l.athlete_id) != read)
            .map(|l| l.leg)
            .collect()
    };
    let mut parts = Vec::new();
    let (marks, not_read) = (no_mark(true), no_mark(false));
    if !marks.is_empty() {
        parts.push(legs_phrase(
            &marks,
            &format!("has no {event} mark"),
            &format!("have no {event} mark"),
        ));
    }
    if !not_read.is_empty() {
        parts.push(legs_phrase(
            &not_read,
            "has a profile that could not be read",
            "have profiles that could not be read",
        ));
    }
    if !missing.is_empty() {
        parts.push(legs_phrase(missing, "has no runner", "have no runner"));
    }
    parts.extend(
        same.iter()
            .map(|g| format!("Same runner on legs {}", and_list(g))),
    );
    parts.join("; ")
}

/// A team's runners: for each of legs 1 to 4, the first entry with that leg
/// number, in leg order. Alternates (another entry for a filled leg, or one
/// with no leg number) are left out.
fn runners(team: &RelayTeam) -> impl Iterator<Item = (u32, &RelayLeg)> {
    (1..=RELAY_LEGS).filter_map(|n| team.legs.iter().find(|l| l.leg == Some(n)).map(|l| (n, l)))
}

/// The result of seeding a relay.
#[derive(Debug, Clone, Default)]
pub struct RelayOutcome {
    pub teams: Vec<SeededTeam>,
    /// How many runners were seeded (each once, however many teams).
    pub runners: usize,
    /// Runners whose Profile could not be read: on their team without a
    /// mark, and listed here.
    pub failures: Vec<FetchFailure>,
    /// The Seeding event's own name (`800 Meters`) as the first runner's
    /// Profile with it spells it.
    pub seeding_event_name: Option<String>,
    /// The event an Estimated mark may be made from (`1 Mile`, on a 4xMile);
    /// `None` when every mark is a real one in the Seeding event.
    pub estimate_from: Option<String>,
}

/// Seed `teams` on `seeding_event` (`800m`, `1600 Meters`): read every
/// runner's Profile once ([`seed_events`]), sum each full team's Personal
/// records, and sort the teams fastest first on that `SortInt`, teams
/// without an aggregate last in entry order, ranked 1 to n.
///
/// With `mile` (a 4xMile seeded on the 1600m), each runner's mark is the
/// faster of their 1600m Personal record and their 1 Mile Personal record
/// converted to the 1600m ([`mile_to_1600m`]), both read by the same rules;
/// a converted mile is flagged as an Estimated mark, and a tie goes to the
/// real 1600m.
pub async fn seed_relay<S: DataSource + ?Sized>(
    source: &S,
    teams: &[RelayTeam],
    seeding_event: &str,
    mile: bool,
    date: SeedDate,
    opts: &FetchOpts,
) -> Result<RelayOutcome> {
    // Every runner once, in team and leg order.
    let mut seen = HashSet::new();
    let roster: Vec<RosterRow> = teams
        .iter()
        .flat_map(runners)
        .map(|(_, l)| l)
        .filter(|l| seen.insert(l.athlete_id))
        .map(|l| RosterRow {
            name: l.name.clone(),
            athlete_id: Some(l.athlete_id),
            school: None,
            city: None,
            state: None,
        })
        .collect();
    let events: &[&str] = if mile { &[seeding_event, MILE_EVENT] } else { &[seeding_event] };
    let mut outcomes = seed_events(source, &roster, events, date, opts)
        .await?
        .into_iter();
    let outcome = outcomes.next().expect("one outcome per event");
    let miles = outcomes.next();
    let marked = |rows: &[SeedRow]| -> HashMap<i64, SeedRow> {
        rows.iter()
            .filter(|r| r.sort_int.is_some())
            .map(|r| (r.athlete_id, r.clone()))
            .collect()
    };
    let marks = marked(&outcome.rows);
    let unread: HashSet<i64> = outcome.failures.iter().map(|f| f.athlete_id).collect();
    let event_label = outcome
        .event_name
        .clone()
        .unwrap_or_else(|| seeding_event.to_string());
    let mile_marks = miles.as_ref().map(|m| marked(&m.rows)).unwrap_or_default();
    let mile_name = miles
        .as_ref()
        .and_then(|m| m.event_name.clone())
        .unwrap_or_else(|| MILE_EVENT.to_string());

    let mut seeded: Vec<SeededTeam> = teams
        .iter()
        .map(|t| {
            let legs: Vec<SeededLeg> = runners(t)
                .map(|(n, l)| {
                    let real = marks.get(&l.athlete_id);
                    let estimate = mile_marks.get(&l.athlete_id).and_then(|m| {
                        let converted = mile_to_1600m(m.sort_int?);
                        let faster = real.and_then(|r| r.sort_int).is_none_or(|r| converted < r);
                        faster.then_some((m, converted))
                    });
                    let mut leg = SeededLeg {
                        leg: n,
                        athlete: l.name.clone(),
                        athlete_id: l.athlete_id,
                        athlete_url: l.athlete_url.clone(),
                        grade: l.grade,
                        pr: real.map(|r| r.pr.clone()).unwrap_or_default(),
                        pr_numeric: real.and_then(|r| r.pr_numeric),
                        pr_season: real.and_then(|r| r.pr_season),
                        sort_int: real.and_then(|r| r.sort_int),
                        estimated: false,
                        estimated_from: None,
                        estimated_from_pr: None,
                        estimated_from_sort_int: None,
                    };
                    if let Some((m, converted)) = estimate {
                        leg.pr = format_running(converted);
                        leg.pr_numeric = Some(running_seconds_from_sortint(converted));
                        leg.pr_season = m.pr_season;
                        leg.sort_int = Some(converted);
                        leg.estimated = true;
                        leg.estimated_from = Some(mile_name.clone());
                        leg.estimated_from_pr = Some(m.pr.clone());
                        leg.estimated_from_sort_int = m.sort_int;
                    }
                    leg
                })
                .collect();
            let estimated = legs.iter().any(|l| l.estimated);
            let missing_legs: Vec<u32> = (1..=RELAY_LEGS)
                .filter(|n| !legs.iter().any(|l| l.leg == *n))
                .collect();
            let same_runner_legs = same_runner_legs(&legs);
            let sort_int = if missing_legs.is_empty() && same_runner_legs.is_empty() {
                legs.iter().map(|l| l.sort_int).sum::<Option<i64>>()
            } else {
                None
            };
            let no_aggregate_reason = sort_int.is_none().then(|| {
                why_no_aggregate(
                    &legs,
                    &missing_legs,
                    &same_runner_legs,
                    &unread,
                    &event_label,
                )
            });
            SeededTeam {
                rank: 0, // assigned after sorting
                team: t.school.clone(),
                letter: t.letter.clone(),
                team_url: t.school_id.map(team_url).unwrap_or_default(),
                aggregate: sort_int.map(format_running).unwrap_or_default(),
                aggregate_seconds: sort_int.map(running_seconds_from_sortint),
                sort_int,
                team_id: t.team_id,
                school_id: t.school_id,
                entered_sort_int: t.entered_sort_int,
                missing_legs,
                same_runner_legs,
                no_aggregate_reason,
                estimated,
                legs,
            }
        })
        .collect();
    // Aggregates first, ascending by SortInt; the rest last (stable order).
    seeded.sort_by_key(|t| (t.sort_int.is_none(), t.sort_int));
    for (i, t) in seeded.iter_mut().enumerate() {
        t.rank = i + 1;
    }
    Ok(RelayOutcome {
        teams: seeded,
        runners: roster.len(),
        failures: outcome.failures,
        seeding_event_name: outcome.event_name,
        estimate_from: mile.then_some(mile_name),
    })
}

/// A relay seed sheet as `ath seed --meet` prints it (the output contract,
/// ADR 0007): the teams in seed order, as in the relay CSV, each with its
/// runners and their marks, plus the meet and event they came from.
///
/// It is `ath seed`'s schema 1, like the individual sheet
/// ([`crate::seed::SeedSheet`]), told apart by `kind: "relay"` (an
/// individual sheet has no `kind`): the same `event`, `event_name`, `as_of`,
/// `meet_date`, `meet`, `skipped` and `unresolved`, with `teams` (and the
/// Seeding event) where an individual sheet has `entrants`.
#[derive(Debug, Clone, Serialize)]
pub struct RelaySheet {
    /// Always `relay`: what a reader dispatches on.
    pub kind: &'static str,
    /// The event as given (`--event`: `4x800m`).
    pub event: String,
    /// The event's name as the meet has it (`4x800 Relay`).
    pub event_name: String,
    /// The event each runner is seeded on, as mapped from the relay
    /// (`800m`) or given with `--seed-on`.
    pub seeding_event: String,
    /// The Seeding event's own name (`800 Meters`), when any runner's
    /// Profile has it.
    pub seeding_event_name: Option<String>,
    /// On a 4xMile, the event a runner's Estimated mark may be made from
    /// (`1 Mile`, converted to the 1600m when faster); `null` otherwise.
    pub estimate_from: Option<String>,
    /// The last day whose marks count.
    pub as_of: NaiveDate,
    /// The meet's first day.
    pub meet_date: Option<NaiveDate>,
    pub meet: SheetMeet,
    /// Every team in seed order, as in the CSV: aggregates fastest first,
    /// then the teams without one.
    pub teams: Vec<SeededTeam>,
    /// Runners whose Profile could not be read (on their team, without a
    /// mark).
    pub skipped: Vec<Skipped>,
    /// Always empty: entries carry athlete ids, so nothing is resolved by
    /// name. Kept so every `ath seed` sheet has it.
    pub unresolved: Vec<NotResolved>,
}

impl RelaySheet {
    pub fn new(
        event: &str,
        seeding_event: &str,
        date: SeedDate,
        outcome: &RelayOutcome,
        meet: SheetMeet,
    ) -> Self {
        Self {
            kind: RELAY_KIND,
            event: event.to_string(),
            event_name: meet.event.event.clone(),
            seeding_event: seeding_event.to_string(),
            seeding_event_name: outcome.seeding_event_name.clone(),
            estimate_from: outcome.estimate_from.clone(),
            as_of: date.as_of(),
            meet_date: date.meet_date(),
            meet,
            teams: outcome.teams.clone(),
            skipped: outcome.failures.iter().map(Skipped::from).collect(),
            unresolved: Vec::new(),
        }
    }

    /// The Seeding event as Profiles name it, else as given.
    pub fn seeding_event_label(&self) -> &str {
        self.seeding_event_name
            .as_deref()
            .unwrap_or(&self.seeding_event)
    }
}

/// A relay sheet's `kind`.
pub const RELAY_KIND: &str = "relay";

/// `n teams`, `1 team`.
fn teams(n: usize) -> String {
    format!("{n} team{}", if n == 1 { "" } else { "s" })
}

impl Report for RelaySheet {
    const SCHEMA: u32 = 1;

    fn table(&self) -> String {
        let m = &self.meet;
        let with = self.teams.iter().filter(|t| t.sort_int.is_some()).count();
        let mut out = format!(
            "{} seed sheet: the sum of each team's {} Personal records{} as of {}, for {} \
             (meet {}){}, {} {}\n{}, {with} with an aggregate\n",
            self.event_name,
            self.seeding_event_label(),
            self.estimate_from
                .as_ref()
                .map_or_else(String::new, |from| format!(
                    " (or their {from}, converted to the {} as an Estimated mark when faster)",
                    self.seeding_event_label()
                )),
            self.as_of,
            m.meet.name,
            m.meet.id,
            m.meet.date.map_or_else(String::new, |d| format!(" on {d}")),
            m.event.gender.noun(),
            m.event.division,
            teams(self.teams.len())
        );
        if !self.teams.is_empty() {
            let mut rows = vec![
                [
                    "Seed",
                    "Team",
                    "Aggregate",
                    "Leg",
                    "Athlete",
                    "Grade",
                    "PR",
                    "Season",
                    "Profile",
                ]
                .map(String::from)
                .to_vec(),
            ];
            for t in &self.teams {
                let aggregate = if t.aggregate.is_empty() {
                    "no aggregate".to_string()
                } else if t.estimated {
                    format!("{} (estimated legs)", t.aggregate)
                } else {
                    t.aggregate.clone()
                };
                for (n, leg) in t.slots() {
                    let first = n == 1;
                    let mut row = vec![
                        if first { t.rank.to_string() } else { String::new() },
                        if first { format!("{} {}", t.team, t.letter) } else { String::new() },
                        if first { aggregate.clone() } else { String::new() },
                        n.to_string(),
                    ];
                    match leg {
                        Some(l) => row.extend([
                            l.athlete.clone(),
                            l.grade.map_or_else(String::new, |g| g.to_string()),
                            match (&l.estimated_from, &l.estimated_from_pr) {
                                _ if l.pr.is_empty() => "no mark".to_string(),
                                (Some(event), Some(pr)) => {
                                    format!("{} (estimated from {event} {pr})", l.pr)
                                }
                                _ => l.pr.clone(),
                            },
                            l.pr_season.map_or_else(String::new, |s| s.to_string()),
                            l.athlete_url.clone(),
                        ]),
                        None => row.push("(no runner)".to_string()),
                    }
                    rows.push(row);
                }
            }
            out.push('\n');
            out.push_str(&columns(&rows));
            let why: Vec<String> = self
                .teams
                .iter()
                .filter_map(|t| {
                    let why = t.no_aggregate_reason.as_ref()?;
                    Some(format!("{} {}: {why}\n", t.team, t.letter))
                })
                .collect();
            if !why.is_empty() {
                out.push_str("\nNo aggregate:\n");
                out.push_str(&why.concat());
            }
        }
        if !m.left_out.is_empty() {
            out.push_str("\nNot on the sheet:\n");
        }
        for l in &m.left_out {
            out.push_str(&left_out_line(l));
        }
        if !self.skipped.is_empty() {
            out.push_str("\nNo mark, because their Profile could not be read:\n");
        }
        for s in &self.skipped {
            out.push_str(&format!(
                "{} (athlete {}): {}\n",
                s.name, s.athlete_id, s.error
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meet::RelayLeg;
    use crate::model::{Bio, athlete_url};
    use crate::source::FixtureSource;

    /// A Profile with one outdoor 800m mark (`SortInt`), or none.
    fn bio(id: i64, mark: Option<i64>) -> Bio {
        let results: Vec<serde_json::Value> = mark
            .map(|s| {
                serde_json::json!({
                    "AthleteID": id, "Result": format_running(s), "SortInt": s, "EventID": 4,
                    "EventTypeID": 0, "SeasonID": 2026, "ResultDate": "2026-05-01T00:00:00"
                })
            })
            .into_iter()
            .collect();
        serde_json::from_value(serde_json::json!({
            "athlete": {"IDAthlete": id, "FirstName": "Runner", "LastName": id.to_string()},
            "eventsTF": [{"Event": "800 Meters", "IDEvent": 4, "Type": "T"}],
            "resultsTF": results
        }))
        .unwrap()
    }

    fn team(school: &str, ids: &[i64]) -> RelayTeam {
        RelayTeam {
            team_id: None,
            school_id: None,
            school: school.to_string(),
            letter: "A".to_string(),
            entered_sort_int: None,
            legs: ids
                .iter()
                .enumerate()
                .map(|(i, &id)| RelayLeg {
                    leg: Some(i as u32 + 1),
                    athlete_id: id,
                    name: format!("Runner {id}"),
                    athlete_url: athlete_url(id),
                    grade: None,
                })
                .collect(),
        }
    }

    fn date() -> SeedDate {
        SeedDate::new(None, "2026-11-06".parse().unwrap()).unwrap()
    }

    #[tokio::test]
    async fn a_runner_whose_profile_cannot_be_read_has_no_mark_and_is_listed() {
        // Runner 4's Profile is missing; runner 8 is a fifth entry (leg 5),
        // not one of legs 1 to 4, so not seeded.
        let src = (1..=9)
            .filter(|&id| id != 4)
            .fold(FixtureSource::new(), |s, id| {
                s.with_bio(bio(id, Some(120_000 + id)))
            });
        let teams = [
            team("Alder High", &[1, 2, 3, 4]),
            team("Birch High", &[5, 6, 7, 9, 8]),
        ];
        let outcome = seed_relay(&src, &teams, "800m", false, date(), &FetchOpts::default())
            .await
            .unwrap();
        assert_eq!(outcome.runners, 8, "the first four legs of each team");
        let failed: Vec<i64> = outcome.failures.iter().map(|f| f.athlete_id).collect();
        assert_eq!(failed, [4]);
        let got: Vec<(&str, Option<i64>, usize)> = outcome
            .teams
            .iter()
            .map(|t| (t.team.as_str(), t.sort_int, t.legs.len()))
            .collect();
        assert_eq!(
            got,
            [
                ("Birch High", Some(480_000 + 5 + 6 + 7 + 9), 4),
                ("Alder High", None, 4),
            ]
        );
        assert_eq!(outcome.teams[1].legs[3].pr, "");
    }

    #[tokio::test]
    async fn one_runner_on_two_legs_leaves_the_team_without_an_aggregate() {
        let src = (1..=5).fold(FixtureSource::new(), |s, id| {
            s.with_bio(bio(id, (id != 5).then_some(120_000)))
        });
        let teams = [
            lettered("Alder High", &[("A1", 1), ("A2", 2), ("A3", 1), ("A4", 4)]),
            // Every cause at once: no mark, no runner, a Profile not read.
            lettered("Birch High", &[("A1", 5), ("A2", 9), ("A4", 3)]),
        ];
        let outcome = seed_relay(&src, &teams, "800m", false, date(), &FetchOpts::default())
            .await
            .unwrap();
        let alder = &outcome.teams[0];
        assert_eq!(alder.same_runner_legs, vec![vec![1, 3]]);
        assert_eq!(alder.sort_int, None, "a runner runs one leg");
        assert_eq!(
            alder.no_aggregate_reason.as_deref(),
            Some("Same runner on legs 1 and 3")
        );
        let birch = &outcome.teams[1];
        assert_eq!(
            birch.no_aggregate_reason.as_deref(),
            Some(
                "Leg 1 has no 800 Meters mark; Leg 2 has a profile that could not be read; \
                 Leg 3 has no runner"
            )
        );
        assert_eq!(
            legs_phrase(&[2, 4], "x", "have no runner"),
            "Legs 2, 4 have no runner"
        );
        assert_eq!(and_list(&[1, 2, 4]), "1, 2 and 4");
    }

    #[test]
    fn a_mile_converts_to_the_1600m_at_even_pace() {
        // 5:02.10 for the mile is 5:00.3456 for 1600m, rounded up to 5:00.35.
        assert_eq!(mile_to_1600m(302_100), 300_350);
        assert_eq!(format_running(mile_to_1600m(302_100)), "5:00.35");
        assert_eq!(mile_to_1600m(240_000), 238_610);
        assert_eq!(mile_to_1600m(0), 0);
    }

    #[tokio::test]
    async fn teams_tie_in_entry_order_and_incomplete_teams_go_last() {
        let src = (1..=7).fold(FixtureSource::new(), |s, id| {
            s.with_bio(bio(id, (id != 7).then_some(120_000)))
        });
        let teams = [
            team("Short High", &[1, 2]),
            team("Cedar High", &[1, 2, 3, 4]),
            team("Dogwood High", &[3, 4, 5, 6]),
            team("Elm High", &[1, 2, 3, 7]),
        ];
        let outcome = seed_relay(&src, &teams, "800m", false, date(), &FetchOpts::default())
            .await
            .unwrap();
        let got: Vec<(usize, &str, &str, Vec<u32>)> = outcome
            .teams
            .iter()
            .map(|t| {
                (
                    t.rank,
                    t.team.as_str(),
                    t.aggregate.as_str(),
                    t.missing_legs.clone(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                (1, "Cedar High", "8:00.00", vec![]),
                (2, "Dogwood High", "8:00.00", vec![]),
                (3, "Short High", "", vec![3, 4]),
                (4, "Elm High", "", vec![]),
            ]
        );
    }

    /// A team entered with these `(leg letter, athlete id)` rows, in entry
    /// order.
    fn lettered(school: &str, rows: &[(&str, i64)]) -> RelayTeam {
        let data = crate::meet::EntriesData {
            event_id: None,
            entries: rows
                .iter()
                .map(|&(letter, id)| {
                    serde_json::from_value(serde_json::json!({
                        "IDAthlete": id, "FirstName": "Runner", "LastName": id.to_string(),
                        "TeamID": 1, "TeamName": school, "Letter": letter,
                        "AcceptanceStatus": 10
                    }))
                    .unwrap()
                })
                .collect(),
            teams: Vec::new(),
        };
        crate::meet::EventEntries::from_data(&data, true)
            .teams
            .remove(0)
    }

    /// (leg, athlete id) of each seeded runner, and the legs with none.
    fn seated(t: &SeededTeam) -> (Vec<(u32, i64)>, Vec<u32>) {
        (
            t.legs.iter().map(|l| (l.leg, l.athlete_id)).collect(),
            t.missing_legs.clone(),
        )
    }

    #[tokio::test]
    async fn an_alternate_never_displaces_a_leg_and_a_missing_leg_is_named() {
        let src = (1..=5).fold(FixtureSource::new(), |s, id| {
            s.with_bio(bio(id, Some(120_000)))
        });
        // A second A2 entry (an alternate) and an unnumbered one come after
        // the real A2; the first entry per leg runs it.
        let teams = [
            lettered(
                "Alder High",
                &[
                    ("A1", 1),
                    ("A2", 2),
                    ("A2", 5),
                    ("A", 4),
                    ("A3", 3),
                    ("A4", 4),
                ],
            ),
            lettered("Birch High", &[("A1", 1), ("A2", 2), ("A4", 4)]),
        ];
        let outcome = seed_relay(&src, &teams, "800m", false, date(), &FetchOpts::default())
            .await
            .unwrap();
        assert_eq!(outcome.runners, 4, "the alternate's Profile is not read");
        let alder = &outcome.teams[0];
        assert_eq!(
            seated(alder),
            (vec![(1, 1), (2, 2), (3, 3), (4, 4)], vec![])
        );
        assert_eq!(alder.aggregate, "8:00.00");
        let birch = &outcome.teams[1];
        assert_eq!(seated(birch), (vec![(1, 1), (2, 2), (4, 4)], vec![3]));
        assert_eq!(birch.sort_int, None);
        // The table names the leg actually missing.
        let slots: Vec<(u32, Option<i64>)> = birch
            .slots()
            .map(|(n, l)| (n, l.map(|l| l.athlete_id)))
            .collect();
        assert_eq!(slots, [(1, Some(1)), (2, Some(2)), (3, None), (4, Some(4))]);
    }
}
