//! Writing the seed sheet and the Mode B resolution outputs to CSV.

use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::meet::RELAY_LEGS;
use crate::model::{SeedRow, athlete_url};
use crate::resolve::{AutoMatch, Unresolved};
use crate::seed_relay::SeededTeam;

/// The seed-sheet column header, kept in sync with [`SeedRow`]'s field order.
const SEED_HEADER: [&str; 11] = [
    "rank",
    "athlete",
    "athlete_id",
    "athlete_url",
    "pr",
    "pr_numeric",
    "school",
    "school_url",
    "club",
    "club_url",
    "pr_season",
];

/// Serialize seed rows to a CSV string. The header is always written - even for
/// an empty sheet - so the output contract is consistent.
pub fn seed_csv_string(rows: &[SeedRow]) -> Result<String> {
    let mut w = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(Vec::new());
    w.write_record(SEED_HEADER)?;
    for row in rows {
        w.serialize(row)?;
    }
    let bytes = w.into_inner().context("finishing CSV")?;
    String::from_utf8(bytes).context("CSV was not valid UTF-8")
}

/// Write seed rows to a CSV file.
pub fn write_seed_csv(path: impl AsRef<Path>, rows: &[SeedRow]) -> Result<()> {
    let path = path.as_ref();
    let csv = seed_csv_string(rows)?;
    std::fs::write(path, csv).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// The relay seed sheet's columns (`docs/HANDOFF.md` §6, Relay): the team
/// and its aggregate (`aggregate_estimated` is `true` when any leg's mark is
/// an Estimated mark), then each of the four runners' name, athlete id,
/// Profile, Personal record in the Seeding event, and, for an Estimated mark
/// (a 4xMile runner's mile converted to the 1600m), what it was made from
/// (`1 Mile 5:02.10`; blank for a real mark).
pub fn relay_csv_header() -> Vec<String> {
    let mut header: Vec<String> = [
        "rank",
        "team",
        "letter",
        "team_url",
        "aggregate",
        "aggregate_seconds",
        "aggregate_estimated",
        "no_aggregate_reason",
    ]
    .map(String::from)
    .to_vec();
    for n in 1..=RELAY_LEGS {
        for field in ["", "_id", "_url", "_pr", "_estimated_from"] {
            header.push(format!("runner{n}{field}"));
        }
    }
    header
}

/// Serialize relay teams to a CSV string, in their order, header always
/// written. A missing runner's four columns are blank.
pub fn relay_csv_string(teams: &[SeededTeam]) -> Result<String> {
    let mut w = csv::WriterBuilder::new().from_writer(Vec::new());
    w.write_record(relay_csv_header())?;
    for t in teams {
        let mut record = vec![
            t.rank.to_string(),
            t.team.clone(),
            t.letter.clone(),
            t.team_url.clone(),
            t.aggregate.clone(),
            // As serde writes an individual sheet's `pr_numeric` (`55.0`).
            t.aggregate_seconds
                .map_or_else(String::new, |s| format!("{s:?}")),
            // `true` when any leg is an Estimated mark.
            t.estimated.to_string(),
            // Why there is no aggregate, in words; blank with one.
            t.no_aggregate_reason.clone().unwrap_or_default(),
        ];
        // runnerN is leg N, blank when leg N has no runner.
        for (_, leg) in t.slots() {
            match leg {
                Some(l) => record.extend([
                    l.athlete.clone(),
                    l.athlete_id.to_string(),
                    l.athlete_url.clone(),
                    l.pr.clone(),
                    // What an Estimated mark was made from: `1 Mile 5:02.10`.
                    match (&l.estimated_from, &l.estimated_from_pr) {
                        (Some(event), Some(pr)) => format!("{event} {pr}"),
                        _ => String::new(),
                    },
                ]),
                None => record.extend(std::iter::repeat_n(String::new(), 5)),
            }
        }
        w.write_record(&record)?;
    }
    let bytes = w.into_inner().context("finishing CSV")?;
    String::from_utf8(bytes).context("CSV was not valid UTF-8")
}

/// Write relay teams to a CSV file.
pub fn write_relay_csv(path: impl AsRef<Path>, teams: &[SeededTeam]) -> Result<()> {
    let path = path.as_ref();
    let csv = relay_csv_string(teams)?;
    std::fs::write(path, csv).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// One long-form row of the unresolved report: one line per candidate (or a
/// single line with blank candidate fields when a name had no matches).
#[derive(Serialize)]
struct UnresolvedCsvRow {
    input_name: String,
    input_school: String,
    input_city: String,
    input_state: String,
    cand_rank: Option<usize>,
    cand_score: Option<f64>,
    athlete_id: Option<i64>,
    cand_name: String,
    cand_school: String,
    cand_city: String,
    cand_state: String,
    athlete_url: String,
}

/// Write the unresolved rows + their ranked candidates for the user to review.
pub fn write_unresolved_csv(path: impl AsRef<Path>, unresolved: &[Unresolved]) -> Result<()> {
    let path = path.as_ref();
    let mut w = csv::Writer::from_path(path)
        .with_context(|| format!("opening {} for write", path.display()))?;
    for u in unresolved {
        let (name, school, city, state) = (
            u.input.name.clone(),
            u.input.school.clone().unwrap_or_default(),
            u.input.city.clone().unwrap_or_default(),
            u.input.state.clone().unwrap_or_default(),
        );
        if u.candidates.is_empty() {
            w.serialize(UnresolvedCsvRow {
                input_name: name,
                input_school: school,
                input_city: city,
                input_state: state,
                cand_rank: None,
                cand_score: None,
                athlete_id: None,
                cand_name: String::new(),
                cand_school: String::new(),
                cand_city: String::new(),
                cand_state: String::new(),
                athlete_url: String::new(),
            })?;
            continue;
        }
        for (i, sc) in u.candidates.iter().enumerate() {
            let c = &sc.candidate;
            w.serialize(UnresolvedCsvRow {
                input_name: name.clone(),
                input_school: school.clone(),
                input_city: city.clone(),
                input_state: state.clone(),
                cand_rank: Some(i + 1),
                cand_score: Some(c.score),
                athlete_id: Some(c.athlete_id),
                cand_name: c.name.clone(),
                cand_school: c.school.clone().unwrap_or_default(),
                cand_city: c.city.clone().unwrap_or_default(),
                cand_state: c.state.clone().unwrap_or_default(),
                athlete_url: athlete_url(c.athlete_id),
            })?;
        }
    }
    w.flush()
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[derive(Serialize)]
struct ResolutionLogRow {
    input_name: String,
    athlete_id: i64,
    search_score: f64,
    reason: String,
}

/// Write the opt-in resolution log of what auto-matched and why.
pub fn write_resolution_log(path: impl AsRef<Path>, matches: &[AutoMatch]) -> Result<()> {
    let path = path.as_ref();
    let mut w = csv::Writer::from_path(path)
        .with_context(|| format!("opening {} for write", path.display()))?;
    for m in matches {
        w.serialize(ResolutionLogRow {
            input_name: m.name.clone(),
            athlete_id: m.athlete_id,
            search_score: m.score,
            reason: m.reason.clone(),
        })?;
    }
    w.flush()
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}
