//! `ath areas` and a Profile's Area, end to end through the
//! fixture-backed data source and the `ath` binary.
//!
//! Recorded-shape fixtures (`tests/fixtures/`):
//!
//! - `navinfo/2026.json`: a made-up 2026 `GetNavInfo` tree: the High School
//!   level, California and its ten CIF sections (by their real names, with
//!   made-up list ids), New York, Oregon and Texas (and their children).
//! - `teamnav/5103-2026.json`: made-up Seacliff Prep's 2026 division chain (in
//!   the Central Coast Section via a league, though in San Francisco).
//!   `5103-2025.json` and `5103-2027.json` are the same school on no lists.
//! - `teamnav/9201-2026.json`: a synthetic club whose chain points at the San
//!   Francisco Section, a trap: a club must never decide the Area.
//! - `teamnav/601-*.json`: Profile 2001's synthetic Oregon school.
//! - Synthetic people: bios `3001` (school 5103 through 2026, club 9201),
//!   `3002` (its own team is the club; its high school 5103 is listed only
//!   through 2025), `3003` (club only) and `3004` (a transfer: 5103 through
//!   2025, then 601 in 2026) and `3005` (a middle school then high school
//!   5103, told apart only by the bio's `grades` map: the middle school has
//!   no `Year`, and the club is graded too).
//! - Bio `1001`'s school (501) has no TeamNav recording, so its Area lookup
//!   fails.

mod common;

use std::process::Output;
use std::time::Duration;

use anyhow::anyhow;
use async_trait::async_trait;
use ath::area::{AreaStatus, Areas, AreasQuery, DEFAULT_FROM_LIST, Gender, areas};
use ath::athletic::{AthleticNet, RetryConfig};
use ath::contract::{Format, Report, emit, to_json};
use ath::http::{FetchResponse, Fetcher, Request};
use ath::profile::{Profile, ProfileQuery, profile};
use ath::source::FixtureSource;
use chrono::NaiveDate;

const FIXTURES: &str = "tests/fixtures";

fn source() -> FixtureSource {
    FixtureSource::from_dir(FIXTURES).unwrap()
}

fn query(state: &str, season: i64) -> AreasQuery<'_> {
    AreasQuery {
        state,
        season,
        gender: Gender::F,
        event: "100m".into(),
        from_list: DEFAULT_FROM_LIST,
    }
}

async fn ca_2026() -> anyhow::Result<Areas> {
    areas(&source(), &query("CA", 2026)).await
}

const SECTIONS: [(&str, i64); 10] = [
    ("Central", 900501),
    ("Central Coast", 900502),
    ("Los Angeles", 900503),
    ("North Coast", 900504),
    ("Northern", 900505),
    ("Oakland", 900506),
    ("Sac-Joaquin", 900507),
    ("San Diego", 900508),
    ("San Francisco", 900509),
    ("Southern", 900510),
];

#[tokio::test]
async fn california_2026_lists_the_ten_cif_sections() -> anyhow::Result<()> {
    let a = ca_2026().await?;
    assert_eq!((a.state.as_str(), a.season), ("CA", 2026));
    assert_eq!(a.level.as_deref(), Some("Section"));
    assert_eq!(
        (a.state_list.id, a.state_list.name.as_str()),
        (900500, "California")
    );
    let got: Vec<(&str, i64)> = a.areas.iter().map(|l| (l.name.as_str(), l.id)).collect();
    assert_eq!(got, SECTIONS);
    let ccs = a.areas.iter().find(|l| l.name == "Central Coast").unwrap();
    assert_eq!(
        ccs.url,
        "https://www.athletic.net/TrackAndField/rankings/list/900502/f/100m"
    );
    assert!(a.note.is_none());
    Ok(())
}

#[tokio::test]
async fn state_code_case_gender_and_event_shape_the_links() -> anyhow::Result<()> {
    let q = AreasQuery {
        gender: Gender::M,
        event: "400m".into(),
        ..query(" ca ", 2026)
    };
    let a = areas(&source(), &q).await?;
    assert_eq!((a.state.as_str(), a.event.as_str()), ("CA", "400m"));
    assert_eq!(
        a.state_list.url,
        "https://www.athletic.net/TrackAndField/rankings/list/900500/m/400m"
    );
    Ok(())
}

#[tokio::test]
async fn a_season_athletic_net_does_not_have_is_no_such_season() {
    let err = areas(&source(), &query("CA", 2027)).await.unwrap_err();
    assert!(err.to_string().contains("no 2027 season yet"), "{err:#}");
}

#[tokio::test]
async fn an_unconfigured_state_lists_no_areas_and_says_so() -> anyhow::Result<()> {
    // New York's level below the State is an `Association`, Texas' a
    // `League`: neither is configured as an Area level.
    for (state, list) in [("NY", 900100), ("TX", 900300)] {
        let a = areas(&source(), &query(state, 2026)).await?;
        assert_eq!(a.state_list.id, list);
        assert!(a.level.is_none() && a.areas.is_empty(), "{state}");
        let note = a.note.unwrap();
        assert!(
            note.contains(&format!("No Area level is configured for {state}")),
            "{note}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn an_unknown_state_or_bad_code_is_an_error() {
    let err = areas(&source(), &query("ZZ", 2026)).await.unwrap_err();
    assert_eq!(
        err.to_string(),
        "Athletic.net's 2026 high school lists have no State ZZ"
    );
    // The recorded tree is only part of the national one (four States), so a
    // real State it lacks may exist: say so.
    let err = areas(&source(), &query("WA", 2026)).await.unwrap_err();
    assert!(
        err.to_string()
            .contains("no State WA (Athletic.net answered with only part of the national"),
        "{err:#}"
    );
    let err = areas(&source(), &query("California", 2026))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("two-letter"), "{err:#}");
}

/// The JSON contract (ADR 0007), pinned.
#[tokio::test]
async fn areas_json_shape_is_pinned() -> anyhow::Result<()> {
    assert_eq!(Areas::SCHEMA, 1);
    let v: serde_json::Value = serde_json::from_str(&to_json(&ca_2026().await?)?)?;
    let url = |id: i64| format!("https://www.athletic.net/TrackAndField/rankings/list/{id}/f/100m");
    let areas: Vec<serde_json::Value> = SECTIONS
        .iter()
        .map(|&(name, id)| serde_json::json!({"id": id, "name": name, "url": url(id)}))
        .collect();
    let want = serde_json::json!({
        "schema": 1,
        "state": "CA",
        "season": 2026,
        "gender": "f",
        "event": "100m",
        "level": "Section",
        "state_list": {"id": 900500, "name": "California", "url": url(900500)},
        "areas": areas,
        "note": null,
    });
    assert_eq!(v, want);
    let text = to_json(&ca_2026().await?)?;
    assert!(
        text.starts_with("{\n  \"schema\": 1,\n  \"state\": \"CA\","),
        "{text}"
    );
    Ok(())
}

#[tokio::test]
async fn areas_table_is_readable() -> anyhow::Result<()> {
    let mut out = Vec::new();
    emit(&ca_2026().await?, Format::Table, &mut out)?;
    let out = String::from_utf8(out)?;
    let want_head = "\
California (CA) 2026 - Area level: Section

List           Kind   Id      URL
California     state  900500  https://www.athletic.net/TrackAndField/rankings/list/900500/f/100m
Central        area   900501  https://www.athletic.net/TrackAndField/rankings/list/900501/f/100m
Central Coast  area   900502  https://www.athletic.net/TrackAndField/rankings/list/900502/f/100m
";
    assert!(out.starts_with(want_head), "{out}");
    assert_eq!(out.lines().count(), 3 + 1 + 10, "{out}");
    Ok(())
}

// ---------------------------------------------------------------------------
// A Profile's Area

const TODAY: &str = "2026-10-26";

fn profile_query(event: Option<&str>, season: Option<i64>) -> ProfileQuery<'_> {
    ProfileQuery {
        event,
        as_of: None,
        season,
        today: NaiveDate::parse_from_str(TODAY, "%Y-%m-%d").unwrap(),
    }
}

async fn profile_of(athlete_id: i64, season: Option<i64>) -> anyhow::Result<Profile> {
    profile(&source(), athlete_id, &profile_query(None, season)).await
}

#[tokio::test]
async fn a_school_5103_athlete_is_in_the_central_coast_section() -> anyhow::Result<()> {
    let p = profile_of(3001, None).await?;
    let area = p.area.unwrap();
    assert_eq!(
        (area.id, area.name.as_str(), area.season),
        (900502, "Central Coast", 2026)
    );
    assert_eq!(
        (area.state.as_str(), area.level.as_str()),
        ("CA", "Section")
    );
    assert_eq!(
        area.url,
        "https://www.athletic.net/TrackAndField/rankings/list/900502/f/100m"
    );
    assert_eq!(p.area_lookup.status, AreaStatus::Ok);
    assert_eq!(p.area_lookup.school_id, Some(5103));
    assert!(p.area_lookup.note.is_none());
    Ok(())
}

#[tokio::test]
async fn the_area_link_opens_on_a_flat_race_event() -> anyhow::Result<()> {
    for (event, short) in [
        ("400m", "400m"),
        ("400 Meters", "400m"),
        ("Long Jump", "100m"),
    ] {
        let p = profile(&source(), 3001, &profile_query(Some(event), None)).await?;
        assert!(
            p.area.unwrap().url.ends_with(&format!("/900502/f/{short}")),
            "{event}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_club_never_decides_the_area() -> anyhow::Result<()> {
    // 3002's own team is the club, whose chain points at San Francisco; the
    // Area still comes from their high school.
    let p = profile_of(3002, Some(2025)).await?;
    assert_eq!(p.area_lookup.school_id, Some(5103));

    // With only a club there is no Area at all.
    let p = profile_of(3003, None).await?;
    assert!(p.area.is_none());
    assert_eq!(p.area_lookup.status, AreaStatus::NoHighSchool);
    assert_eq!(p.area_lookup.school_id, None);
    assert!(p.school.is_none());
    Ok(())
}

#[tokio::test]
async fn a_school_from_an_earlier_season_is_never_used_silently() -> anyhow::Result<()> {
    // 3002's high school is listed only through 2025: its 2026 Area comes
    // from that school, and the note says so.
    let p = profile_of(3002, None).await?;
    let area = p.area.clone().unwrap();
    assert_eq!((area.id, area.season), (900502, 2026));
    assert!(area.url.ends_with("/900502/m/100m"), "the Profile's gender");
    assert_eq!(p.area_lookup.status, AreaStatus::Ok);
    assert_eq!(
        p.area_lookup.note.as_deref(),
        Some(
            "no school on this Profile is listed for 2026; used Seacliff Prep \
             (last listed 2025)"
        )
    );
    let mut out = Vec::new();
    emit(&p, Format::Table, &mut out)?;
    let out = String::from_utf8(out)?;
    assert!(
        out.contains("\n         (no school on this Profile is listed for 2026; "),
        "{out}"
    );
    Ok(())
}

#[tokio::test]
async fn a_transfer_gets_the_school_of_the_season() -> anyhow::Result<()> {
    // 2026: the new school (Oregon), on both the School row and the Area.
    let p = profile_of(3004, None).await?;
    assert_eq!(p.school.as_ref().unwrap().id, 601);
    assert_eq!(p.area_lookup.school_id, Some(601));
    assert_eq!(p.area_lookup.status, AreaStatus::StateNotConfigured);
    assert!(
        p.area_lookup
            .note
            .as_deref()
            .unwrap()
            .starts_with("OR has no Area")
    );

    // 2025: the school before the transfer.
    let p = profile_of(3004, Some(2025)).await?;
    assert_eq!(p.school.as_ref().unwrap().id, 5103);
    assert_eq!(p.area_lookup.school_id, Some(5103));
    assert_eq!(p.area_lookup.status, AreaStatus::NotListed);
    Ok(())
}

#[tokio::test]
async fn grades_tell_middle_school_from_high_school() -> anyhow::Result<()> {
    // 2025: the middle school, so no Area (and no request for one).
    let p = profile_of(3005, Some(2025)).await?;
    let school = p.school.as_ref().unwrap();
    assert_eq!((school.id, school.level), (701, 2));
    assert!(p.area.is_none());
    assert_eq!(p.area_lookup.status, AreaStatus::NoHighSchool);
    assert_eq!(p.area_lookup.school_id, None);
    assert_eq!(
        p.area_lookup.note.as_deref(),
        Some("no high school on this Profile for 2025 (a club never decides the Area)")
    );
    // 2026: the high school, whose Area it is.
    let p = profile_of(3005, Some(2026)).await?;
    assert_eq!(p.school.as_ref().unwrap().id, 5103);
    assert_eq!(p.area.unwrap().name, "Central Coast");
    assert!(p.area_lookup.note.is_none());
    Ok(())
}

#[tokio::test]
async fn a_season_before_every_school_is_a_guess_and_says_so() -> anyhow::Result<()> {
    // 2001: Harbor High through 2026, Bayview Middle through 2024, no grades.
    let p = profile_of(2001, Some(2023)).await?;
    assert_eq!(p.school.as_ref().unwrap().id, 602);
    assert_eq!(p.area_lookup.status, AreaStatus::NoHighSchool);
    assert_eq!(
        p.area_lookup.note.as_deref(),
        Some(
            "no high school on this Profile for 2023 (a club never decides the Area); no \
             school on this Profile is listed for 2023, which is before every school's last \
             listed season; guessed Bayview Middle (last listed 2024)"
        )
    );
    assert!(p.area_lookup.school_not_listed);
    Ok(())
}

#[tokio::test]
async fn a_school_in_an_unconfigured_state_has_a_null_area_with_a_reason() -> anyhow::Result<()> {
    let p = profile_of(2001, None).await?;
    assert!(p.area.is_none());
    let l = &p.area_lookup;
    assert_eq!(l.status, AreaStatus::StateNotConfigured);
    assert_eq!((l.school_id, l.state.as_deref()), (Some(601), Some("OR")));
    assert_eq!(
        l.note.as_deref(),
        Some("OR has no Area level configured in ath")
    );
    Ok(())
}

#[tokio::test]
async fn a_school_on_no_lists_for_the_season_has_no_area() -> anyhow::Result<()> {
    let p = profile_of(3001, Some(2027)).await?;
    assert!(p.area.is_none());
    assert_eq!(p.area_lookup.status, AreaStatus::NotListed);
    assert_eq!(
        p.area_lookup.note.as_deref(),
        Some(
            "the school is on no Athletic.net lists for 2027 (Athletic.net may not have the \
             season yet); no school on this Profile is listed for 2027; used Seacliff Prep \
             (last listed 2026)"
        )
    );
    Ok(())
}

#[tokio::test]
async fn a_failed_lookup_keeps_the_rest_of_the_profile() -> anyhow::Result<()> {
    // No TeamNav recording for 1001's school.
    let p = profile_of(1001, None).await?;
    assert!(p.area.is_none());
    assert_eq!(p.area_lookup.status, AreaStatus::LookupFailed);
    assert_eq!(p.area_lookup.school_id, Some(501));
    assert_eq!(
        p.area_lookup.note.as_deref(),
        Some("no fixture TeamNav for school 501 in 2026")
    );
    assert_eq!(p.school.unwrap().id, 501);
    Ok(())
}

/// Serves 3001's bio and answers TeamNav with a server error page.
struct TeamNavDown;

#[async_trait]
impl Fetcher for TeamNavDown {
    async fn fetch(&self, req: &Request) -> anyhow::Result<FetchResponse> {
        if req.url().contains("/AthleteBio/") {
            return Ok(FetchResponse {
                status: 200,
                body: std::fs::read_to_string(format!("{FIXTURES}/3001.json"))?,
            });
        }
        if req.url().contains("/TeamNav/") {
            return Ok(FetchResponse {
                status: 500,
                body: "<html>Server Error</html>".into(),
            });
        }
        Err(anyhow!("unexpected request {req}"))
    }

    fn challenge_advice(&self) -> &'static str {
        "n/a"
    }
}

#[tokio::test]
async fn a_live_lookup_error_is_a_lookup_failed_status() -> anyhow::Result<()> {
    let src = AthleticNet::new(TeamNavDown).with_retry(RetryConfig {
        max_retries: 0,
        base_delay: Duration::ZERO,
    });
    let p = profile(&src, 3001, &profile_query(None, None)).await?;
    assert_eq!(p.name, "Casey Example");
    assert!(p.area.is_none());
    assert_eq!(p.area_lookup.status, AreaStatus::LookupFailed);
    let note = p.area_lookup.note.unwrap();
    assert!(
        note.contains("parsing divisions of school 5103 in 2026"),
        "{note}"
    );
    Ok(())
}

#[tokio::test]
async fn profile_area_json_and_table() -> anyhow::Result<()> {
    let p = profile_of(3001, None).await?;
    let v: serde_json::Value = serde_json::from_str(&to_json(&p)?)?;
    assert_eq!(
        v["area"],
        serde_json::json!({
            "id": 900502,
            "name": "Central Coast",
            "level": "Section",
            "state": "CA",
            "season": 2026,
            "url": "https://www.athletic.net/TrackAndField/rankings/list/900502/f/100m",
        })
    );
    assert_eq!(
        v["area_lookup"],
        serde_json::json!({
            "status": "ok", "season": 2026, "school_id": 5103, "state": "CA", "note": null,
        })
    );
    let mut out = Vec::new();
    emit(&p, Format::Table, &mut out)?;
    let out = String::from_utf8(out)?;
    assert!(
        out.contains(
            "Area     Central Coast (CA Section, 2026)  \
             https://www.athletic.net/TrackAndField/rankings/list/900502/f/100m\n"
        ),
        "{out}"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// The binary

fn ath(args: &[&str]) -> Output {
    common::ath()
        .args(args)
        .env_remove("ATH_COOKIE")
        .output()
        .expect("running ath")
}

#[test]
fn ath_areas_prints_the_sections_as_json() -> anyhow::Result<()> {
    let out = ath(&[
        "areas",
        "CA",
        "--season",
        "2026",
        "--fixtures-dir",
        FIXTURES,
    ]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    let rt = tokio::runtime::Runtime::new()?;
    let want = to_json(&rt.block_on(ca_2026())?)?;
    assert_eq!(String::from_utf8(out.stdout)?, format!("{want}\n"));
    Ok(())
}

#[test]
fn ath_areas_table_flag_gender_and_event() -> anyhow::Result<()> {
    let out = ath(&[
        "areas",
        "ca",
        "--season",
        "2026",
        "--gender",
        "m",
        "--event",
        "400 Meters",
        "--table",
        "--fixtures-dir",
        FIXTURES,
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout)?;
    assert!(
        stdout.contains(
            "Central Coast  area   900502  \
             https://www.athletic.net/TrackAndField/rankings/list/900502/m/400m"
        ),
        "{stdout}"
    );
    Ok(())
}

#[test]
fn ath_areas_rejects_an_event_that_is_not_a_link_event() {
    let out = ath(&[
        "areas",
        "CA",
        "--event",
        "Long Jump",
        "--fixtures-dir",
        FIXTURES,
    ]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--event takes a flat race"), "{stderr}");
}

#[test]
fn ath_areas_rejects_a_bare_number_event_with_a_hint() {
    let out = ath(&["areas", "CA", "--event", "400", "--fixtures-dir", FIXTURES]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("did you mean 400m?"), "{stderr}");
    assert!(out.stdout.is_empty());
}

#[test]
fn ath_areas_unknown_season_fails_clearly() {
    let out = ath(&[
        "areas",
        "CA",
        "--season",
        "2027",
        "--fixtures-dir",
        FIXTURES,
    ]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Athletic.net has no 2027 season yet"),
        "{stderr}"
    );
    assert!(out.stdout.is_empty());
}

#[test]
fn ath_areas_unconfigured_state_says_so_once() -> anyhow::Result<()> {
    let args = [
        "areas",
        "NY",
        "--season",
        "2026",
        "--fixtures-dir",
        FIXTURES,
    ];
    // JSON: the note is in the JSON and, for people, on stderr.
    let out = ath(&args);
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("No Area level is configured for NY"),
        "{stderr}"
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    assert_eq!(v["areas"], serde_json::json!([]));
    assert!(v["level"].is_null());

    // Table: the table carries it, so stderr stays quiet.
    let mut table: Vec<&str> = args.to_vec();
    table.push("--table");
    let out = ath(&table);
    assert!(out.status.success());
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout)?;
    assert_eq!(
        stdout.matches("No Area level is configured").count(),
        1,
        "{stdout}"
    );
    Ok(())
}

#[test]
fn ath_areas_rejects_a_bad_gender() {
    let out = ath(&["areas", "CA", "--gender", "x", "--fixtures-dir", FIXTURES]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn ath_profile_shows_the_area() -> anyhow::Result<()> {
    // --season pins the season (2026) whatever today is; no --event needed.
    let out = ath(&[
        "profile",
        "3001",
        "--season",
        "2026",
        "--fixtures-dir",
        FIXTURES,
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    assert_eq!(v["area"]["name"], "Central Coast");
    assert_eq!(v["area"]["id"], 900502);
    assert!(v["event"].is_null());
    Ok(())
}

#[test]
fn ath_profile_with_a_failed_area_lookup_still_prints_and_warns() -> anyhow::Result<()> {
    let out = ath(&[
        "profile",
        "1001",
        "--season",
        "2026",
        "--fixtures-dir",
        FIXTURES,
    ]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(
        stderr.contains("ath: warning - Area: no fixture TeamNav for school 501 in 2026"),
        "{stderr}"
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    assert_eq!(v["area_lookup"]["status"], "lookup_failed");
    assert_eq!(v["school"]["id"], 501);
    Ok(())
}

#[test]
fn ath_profile_warns_about_a_school_from_another_season_whatever_the_area() -> anyhow::Result<()> {
    // 2027: 3001's school is listed only through 2026, and it is on no lists
    // for 2027, so the Area is null; the JSON reader is still warned.
    let out = ath(&[
        "profile",
        "3001",
        "--season",
        "2027",
        "--fixtures-dir",
        FIXTURES,
    ]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    assert!(v["area"].is_null());
    assert_eq!(v["area_lookup"]["status"], "not_listed");
    assert!(
        stderr.contains("ath: warning - Area: the school is on no Athletic.net lists for 2027")
            && stderr.contains("no school on this Profile is listed for 2027"),
        "{stderr}"
    );
    // The flag behind the warning is not part of the JSON.
    assert!(v["area_lookup"].get("school_not_listed").is_none());

    // A school listed for the season, with no Area: nothing to warn about.
    let out = ath(&[
        "profile",
        "3005",
        "--season",
        "2025",
        "--fixtures-dir",
        FIXTURES,
    ]);
    assert!(out.status.success());
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(())
}
