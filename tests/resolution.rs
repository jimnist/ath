//! Name resolution end to end against a fixture-backed search source -
//! confident pins, ambiguous rows reported, athlete_id written back.

use ath::model::Candidate;
use ath::output::write_unresolved_csv;
use ath::resolve::resolve;
use ath::roster::{RosterRow, apply_resolved_ids, read_roster};
use ath::source::FixtureSource;

fn pinned(name: &str, id: i64) -> RosterRow {
    RosterRow {
        name: name.to_string(),
        athlete_id: Some(id),
        school: None,
        city: None,
        state: None,
    }
}

fn name_only(name: &str) -> RosterRow {
    RosterRow {
        name: name.to_string(),
        athlete_id: None,
        school: None,
        city: None,
        state: None,
    }
}

fn cand(id: i64, name: &str, school: &str, score: f64) -> Candidate {
    Candidate {
        athlete_id: id,
        name: name.to_string(),
        school: Some(school.to_string()),
        city: None,
        state: None,
        score,
    }
}

#[tokio::test]
async fn resolves_pins_and_reports_unresolved() -> anyhow::Result<()> {
    let src = FixtureSource::new()
        .with_search(
            "Bob Brooks",
            vec![cand(1002, "Bob Brooks", "Lakeside High", 100.0)],
        )
        .with_search(
            "John Doe",
            vec![
                cand(1, "John Doe", "North HS", 90.0),
                cand(2, "John Doe", "South HS", 88.0),
            ],
        );

    let roster = vec![
        pinned("Ada Pinned", 999),
        name_only("Bob Brooks"), // single hit → auto-resolve
        name_only("John Doe"),   // two hits, no locator → unresolved
    ];

    let out = resolve(&src, &roster).await?;

    // Resolved: the pinned Ada + the single-hit Bob.
    assert_eq!(out.resolved.len(), 2);
    assert!(out.resolved.iter().any(|r| r.athlete_id == Some(999)));
    assert!(out.resolved.iter().any(|r| r.athlete_id == Some(1002)));

    // Only Bob was auto-matched (Ada was already pinned).
    assert_eq!(out.auto_matches.len(), 1);
    assert_eq!(out.auto_matches[0].athlete_id, 1002);

    // John Doe is ambiguous → unresolved with both candidates.
    assert_eq!(out.unresolved.len(), 1);
    assert_eq!(out.unresolved[0].input.name, "John Doe");
    assert_eq!(out.unresolved[0].candidates.len(), 2);

    let dir = tempfile::tempdir()?;

    // Write-back patches the existing file in place. It must fill Bob's id, leave
    // John's blank, and PRESERVE the user's extra `notes` column.
    let roster_path = dir.path().join("roster.csv");
    std::fs::write(
        &roster_path,
        "name,athlete_id,school,city,state,notes\n\
         Ada Pinned,999,,,,keep-ada\n\
         Bob Brooks,,,,,vip\n\
         John Doe,,,,,\n",
    )?;
    apply_resolved_ids(&roster_path, &out.updated_roster)?;

    let text = std::fs::read_to_string(&roster_path)?;
    assert!(text.contains("notes"), "extra header preserved");
    assert!(
        text.contains("keep-ada") && text.contains("vip"),
        "extra data preserved"
    );

    let reread = read_roster(&roster_path)?;
    assert_eq!(
        reread
            .iter()
            .find(|r| r.name == "Bob Brooks")
            .unwrap()
            .athlete_id,
        Some(1002)
    );
    assert_eq!(
        reread
            .iter()
            .find(|r| r.name == "John Doe")
            .unwrap()
            .athlete_id,
        None
    );

    // Unresolved CSV: header + one long-form row per candidate.
    let unresolved_path = dir.path().join("roster.unresolved.csv");
    write_unresolved_csv(&unresolved_path, &out.unresolved)?;
    let text = std::fs::read_to_string(&unresolved_path)?;
    let mut lines = text.lines();
    assert!(
        lines
            .next()
            .unwrap()
            .starts_with("input_name,input_school,input_city,input_state,cand_rank")
    );
    assert_eq!(text.lines().count(), 3); // header + 2 candidates
    Ok(())
}
