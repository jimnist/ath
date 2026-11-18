//! `ath colleges`, an Athlete's Personal record against college teams'
//! season bests, end to end through the fixture-backed data source and the
//! `ath` binary.
//!
//! The subject is the anonymized Jamie Uddumcombe (`982389051.json`, 400m
//! Personal record 58.98, clock pinned to 2099-10-30). The college teams are
//! made up (`teamrecords/` and `teamcore/`, ids 96000000x, invented people):
//!
//! - Harrowgate (960000001): 2099 has five FAT women's 400m marks, three
//!   faster and one 58.98 (so T4), plus a faster hand time, a men's 400m, an
//!   800m, a hurdles race and a relay that do not count; 2098 has two (#2).
//! - Wrenmoor State (960000002): 2099 has nine (#8); 2098 only a men's mark.
//! - Ostrander Tech (960000003): 2099 one slower mark (#1); 2098 one faster
//!   FAT mark and a hand time (#2).

mod common;

use std::collections::BTreeSet;
use std::process::Output;

use ath::college::{CollegeQuery, CollegeStatus, Colleges, colleges};
use ath::source::FixtureSource;
use chrono::NaiveDate;

const FIXTURES: &str = "tests/fixtures";
const SUBJECT: i64 = 982_389_051;
const TODAY: &str = "2099-10-30";
const HARROWGATE: i64 = 960_000_001;
const WRENMOOR: i64 = 960_000_002;
const OSTRANDER: i64 = 960_000_003;
const TEAMS: [i64; 3] = [HARROWGATE, WRENMOOR, OSTRANDER];

fn today() -> NaiveDate {
    NaiveDate::parse_from_str(TODAY, "%Y-%m-%d").unwrap()
}

async fn run(event: &str, teams: &[i64], seasons: &[i64]) -> anyhow::Result<Colleges> {
    let source = FixtureSource::from_dir(FIXTURES)?;
    colleges(
        &source,
        &CollegeQuery {
            athlete_id: SUBJECT,
            event,
            teams,
            seasons,
            today: today(),
        },
    )
    .await
}

/// `(team id, season, label, marks, ahead)` for every result.
fn summary(r: &Colleges) -> Vec<(i64, i64, String, usize, usize)> {
    r.results
        .iter()
        .map(|x| {
            (
                x.team.id,
                x.season,
                x.place
                    .as_ref()
                    .map_or_else(String::new, |p| p.label.clone()),
                x.marks,
                x.ahead.len(),
            )
        })
        .collect()
}

#[tokio::test]
async fn ranks_her_personal_record_against_each_team_and_season() -> anyhow::Result<()> {
    let r = run("400m", &TEAMS, &[2099, 2098]).await?;
    assert_eq!(r.status, CollegeStatus::Ok);
    let mark = r.mark.as_ref().unwrap();
    assert_eq!((mark.mark.as_str(), mark.sort_int), ("58.98", 58_980));
    assert_eq!(r.event_name.as_deref(), Some("400 Meters"));
    assert_eq!(
        summary(&r),
        [
            (HARROWGATE, 2099, "T4".into(), 5, 3),
            (HARROWGATE, 2098, "2".into(), 2, 1),
            (WRENMOOR, 2099, "8".into(), 9, 7),
            (WRENMOOR, 2098, "1".into(), 0, 0),
            (OSTRANDER, 2099, "1".into(), 1, 0),
            (OSTRANDER, 2098, "2".into(), 1, 1),
        ]
    );
    let first = &r.results[0];
    assert_eq!(first.team.name.as_deref(), Some("Harrowgate"));
    assert_eq!(first.team.level, Some(8));
    assert_eq!(first.tied.len(), 1);
    assert_eq!(first.tied[0].mark, "58.98");
    // Best first; the hand time (57.0h) is not among them.
    assert_eq!(
        first
            .ahead
            .iter()
            .map(|a| a.mark.as_str())
            .collect::<Vec<_>>(),
        ["53.41", "55.02", "56.10"]
    );
    assert_eq!(
        first.url,
        "https://www.athletic.net/team/960000001/track-and-field-outdoor/2099"
    );
    assert!(r.note.is_none());
    Ok(())
}

#[tokio::test]
async fn the_event_is_matched_by_its_full_name_too() -> anyhow::Result<()> {
    let by_name = run("400 Meters", &[HARROWGATE], &[2099]).await?;
    assert_eq!(summary(&by_name)[0].2, "T4");
    Ok(())
}

#[tokio::test]
async fn no_personal_record_is_a_report_not_an_error() -> anyhow::Result<()> {
    let r = run("3200m", &[HARROWGATE], &[2099]).await?;
    assert_eq!(r.status, CollegeStatus::NoMark);
    assert!(r.mark.is_none());
    assert!(
        r.note
            .as_deref()
            .unwrap()
            .contains("no 3200m Personal record")
    );
    assert_eq!(r.results[0].status, CollegeStatus::NoMark);
    assert!(r.results[0].place.is_none());
    Ok(())
}

#[tokio::test]
async fn a_team_that_cannot_be_read_is_noted_and_the_rest_stand() -> anyhow::Result<()> {
    let r = run("400m", &[HARROWGATE, 7], &[2099]).await?;
    assert_eq!(r.results[0].status, CollegeStatus::Ok);
    let missing = &r.results[1];
    assert_eq!(missing.status, CollegeStatus::LookupFailed);
    assert!(
        missing
            .note
            .as_deref()
            .unwrap()
            .contains("teamrecords/7-2099.json")
    );
    assert!(missing.team.name.is_none());
    assert!(r.note.as_deref().unwrap().contains("team 7's name"));
    Ok(())
}

fn key_paths(v: &serde_json::Value, prefix: &str, out: &mut BTreeSet<String>) {
    match v {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let path = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                out.insert(path.clone());
                key_paths(v, &path, out);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                key_paths(item, &format!("{prefix}[]"), out);
            }
        }
        _ => {}
    }
}

/// rankmap reads this JSON: a renamed or removed field breaks it, so the
/// shape is pinned here. Bump `Colleges::SCHEMA` with any change to it.
#[tokio::test]
async fn the_json_shape_is_pinned() -> anyhow::Result<()> {
    let r = run("400m", &TEAMS, &[2099, 2098]).await?;
    let json: serde_json::Value = serde_json::from_str(&ath::contract::to_json(&r)?)?;
    assert_eq!(json["schema"], 1);
    let mut paths = BTreeSet::new();
    key_paths(&json, "", &mut paths);
    let mut want: BTreeSet<String> = [
        "schema",
        "athlete",
        "athlete.id",
        "athlete.name",
        "athlete.first_name",
        "athlete.url",
        "gender",
        "event",
        "event_name",
        "status",
        "mark",
        "mark.mark",
        "mark.sort_int",
        "mark.season",
        "mark.date",
        "mark.meet",
        "mark.meet_id",
        "seasons",
        "note",
        "results",
        "results[].team",
        "results[].team.id",
        "results[].team.name",
        "results[].team.level",
        "results[].season",
        "results[].url",
        "results[].status",
        "results[].marks",
        "results[].place",
        "results[].place.rank",
        "results[].place.tied",
        "results[].place.label",
        "results[].place.ahead",
        "results[].place.ties",
        "results[].ahead",
        "results[].tied",
        "results[].note",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    for part in ["ahead", "tied"] {
        for key in [
            "athlete_id",
            "name",
            "grade_id",
            "mark",
            "sort_int",
            "meet",
            "meet_id",
            "date",
            "url",
        ] {
            want.insert(format!("results[].{part}[].{key}"));
        }
    }
    assert_eq!(paths, want);
    Ok(())
}

// ---------------------------------------------------------------------------
// The binary

fn ath(args: &[&str]) -> Output {
    common::ath()
        .args(args)
        .env("ATH_TODAY", TODAY)
        .output()
        .expect("running ath")
}

fn args<'a>(teams: &'a str, seasons: &'a str, extra: &[&'a str]) -> Vec<&'a str> {
    let mut a = vec![
        "colleges",
        "982389051",
        "--event",
        "400m",
        "--teams",
        teams,
        "--seasons",
        seasons,
        "--fixtures-dir",
        FIXTURES,
    ];
    a.extend_from_slice(extra);
    a
}

#[test]
fn ath_colleges_json_is_the_report() -> anyhow::Result<()> {
    let out = ath(&args(
        "960000001,960000002,960000003",
        "2099,2098",
        &["--json"],
    ));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rt = tokio::runtime::Runtime::new()?;
    let want = ath::contract::to_json(&rt.block_on(run("400m", &TEAMS, &[2099, 2098]))?)?;
    assert_eq!(String::from_utf8(out.stdout)?, format!("{want}\n"));
    Ok(())
}

#[test]
fn ath_colleges_prints_a_table() -> anyhow::Result<()> {
    let out = ath(&args("960000001,960000002", "2099,2098", &["--table"]));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout)?;
    assert!(text.contains("58.98 (Personal record, 2099)"), "{text}");
    assert!(text.contains("Harrowgate      2099    T4"), "{text}");
    assert!(text.contains("Wrenmoor State  2098    no marks"), "{text}");
    Ok(())
}

#[test]
fn ath_colleges_fails_when_no_team_can_be_read() {
    let out = ath(&args("7,8", "2099", &["--json"]));
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("none of the 2 team(s) could be read"), "{err}");
}

/// rankmap's tests read saved `ath colleges` reports, one per season as
/// rankmap saves them, so a change to the JSON fails here until they are
/// refreshed (`ATH_BLESS=1 cargo test`).
#[test]
fn rankmaps_saved_reports_are_current() -> anyhow::Result<()> {
    for season in ["2099", "2098"] {
        let path = format!("tools/rankmap/testdata/ath-colleges-{season}.json");
        let out = ath(&args("960000001,960000002,960000003", season, &["--json"]));
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        if std::env::var_os("ATH_BLESS").is_some() {
            std::fs::write(&path, &out.stdout)?;
        }
        assert!(
            std::fs::read(&path)? == out.stdout,
            "{path} is not what `ath colleges` prints now; refresh it with ATH_BLESS=1 cargo test"
        );
    }
    Ok(())
}
