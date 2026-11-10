//! End-to-end pipeline test: roster + fixture-backed data source → seed sheet.
//! Drives the whole pipeline through the `DataSource` seam, no network.

use ath::area::Gender;
use ath::contract::{Format, emit};
use ath::meet::{EventSelect, MeetRef, read_event, relay_seeding, seedable};
use ath::output::{relay_csv_string, seed_csv_string};
use ath::resolve::resolve;
use ath::roster::read_roster;
use ath::seed::{FetchOpts, SeedDate, SeedSheet, SheetMeet, not_resolved, seed};
use ath::seed_relay::{RelaySheet, seed_relay};
use ath::source::{DataSource, FixtureSource};
use chrono::NaiveDate;

fn day(s: &str) -> NaiveDate {
    s.parse().unwrap()
}

const TODAY: &str = "2026-10-26";

#[tokio::test]
async fn seeds_pinned_roster_400m() -> anyhow::Result<()> {
    let source = FixtureSource::from_dir("tests/fixtures")?;
    let roster = read_roster("tests/roster_400.csv")?;

    let outcome = seed(
        &source,
        &roster,
        "400 Meters",
        SeedDate::new(None, day(TODAY))?,
        &FetchOpts::default(),
    )
    .await?;
    let rows = outcome.rows;

    assert!(
        outcome.failures.is_empty(),
        "no fetch failures with fixtures"
    );
    assert_eq!(rows.len(), 3);

    // Ranked fastest-first by SortInt; no-mark athletes last.
    // Bob 55.20 < Alice 57.00 < Cara (no 400m mark ever).
    assert_eq!(rows[0].rank, 1);
    assert_eq!(rows[0].athlete, "Bob Brooks");
    assert_eq!(rows[0].pr, "55.20");
    assert_eq!(rows[0].pr_season, Some(2026));
    assert_eq!(rows[0].school, "Lakeside High");
    assert_eq!(rows[0].club, ""); // no club affiliation
    assert!((rows[0].pr_numeric.unwrap() - 55.2).abs() < 1e-9);

    // Alice is seeded on her Personal record, 57.00 from 2025 (before seeding
    // moved to the Personal record she was seeded on her 2026 Season best,
    // 58.24). It survives a faster relay split (53.62, EventTypeID 98), an
    // exhibition (54.00), a hand-time (56.50h), a DNS, and relay-team noise,
    // none of which ever count.
    assert_eq!(rows[1].rank, 2);
    assert_eq!(rows[1].athlete, "Alice Anders");
    assert_eq!(rows[1].pr, "57.00");
    assert_eq!(rows[1].pr_season, Some(2025));
    assert_eq!(rows[1].school, "Riverside High");
    assert_eq!(rows[1].club, "Velocity Track Club");

    // Cara has no qualifying 400m mark in any season → blank, sorted last.
    assert_eq!(rows[2].rank, 3);
    assert_eq!(rows[2].athlete, "Cara Cole");
    assert_eq!(rows[2].pr, "");
    assert!(rows[2].pr_numeric.is_none());
    assert_eq!(rows[2].pr_season, None);
    assert_eq!(rows[2].school, "Hillcrest High");

    // The CSV renders with the agreed header + one line per row; `pr_season`
    // is appended last so the earlier columns keep their positions.
    let csv = seed_csv_string(&rows)?;
    let mut lines = csv.lines();
    assert_eq!(
        lines.next().unwrap(),
        "rank,athlete,athlete_id,athlete_url,pr,pr_numeric,school,school_url,club,club_url,pr_season"
    );
    assert_eq!(
        lines.next().unwrap(),
        "1,Bob Brooks,1002,https://www.athletic.net/athlete/1002/track-and-field,55.20,55.2,\
         Lakeside High,https://www.athletic.net/team/502/track-and-field-outdoor,,,2026"
    );
    assert_eq!(csv.lines().count(), 4); // header + 3 rows
    Ok(())
}

/// Seed `tests/roster_pr.csv` (Erin, Finn) on the 400m as of `today`, with an
/// optional meet date: `(athlete, pr, pr_season)` in rank order.
async fn seed_pr(
    today: &str,
    meet_date: Option<&str>,
) -> anyhow::Result<Vec<(String, String, Option<i64>)>> {
    let source = FixtureSource::from_dir("tests/fixtures")?;
    let roster = read_roster("tests/roster_pr.csv")?;
    let date = SeedDate::new(meet_date.map(day), day(today))?;
    let outcome = seed(&source, &roster, "400m", date, &FetchOpts::default()).await?;
    Ok(outcome
        .rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            assert_eq!(r.rank, i + 1);
            (r.athlete.clone(), r.pr.clone(), r.pr_season)
        })
        .collect())
}

fn row(athlete: &str, pr: &str, season: i64) -> (String, String, Option<i64>) {
    (athlete.into(), pr.into(), Some(season))
}

/// The Seed mark is the Personal record, not the Season best. Erin ran
/// 54.50 in 2025 but only 56.00 this season; Finn ran 55.00 this season and
/// was never faster. On Season best Finn led; on Personal record Erin does.
#[tokio::test]
async fn seeds_on_personal_record_from_an_earlier_season() -> anyhow::Result<()> {
    // Erin's faster 2026 indoor 53.20 (SeasonID 12026), 2024 hand-time
    // (53.00h), 2025 exhibition (53.50) and 2025 relay split (52.90,
    // EventTypeID 98) never count, nor does her DNF.
    assert_eq!(
        seed_pr(TODAY, None).await?,
        [
            row("Erin Ellis", "54.50", 2025),
            row("Finn Foster", "55.00", 2026),
        ]
    );
    Ok(())
}

/// A meet today is seeded as of the day before: Finn's 55.00 is from the meet
/// day (2026-05-09) itself, and his undated 56.00 may be too, so neither
/// counts and his seed is 2025's 57.40.
#[tokio::test]
async fn a_meet_today_excludes_results_from_the_meet_day() -> anyhow::Result<()> {
    assert_eq!(
        seed_pr("2026-05-09", Some("2026-05-09")).await?,
        [
            row("Erin Ellis", "54.50", 2025),
            row("Finn Foster", "57.40", 2025),
        ]
    );
    Ok(())
}

/// A later meet is seeded as of today: Finn's 55.00 (2026-05-09) is still in
/// the future, and his undated 56.00 counts (it can only be from by today).
#[tokio::test]
async fn a_future_meet_seeds_as_of_today() -> anyhow::Result<()> {
    let want = [
        row("Erin Ellis", "54.50", 2025),
        row("Finn Foster", "56.00", 2026),
    ];
    assert_eq!(seed_pr("2026-05-01", Some("2026-06-01")).await?, want);
    assert_eq!(seed_pr("2026-05-01", None).await?, want);
    Ok(())
}

/// The seed sheet page's tests (`tools/seedsheet/`) read a saved `ath seed`
/// report and the seed sheet CSV from the same run, so a change to either
/// fails here until the copies are refreshed (`ATH_BLESS=1 cargo test`), and
/// then fails on the Python side if the page cannot read them
/// (`tests/tools.rs`). Today is pinned, so the report's `as_of` is too. The
/// roster's last row (Ivy Ironwood) matches no Profile, so the report lists
/// it as unresolved, as `ath seed` does.
#[tokio::test]
async fn seedsheets_saved_report_is_current() -> anyhow::Result<()> {
    let source = FixtureSource::from_dir("tests/fixtures")?;
    let roster = read_roster("tests/roster_seedsheet.csv")?;
    let resolution = resolve(&source, &roster).await?;
    let unresolved = not_resolved(&resolution, "roster_seedsheet.unresolved.csv");
    assert_eq!(
        unresolved
            .iter()
            .map(|u| u.name.as_str())
            .collect::<Vec<_>>(),
        ["Ivy Ironwood"]
    );
    let date = SeedDate::new(Some(day("2026-06-13")), day("2026-06-10"))?;
    let outcome = seed(
        &source,
        &resolution.resolved,
        "400m",
        date,
        &FetchOpts::default(),
    )
    .await?;
    let mut json = Vec::new();
    emit(
        &SeedSheet::new("400m", date, &outcome, unresolved),
        Format::Json,
        &mut json,
    )?;
    let csv = seed_csv_string(&outcome.rows)?;
    blessed("tools/seedsheet/testdata/ath-seed.json", json)?;
    blessed("tools/seedsheet/testdata/ath-seed.csv", csv.into_bytes())
}

/// Fail unless the saved copy at `path` is `got`; with `ATH_BLESS` set,
/// write it first.
fn blessed(path: &str, got: Vec<u8>) -> anyhow::Result<()> {
    if std::env::var_os("ATH_BLESS").is_some() {
        std::fs::write(path, &got)?;
    }
    assert!(
        std::fs::read(path)? == got,
        "{path} is not what `ath seed` writes now; refresh it with ATH_BLESS=1 cargo test"
    );
    Ok(())
}

/// The relay sheets the seed sheet page's relay tests read, saved the same
/// way (`ATH_BLESS=1 cargo test` refreshes them), as of a pinned today:
/// `ath seed --meet` on the made-up meet 900700's boys' Varsity 4x800m, and
/// on 900702's girls' Varsity 4xMile (whose runners' miles become Estimated
/// marks when faster).
#[tokio::test]
async fn seedsheets_saved_relay_reports_are_current() -> anyhow::Result<()> {
    saved_relay(900_700, Gender::M, "13", "4x800m", "ath-seed-relay").await?;
    saved_relay(900_702, Gender::F, "21", "4xmile", "ath-seed-4xmile").await
}

async fn saved_relay(
    meet_id: i64,
    gender: Gender,
    division: &str,
    event: &str,
    name: &str,
) -> anyhow::Result<()> {
    let source = FixtureSource::from_dir("tests/fixtures")?;
    let meet = source.meet(meet_id).await?;
    let list = source.meet_events(meet_id).await?;
    let select = EventSelect {
        meet_id,
        gender,
        division,
        event,
    };
    let sel = read_event(&source, meet, &list, &select, seedable).await?;
    // The Seeding event and the mile flag, exactly as `ath seed --meet` gets
    // them.
    let seeding = relay_seeding(&sel.event, &sel.relay_shape, None)?;
    let info = &sel.meet.meet;
    let date = SeedDate::for_meet(
        meet_id,
        info.date().unwrap(),
        info.end().unwrap(),
        day("2026-11-06"),
    )?;
    let outcome = seed_relay(
        &source,
        &sel.entries.teams,
        &seeding.event,
        seeding.mile,
        date,
        &FetchOpts::default(),
    )
    .await?;
    let sheet = RelaySheet::new(
        event,
        &seeding.event,
        date,
        &outcome,
        SheetMeet {
            meet: MeetRef::new(info),
            event: sel.event.clone(),
            left_out: sel.entries.left_out.clone(),
        },
    );
    let mut json = Vec::new();
    emit(&sheet, Format::Json, &mut json)?;
    let csv = relay_csv_string(&outcome.teams)?;
    blessed(&format!("tools/seedsheet/testdata/{name}.json"), json)?;
    blessed(
        &format!("tools/seedsheet/testdata/{name}.csv"),
        csv.into_bytes(),
    )
}
