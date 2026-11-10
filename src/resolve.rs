//! Mode B name resolution (ADR-0003, two-pass).
//!
//! A roster row with an `athlete_id` is used directly. Otherwise we `search` by
//! name and score the candidates against the optional locators (`school`,
//! `city`, `state`). A **confident** match auto-resolves (and the `athlete_id`
//! is written back to the roster); anything else is left **unresolved** with its
//! ranked candidates for the user to review, pin, and re-run.
//!
//! We resolve conservatively: when a name-only row has several plausible hits,
//! we do NOT guess - a wrong silent pick would seed the wrong person's marks.

use anyhow::Result;

use crate::model::Candidate;
use crate::roster::RosterRow;
use crate::source::DataSource;

/// A candidate with its locator-match score against a roster row.
#[derive(Debug, Clone)]
pub struct ScoredCandidate {
    pub candidate: Candidate,
    /// School match = 2, city = 1, state = 1, counting only locators the roster
    /// row actually provides.
    pub locator_score: u32,
}

/// An auto-resolved match (for the resolution log + write-back).
#[derive(Debug, Clone)]
pub struct AutoMatch {
    pub name: String,
    pub athlete_id: i64,
    pub score: f64,
    pub reason: String,
}

/// A roster row that did not resolve, with its ranked candidates.
#[derive(Debug, Clone)]
pub struct Unresolved {
    pub input: RosterRow,
    pub candidates: Vec<ScoredCandidate>,
}

/// The result of resolving a whole roster.
#[derive(Debug, Clone, Default)]
pub struct ResolveOutcome {
    /// The roster in original order with `athlete_id` filled where resolved
    /// (for write-back).
    pub updated_roster: Vec<RosterRow>,
    /// The subset that resolved to a single athlete (all have `athlete_id`),
    /// ready to seed.
    pub resolved: Vec<RosterRow>,
    /// Rows auto-resolved via search this run (for the resolution log).
    pub auto_matches: Vec<AutoMatch>,
    /// Rows left for the user to disambiguate.
    pub unresolved: Vec<Unresolved>,
    /// Rows whose `search` call errored (name, error) - skipped, not fatal.
    pub search_failures: Vec<(String, String)>,
}

/// Resolve every roster row to an athlete via `source`.
pub async fn resolve<S: DataSource + ?Sized>(
    source: &S,
    roster: &[RosterRow],
) -> Result<ResolveOutcome> {
    let mut out = ResolveOutcome::default();

    for row in roster {
        if row.athlete_id.is_some() {
            out.updated_roster.push(row.clone());
            out.resolved.push(row.clone());
            continue;
        }

        let candidates = match source.search(&row.name).await {
            Ok(c) => c,
            Err(e) => {
                // A single search failure is skipped, not fatal - keep the row
                // in the roster (unpinned) and report it.
                out.search_failures
                    .push((row.name.clone(), format!("{e:#}")));
                out.updated_roster.push(row.clone());
                continue;
            }
        };
        let ranked = rank(candidates, row);

        match decide(&ranked, row) {
            Some(idx) => {
                let chosen = &ranked[idx];
                let mut resolved_row = row.clone();
                resolved_row.athlete_id = Some(chosen.candidate.athlete_id);
                out.auto_matches.push(AutoMatch {
                    name: row.name.clone(),
                    athlete_id: chosen.candidate.athlete_id,
                    score: chosen.candidate.score,
                    reason: match_reason(&ranked, idx),
                });
                out.updated_roster.push(resolved_row.clone());
                out.resolved.push(resolved_row);
            }
            None => {
                out.updated_roster.push(row.clone());
                out.unresolved.push(Unresolved {
                    input: row.clone(),
                    candidates: ranked,
                });
            }
        }
    }

    Ok(out)
}

/// Score candidates against the row's locators and sort best-first (locator
/// score, then search relevance).
fn rank(candidates: Vec<Candidate>, row: &RosterRow) -> Vec<ScoredCandidate> {
    let mut scored: Vec<ScoredCandidate> = candidates
        .into_iter()
        .map(|c| {
            let locator_score = locator_score(&c, row);
            ScoredCandidate {
                candidate: c,
                locator_score,
            }
        })
        .collect();
    scored.sort_by(|a, b| {
        b.locator_score
            .cmp(&a.locator_score)
            .then(b.candidate.score.total_cmp(&a.candidate.score))
    });
    scored
}

fn locator_score(c: &Candidate, row: &RosterRow) -> u32 {
    let mut score = 0;
    if let (Some(want), Some(have)) = (&row.school, &c.school)
        && school_matches(have, want)
    {
        score += 2;
    }
    if let (Some(want), Some(have)) = (&row.city, &c.city)
        && have.eq_ignore_ascii_case(want)
    {
        score += 1;
    }
    if let (Some(want), Some(have)) = (&row.state, &c.state)
        && have.eq_ignore_ascii_case(want)
    {
        score += 1;
    }
    score
}

/// A school matches when every word of the roster's school name appears as a
/// whole word in the candidate's (case-insensitive; e.g. "North High" matches
/// "North High School" but "Lincoln" does NOT match "Lincolnwood Prep"). This
/// tolerates suffixes like "(HS)" while avoiding spurious substring matches.
fn school_matches(candidate: &str, roster: &str) -> bool {
    let cand: std::collections::HashSet<String> = tokens(candidate).collect();
    let roster_words: Vec<String> = tokens(roster).collect();
    !roster_words.is_empty() && roster_words.iter().all(|w| cand.contains(w))
}

/// Lowercased alphanumeric word tokens.
fn tokens(s: &str) -> impl Iterator<Item = String> + '_ {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_ascii_lowercase())
}

/// Decide whether the ranked candidates resolve confidently, returning the
/// chosen index. Conservative by design (ADR-0003: never guess).
///
/// - Exactly one hit: confident, UNLESS the roster supplied locators and the hit
///   matches none of them (a contradiction - probably the wrong person).
/// - Several hits: confident only when the top candidate has a strong, unique
///   positive signal (locator score >= 2 - a school match, or city+state -
///   strictly beating the runner-up). A lone shared city/state (score 1) is too
///   weak to pick among several.
fn decide(ranked: &[ScoredCandidate], row: &RosterRow) -> Option<usize> {
    match ranked.len() {
        0 => None,
        1 => {
            if has_locators(row) && ranked[0].locator_score == 0 {
                None
            } else {
                Some(0)
            }
        }
        _ => {
            let top = ranked[0].locator_score;
            let second = ranked[1].locator_score;
            if top >= 2 && top > second { Some(0) } else { None }
        }
    }
}

fn has_locators(row: &RosterRow) -> bool {
    row.school.is_some() || row.city.is_some() || row.state.is_some()
}

fn match_reason(ranked: &[ScoredCandidate], idx: usize) -> String {
    let locator_score = ranked[idx].locator_score;
    match (ranked.len(), locator_score) {
        (1, 0) => "single search match".to_string(),
        (1, ls) => format!("single hit, locator score {ls}"),
        (_, ls) => format!("unique locator match (locator score {ls})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::FixtureSource;

    fn row(name: &str, school: Option<&str>, city: Option<&str>, state: Option<&str>) -> RosterRow {
        RosterRow {
            name: name.to_string(),
            athlete_id: None,
            school: school.map(str::to_string),
            city: city.map(str::to_string),
            state: state.map(str::to_string),
        }
    }

    fn cand(
        id: i64,
        name: &str,
        school: Option<&str>,
        city: Option<&str>,
        score: f64,
    ) -> Candidate {
        Candidate {
            athlete_id: id,
            name: name.to_string(),
            school: school.map(str::to_string),
            city: city.map(str::to_string),
            state: None,
            score,
        }
    }

    #[tokio::test]
    async fn pinned_row_skips_search() {
        let mut pinned = row("Alice Anders", None, None, None);
        pinned.athlete_id = Some(1001);
        // Source with NO search fixtures - if it searched, it would return empty
        // and the row would go unresolved.
        let src = FixtureSource::new();
        let out = resolve(&src, &[pinned]).await.unwrap();
        assert_eq!(out.resolved.len(), 1);
        assert_eq!(out.resolved[0].athlete_id, Some(1001));
        assert!(out.unresolved.is_empty());
        assert!(out.auto_matches.is_empty());
    }

    #[tokio::test]
    async fn single_hit_resolves_confidently() {
        let src = FixtureSource::new().with_search(
            "Jane Smith",
            vec![cand(200, "Jane Smith", Some("Some HS"), None, 100.0)],
        );
        let out = resolve(&src, &[row("Jane Smith", None, None, None)])
            .await
            .unwrap();
        assert_eq!(out.resolved.len(), 1);
        assert_eq!(out.resolved[0].athlete_id, Some(200));
        assert_eq!(out.auto_matches.len(), 1);
        assert!(out.unresolved.is_empty());
    }

    #[tokio::test]
    async fn name_only_with_multiple_hits_is_unresolved() {
        let src = FixtureSource::new().with_search(
            "John Doe",
            vec![
                cand(1, "John Doe", Some("North HS"), None, 90.0),
                cand(2, "John Doe", Some("South HS"), None, 88.0),
            ],
        );
        let out = resolve(&src, &[row("John Doe", None, None, None)])
            .await
            .unwrap();
        assert!(out.resolved.is_empty());
        assert_eq!(out.unresolved.len(), 1);
        assert_eq!(out.unresolved[0].candidates.len(), 2);
    }

    #[tokio::test]
    async fn locator_breaks_the_tie() {
        let src = FixtureSource::new().with_search(
            "John Doe",
            vec![
                cand(1, "John Doe", Some("North High School"), None, 90.0),
                cand(2, "John Doe", Some("South High School"), None, 95.0),
            ],
        );
        // Roster names the school → the matching candidate wins even though the
        // other has a higher search score.
        let out = resolve(&src, &[row("John Doe", Some("North High"), None, None)])
            .await
            .unwrap();
        assert_eq!(out.resolved.len(), 1);
        assert_eq!(out.resolved[0].athlete_id, Some(1));
    }

    #[tokio::test]
    async fn same_school_tie_stays_unresolved() {
        let src = FixtureSource::new().with_search(
            "John Doe",
            vec![
                cand(1, "John Doe", Some("North High School"), None, 90.0),
                cand(2, "John Doe", Some("North High School"), None, 88.0),
            ],
        );
        let out = resolve(&src, &[row("John Doe", Some("North High"), None, None)])
            .await
            .unwrap();
        assert!(out.resolved.is_empty());
        assert_eq!(out.unresolved.len(), 1);
    }

    #[tokio::test]
    async fn no_hits_is_unresolved_with_empty_candidates() {
        let src = FixtureSource::new(); // no search fixtures → empty results
        let out = resolve(&src, &[row("Nobody Here", None, None, None)])
            .await
            .unwrap();
        assert_eq!(out.unresolved.len(), 1);
        assert!(out.unresolved[0].candidates.is_empty());
    }

    #[tokio::test]
    async fn lone_hit_that_contradicts_the_locator_is_unresolved() {
        // The roster gives a school precisely to disambiguate; the single hit is
        // at a different school → do NOT auto-pin the wrong person.
        let src = FixtureSource::new().with_search(
            "Chris Lee",
            vec![cand(555, "Chris Lee", Some("East High"), None, 100.0)],
        );
        let out = resolve(&src, &[row("Chris Lee", Some("West High"), None, None)])
            .await
            .unwrap();
        assert!(out.resolved.is_empty());
        assert_eq!(out.unresolved.len(), 1);
    }

    #[tokio::test]
    async fn a_shared_state_is_too_weak_to_break_a_tie() {
        // Two John Does; one merely shares the state. A single weak locator point
        // must not auto-resolve over a higher-relevance candidate.
        let mut with_state = cand(1, "John Doe", None, None, 20.0);
        with_state.state = Some("CA".to_string());
        let no_state = cand(2, "John Doe", None, None, 99.0);
        let src = FixtureSource::new().with_search("John Doe", vec![with_state, no_state]);
        let out = resolve(&src, &[row("John Doe", None, None, Some("CA"))])
            .await
            .unwrap();
        assert!(out.resolved.is_empty());
        assert_eq!(out.unresolved.len(), 1);
    }
}
