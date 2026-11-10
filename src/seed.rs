//! The seeding pipeline: roster + data source → ranked seed sheet.
//!
//! Bios are fetched with bounded concurrency and an inter-batch delay (be a good
//! API citizen). A single athlete's fetch failure is collected and skipped, not
//! fatal - one bad id must not discard the whole sheet.
//!
//! Each entrant is seeded on their Seed mark: the Personal record in the event
//! as of today (`CONTEXT.md`), across every outdoor season. Seed sheets are
//! only for upcoming meets, so a meet date in the past is refused and a meet
//! today is seeded as of the day before ([`SeedDate`]).
//!
//! The entrants come from a roster (Mode B, every row resolved to a pinned
//! `athlete_id` first) or from a meet's entries (Mode A, where every entry
//! already carries its athlete id); either way they are a list of
//! [`RosterRow`]s with ids.

use std::time::Duration;

use anyhow::{Result, bail};
use chrono::NaiveDate;
use serde::Serialize;

use crate::contract::{Report, columns};
use crate::mark::{
    AsOf, personal_record_as_of, resolve_event, running_seconds_from_sortint, strip_timing_suffix,
};
use crate::meet::{EventRef, LeftOut, MeetRef, left_out_line};
use crate::model::{Bio, SeedRow, athlete_url};
use crate::resolve::ResolveOutcome;
use crate::roster::RosterRow;
use crate::source::DataSource;

/// How bios are fetched.
#[derive(Debug, Clone)]
pub struct FetchOpts {
    /// Max bios fetched concurrently.
    pub concurrency: usize,
    /// Delay between concurrent batches.
    pub delay: Duration,
}

impl Default for FetchOpts {
    fn default() -> Self {
        Self {
            concurrency: 5,
            delay: Duration::ZERO,
        }
    }
}

/// An athlete whose bio could not be fetched (skipped from the sheet).
#[derive(Debug, Clone)]
pub struct FetchFailure {
    pub name: String,
    pub athlete_id: i64,
    pub error: String,
}

/// The result of a seeding run.
#[derive(Debug, Clone, Default)]
pub struct SeedOutcome {
    pub rows: Vec<SeedRow>,
    pub failures: Vec<FetchFailure>,
    /// The event's own name (`400 Meters`) as the first entrant's Profile
    /// spells it, or `None` when no entrant's Profile has the event.
    pub event_name: Option<String>,
}

/// The date a seed sheet's marks are read as of.
///
/// Seed sheets are only for upcoming meets, so marks are as of today, except
/// that for a meet today they are as of the day before: a result from the meet
/// itself must never become its seed. A meet before today is refused: seed
/// sheets never reconstruct a past seeding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeedDate {
    as_of: AsOf,
    meet: Option<NaiveDate>,
}

/// Why a past meet is refused, after what was given.
const PAST_MEET: &str =
    "Seed sheets are only for upcoming meets; ath does not reconstruct a past seeding.";

impl SeedDate {
    /// Marks as of `min(today, meet_date - 1 day)`, or as of `today` with no
    /// meet date. Errors when `meet_date` is before `today`.
    pub fn new(meet_date: Option<NaiveDate>, today: NaiveDate) -> Result<Self> {
        let as_of = match meet_date {
            Some(meet) if meet < today => {
                bail!("--meet-date {meet} is in the past (today is {today}). {PAST_MEET}")
            }
            Some(meet) => meet.pred_opt().unwrap_or(meet).min(today),
            None => today,
        };
        Ok(Self {
            as_of: AsOf { date: as_of, today },
            meet: meet_date,
        })
    }

    /// For a meet read from Athletic.net (Mode A) that starts on `start` and
    /// ends on `end`: refused only once it is over (`end` before today), and
    /// marks as of the day before it starts at the latest, so a meet on its
    /// second day never seeds on its first day's results. The error names the
    /// meet instead of `--meet-date`.
    pub fn for_meet(
        meet_id: i64,
        start: NaiveDate,
        end: NaiveDate,
        today: NaiveDate,
    ) -> Result<Self> {
        if end < today {
            let when =
                if end == start { format!("was on {start}") } else { format!("ended on {end}") };
            bail!("meet {meet_id} {when}, in the past (today is {today}). {PAST_MEET}");
        }
        Ok(Self {
            as_of: AsOf {
                date: start.pred_opt().unwrap_or(start).min(today),
                today,
            },
            meet: Some(start),
        })
    }

    /// The meet date given, if any.
    pub fn meet_date(&self) -> Option<NaiveDate> {
        self.meet
    }

    /// The last day whose marks count.
    pub fn as_of(&self) -> NaiveDate {
        self.as_of.date
    }

    /// True for a meet today, whose marks are read as of the day before.
    pub fn is_meet_day(&self) -> bool {
        self.as_of.date < self.as_of.today
    }
}

/// Build a seed sheet for `event` from `roster`, fetching each athlete's
/// bio via `source`. Each entrant's Seed mark is their Personal record in the
/// event as of `date` (any outdoor season). Rows are sorted fastest-first by
/// `SortInt`, with no-mark athletes blank and last, then ranked 1..n. Athletes
/// whose bio can't be fetched are reported in `failures` rather than aborting
/// the run.
pub async fn seed<S: DataSource + ?Sized>(
    source: &S,
    roster: &[RosterRow],
    event: &str,
    date: SeedDate,
    opts: &FetchOpts,
) -> Result<SeedOutcome> {
    let mut outcomes = seed_events(source, roster, &[event], date, opts).await?;
    Ok(outcomes.remove(0))
}

/// [`seed`] on several events from one read of each Profile: one outcome
/// per event, in `events`' order, each with the same `failures`.
pub async fn seed_events<S: DataSource + ?Sized>(
    source: &S,
    roster: &[RosterRow],
    events: &[&str],
    date: SeedDate,
    opts: &FetchOpts,
) -> Result<Vec<SeedOutcome>> {
    // Every row must be pinned; the caller resolves un-pinned rows by name first.
    let mut pinned: Vec<(&str, i64)> = Vec::with_capacity(roster.len());
    for entry in roster {
        let Some(id) = entry.athlete_id else {
            bail!(
                "roster row for {:?} has no athlete_id; seeding takes resolved rows \
                 only - pin an athlete_id",
                entry.name
            );
        };
        pinned.push((entry.name.as_str(), id));
    }

    let concurrency = opts.concurrency.max(1);
    let mut built: Vec<Vec<SeedRow>> = vec![Vec::with_capacity(pinned.len()); events.len()];
    let mut event_names: Vec<Option<String>> = vec![None; events.len()];
    let mut failures: Vec<FetchFailure> = Vec::new();

    for (batch, chunk) in pinned.chunks(concurrency).enumerate() {
        if batch > 0 && !opts.delay.is_zero() {
            tokio::time::sleep(opts.delay).await;
        }
        // join_all preserves input order, and chunks run in order, so `built`
        // stays in roster order (keeps no-mark ties stable after sorting).
        let results = futures::future::join_all(
            chunk
                .iter()
                .map(|&(name, id)| async move { (name, id, source.get_bio(id).await) }),
        )
        .await;

        for (name, id, res) in results {
            match res {
                Ok(bio) => {
                    for (i, event) in events.iter().enumerate() {
                        let (row, name) = build_row(&bio, id, event, date);
                        event_names[i] = event_names[i].take().or(name);
                        built[i].push(row);
                    }
                }
                Err(e) => failures.push(FetchFailure {
                    name: name.to_string(),
                    athlete_id: id,
                    error: format!("{e:#}"),
                }),
            }
        }
    }

    Ok(built
        .into_iter()
        .zip(event_names)
        .map(|(mut rows, event_name)| {
            // Marks first, ascending by SortInt; no-mark rows last (stable order).
            rows.sort_by(|a, b| match (a.sort_int, b.sort_int) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            });
            SeedOutcome {
                rows: rows
                    .into_iter()
                    .enumerate()
                    .map(|(i, row)| SeedRow { rank: i + 1, ..row })
                    .collect(),
                failures: failures.clone(),
                event_name,
            }
        })
        .collect())
}

/// One entrant's seed row (rank assigned after sorting), and the event's name
/// as their Profile spells it, when it has the event.
fn build_row(bio: &Bio, athlete_id: i64, event: &str, date: SeedDate) -> (SeedRow, Option<String>) {
    let resolved = resolve_event(bio, event);
    let event_name = resolved.as_ref().map(|e| e.name.clone());
    let event_ids = resolved.map(|e| e.ids).unwrap_or_default();
    let best = personal_record_as_of(bio, &event_ids, date.as_of);

    let school = bio.school();
    let club = bio.club();

    let (pr, pr_numeric, pr_season, sort_int) = match best {
        Some(r) => (
            strip_timing_suffix(&r.result).to_string(),
            Some(running_seconds_from_sortint(r.sort_int)),
            Some(r.season_id),
            Some(r.sort_int),
        ),
        None => (String::new(), None, None, None),
    };

    let row = SeedRow {
        rank: 0, // assigned after sorting
        athlete: bio.athlete.full_name(),
        athlete_id,
        athlete_url: athlete_url(athlete_id),
        pr,
        pr_numeric,
        school: school.map(|t| t.name.clone()).unwrap_or_default(),
        school_url: school.map(|t| t.url()).unwrap_or_default(),
        club: club.map(|t| t.name.clone()).unwrap_or_default(),
        club_url: club.map(|t| t.url()).unwrap_or_default(),
        pr_season,
        sort_int,
    };
    (row, event_name)
}

/// A seed sheet as `ath seed` prints it (the output contract, ADR 0007): the
/// same entrants, in the same order, as the seed sheet CSV, plus the
/// `SortInt` each is sorted on, the date the marks are as of, and every
/// roster row that is not on the sheet, with why. The seed sheet page
/// (`tools/seedsheet/`) renders it.
///
/// A Mode B sheet has no single Athletic.net view of its own (a roster is not
/// a meet's entries page), so its links are per entrant: each one's Profile,
/// school and club. A Mode A sheet also has `meet`: the meet, the event's
/// entries page, and the entries left out.
#[derive(Debug, Clone, Serialize)]
pub struct SeedSheet {
    /// The event as given (`--event`: `400m` or `400 Meters`).
    pub event: String,
    /// The event's own name (`400 Meters`), when any entrant's Profile has it.
    pub event_name: Option<String>,
    /// The last day whose marks count: today, or the day before a meet today.
    pub as_of: NaiveDate,
    /// The meet date given (`--meet-date`), or the meet's own (`--meet`), if
    /// any.
    pub meet_date: Option<NaiveDate>,
    /// Mode A (`--meet`): where the entrants came from. Left out of a roster's
    /// sheet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meet: Option<SheetMeet>,
    /// Every entrant in seed order, as in the CSV: marks fastest first, then
    /// the ones with no Qualifying mark.
    pub entrants: Vec<Entrant>,
    /// Athletes left off the sheet because their Profile could not be read.
    pub skipped: Vec<Skipped>,
    /// Roster rows left off the sheet because they did not resolve to one
    /// athlete (no match, several, or the search failed).
    pub unresolved: Vec<NotResolved>,
}

/// The meet a Mode A seed sheet's entrants came from: the meet (its `url`
/// is the meet's page), the event in its division (its `url` is the entries
/// page), and the entries that are not Entrants, with why.
#[derive(Debug, Clone, Serialize)]
pub struct SheetMeet {
    #[serde(flatten)]
    pub meet: MeetRef,
    pub event: EventRef,
    pub left_out: Vec<LeftOut>,
}

/// One seed sheet row: the CSV's columns (in the CSV's order) plus the
/// `SortInt` it is sorted on (milliseconds; `null` with no mark).
#[derive(Debug, Clone, Serialize)]
pub struct Entrant {
    #[serde(flatten)]
    pub row: SeedRow,
    pub sort_int: Option<i64>,
}

/// An athlete whose Profile could not be read, so is not on the sheet.
#[derive(Debug, Clone, Serialize)]
pub struct Skipped {
    pub name: String,
    pub athlete_id: i64,
    pub athlete_url: String,
    pub error: String,
}

impl From<&FetchFailure> for Skipped {
    fn from(f: &FetchFailure) -> Self {
        Self {
            name: f.name.clone(),
            athlete_id: f.athlete_id,
            athlete_url: athlete_url(f.athlete_id),
            error: f.error.clone(),
        }
    }
}

/// Why a roster row did not resolve to one athlete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnresolvedReason {
    /// No Profile matched the name.
    NoMatch,
    /// Several Profiles matched and none stood out.
    Ambiguous,
    /// The search itself failed.
    SearchFailed,
}

/// A roster row that is not on the sheet because it did not resolve.
#[derive(Debug, Clone, Serialize)]
pub struct NotResolved {
    /// The name as the roster has it.
    pub name: String,
    pub reason: UnresolvedReason,
    /// What happened and what to do, in words.
    pub note: String,
}

/// The roster rows `resolution` could not seed, in roster order where it
/// kept one (unresolved rows, then failed searches). `review` is the CSV the
/// unresolved rows' candidates were written to.
pub fn not_resolved(resolution: &ResolveOutcome, review: &str) -> Vec<NotResolved> {
    let mut out: Vec<NotResolved> = resolution
        .unresolved
        .iter()
        .map(|u| match u.candidates.len() {
            0 => NotResolved {
                name: u.input.name.clone(),
                reason: UnresolvedReason::NoMatch,
                note: "no Profile matches the name; pin an athlete_id".to_string(),
            },
            n => NotResolved {
                name: u.input.name.clone(),
                reason: UnresolvedReason::Ambiguous,
                note: format!("{n} possible Profiles; pin an athlete_id (candidates in {review})"),
            },
        })
        .collect();
    out.extend(
        resolution
            .search_failures
            .iter()
            .map(|(name, err)| NotResolved {
                name: name.clone(),
                reason: UnresolvedReason::SearchFailed,
                note: format!("the search failed: {err}"),
            }),
    );
    out
}

impl SeedSheet {
    /// The report for one seeding run of `event` on `date`, with the roster
    /// rows that did not resolve (`not_resolved`).
    pub fn new(
        event: &str,
        date: SeedDate,
        outcome: &SeedOutcome,
        unresolved: Vec<NotResolved>,
    ) -> Self {
        Self {
            event: event.to_string(),
            event_name: outcome.event_name.clone(),
            as_of: date.as_of(),
            meet_date: date.meet_date(),
            entrants: outcome
                .rows
                .iter()
                .map(|r| Entrant {
                    row: r.clone(),
                    sort_int: r.sort_int,
                })
                .collect(),
            skipped: outcome.failures.iter().map(Skipped::from).collect(),
            unresolved,
            meet: None,
        }
    }

    /// The same sheet, for a meet's entrants (Mode A).
    pub fn with_meet(mut self, meet: SheetMeet) -> Self {
        self.meet = Some(meet);
        self
    }
}

/// `n entrants`, `1 entrant`.
fn entrants(n: usize) -> String {
    format!("{n} entrant{}", if n == 1 { "" } else { "s" })
}

impl Report for SeedSheet {
    const SCHEMA: u32 = 1;

    fn table(&self) -> String {
        let event = self.event_name.as_deref().unwrap_or(&self.event);
        let meet = match (&self.meet, self.meet_date) {
            (Some(m), _) => format!(
                ", for {} (meet {}){}, {} {}",
                m.meet.name,
                m.meet.id,
                m.meet.date.map_or_else(String::new, |d| format!(" on {d}")),
                m.event.gender.noun(),
                m.event.division,
            ),
            (None, Some(d)) => format!(", for a meet on {d}"),
            (None, None) => String::new(),
        };
        let seeded = self
            .entrants
            .iter()
            .filter(|e| e.sort_int.is_some())
            .count();
        let mut out = format!(
            "{event} seed sheet: Personal records as of {}{meet}\n{}, {seeded} with a mark\n",
            self.as_of,
            entrants(self.entrants.len())
        );
        if !self.entrants.is_empty() {
            let mut rows = vec![
                [
                    "Seed", "Athlete", "PR", "Season", "School", "Club", "Profile",
                ]
                .map(String::from)
                .to_vec(),
            ];
            rows.extend(self.entrants.iter().map(|e| {
                let r = &e.row;
                vec![
                    r.rank.to_string(),
                    r.athlete.clone(),
                    if r.pr.is_empty() { "no mark".to_string() } else { r.pr.clone() },
                    r.pr_season.map_or_else(String::new, |s| s.to_string()),
                    r.school.clone(),
                    r.club.clone(),
                    r.athlete_url.clone(),
                ]
            }));
            out.push('\n');
            out.push_str(&columns(&rows));
        }
        let left_out = self.meet.as_ref().map_or(&[][..], |m| &m.left_out[..]);
        if !self.skipped.is_empty() || !self.unresolved.is_empty() || !left_out.is_empty() {
            out.push_str("\nNot on the sheet:\n");
        }
        for l in left_out {
            out.push_str(&left_out_line(l));
        }
        for s in &self.skipped {
            out.push_str(&format!(
                "{} (athlete {}): Profile could not be read: {}\n",
                s.name, s.athlete_id, s.error
            ));
        }
        for u in &self.unresolved {
            out.push_str(&format!("{}: {}\n", u.name, u.note));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate {
        s.parse().unwrap()
    }

    #[test]
    fn a_past_meet_is_refused_and_says_why() {
        let err = SeedDate::new(Some(day("2026-10-25")), day("2026-10-26")).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("--meet-date 2026-10-25 is in the past"),
            "{msg}"
        );
        assert!(msg.contains("today is 2026-10-26"), "{msg}");
        assert!(msg.contains("only for upcoming meets"), "{msg}");
        assert!(msg.contains("does not reconstruct a past seeding"), "{msg}");
    }

    #[test]
    fn a_past_meet_from_athletic_net_is_refused_by_its_id() {
        let past = |start, end| {
            SeedDate::for_meet(900701, day(start), day(end), day("2026-10-26"))
                .unwrap_err()
                .to_string()
        };
        assert_eq!(
            past("2020-01-11", "2020-01-11"),
            "meet 900701 was on 2020-01-11, in the past (today is 2026-10-26). Seed sheets \
             are only for upcoming meets; ath does not reconstruct a past seeding."
        );
        assert!(
            past("2026-10-24", "2026-10-25")
                .starts_with("meet 900701 ended on 2026-10-25, in the past"),
        );
        let d = SeedDate::for_meet(
            900700,
            day("2026-10-26"),
            day("2026-10-26"),
            day("2026-10-26"),
        )
        .unwrap();
        assert_eq!(d.meet_date(), Some(day("2026-10-26")));
        assert_eq!(d.as_of(), day("2026-10-25"));
        assert!(d.is_meet_day());
    }

    #[test]
    fn a_two_day_meet_on_its_second_day_seeds_as_of_the_day_before_it_started() {
        let d = SeedDate::for_meet(
            900702,
            day("2026-10-25"),
            day("2026-10-26"),
            day("2026-10-26"),
        )
        .unwrap();
        assert_eq!(d.as_of(), day("2026-10-24"), "not the first day's results");
        assert_eq!(d.meet_date(), Some(day("2026-10-25")));
        assert!(d.is_meet_day());
        // A later meet still seeds as of today.
        let d = SeedDate::for_meet(
            900702,
            day("2026-11-01"),
            day("2026-11-02"),
            day("2026-10-26"),
        )
        .unwrap();
        assert_eq!(d.as_of(), day("2026-10-26"));
    }

    #[test]
    fn no_meet_date_or_a_later_meet_reads_marks_as_of_today() {
        let today = day("2026-10-26");
        for meet in [None, Some(day("2026-10-27")), Some(day("2027-05-01"))] {
            let d = SeedDate::new(meet, today).unwrap();
            assert_eq!(d.as_of(), today, "{meet:?}");
            assert!(!d.is_meet_day(), "{meet:?}");
        }
    }

    #[test]
    fn a_meet_today_reads_marks_as_of_yesterday_without_undated_ones() {
        let d = SeedDate::new(Some(day("2026-10-26")), day("2026-10-26")).unwrap();
        assert_eq!(d.as_of(), day("2026-10-25"));
        assert!(d.is_meet_day());
    }

    #[test]
    fn the_table_counts_entrants_in_words() {
        assert_eq!(entrants(0), "0 entrants");
        assert_eq!(entrants(1), "1 entrant");
        assert_eq!(entrants(2), "2 entrants");
        let date = SeedDate::new(None, day("2026-10-26")).unwrap();
        let empty = SeedSheet::new("400m", date, &SeedOutcome::default(), Vec::new());
        assert_eq!(
            empty.table(),
            "400m seed sheet: Personal records as of 2026-10-26\n0 entrants, 0 with a mark\n"
        );
    }
}
