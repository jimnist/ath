//! `ath profile` end to end through the fixture-backed data source.
//!
//! `tests/fixtures/2001.json` is a synthetic Profile (not a real athlete). Its
//! 400m history, best first among the marks that must never count:
//!
//! | mark      | SortInt  | date       | season | counts?                     |
//! |-----------|----------|------------|--------|-----------------------------|
//! | 54.00a    | 54000    | 2026-05-09 | 2026   | no - a relay team's result  |
//! | 54.80a    | 54800    | 2026-02-14 | 12026  | no - indoor (2026 Indoor)   |
//! | 55.10a    | 55100    | 2026-04-04 | 2026   | no - relay split (98)       |
//! | 55.50a    | 55500    | 2026-04-11 | 2026   | no - exhibition             |
//! | 55.80h    | 55800    | 2026-03-28 | 2026   | no - hand-timed             |
//! | DNF       | 1        | 2026-05-09 | 2026   | no - not a mark             |
//! | DNS       | 20000001 | 2026-05-02 | 2026   | no - not a mark             |
//! | 56.90a    | 56904    | 2025-05-10 | 2025   | yes                         |
//! | 57.10a    | 57100    | 2026-04-18 | 2026   | yes                         |
//! | 58.20a    | 58195    | 2026-03-14 | 2026   | yes                         |
//! | 59.40a    | 59400    | 2025-03-22 | 2025   | yes                         |
//! | 1:01.20a  | 61200    | 2024-05-04 | 2024   | yes                         |
//!
//! It also has a 200m mark with no `ResultDate`, an 800m with only marks that
//! never count, and a Long Jump (a field event).

use ath::contract::{Format, Report, emit, to_json};
use ath::profile::{EventStatus, Profile, ProfileQuery, TeamKind, profile};
use ath::source::FixtureSource;
use chrono::NaiveDate;

fn date(s: &str) -> NaiveDate {
    s.parse().unwrap()
}

/// The "today" these tests run on.
const TODAY: &str = "2026-10-26";

async fn query(q: ProfileQuery<'_>) -> anyhow::Result<Profile> {
    let source = FixtureSource::from_dir("tests/fixtures")?;
    profile(&source, 2001, &q).await
}

/// Profile 2001 with an explicit `--as-of`.
async fn dana(event: Option<&str>, as_of: &str) -> anyhow::Result<Profile> {
    query(ProfileQuery {
        event,
        as_of: Some(date(as_of)),
        season: None,
        today: date(TODAY),
    })
    .await
}

/// Profile 2001 with no `--as-of` (so as of today).
async fn dana_today(event: &str) -> anyhow::Result<Profile> {
    query(ProfileQuery {
        event: Some(event),
        ..ProfileQuery::today(date(TODAY))
    })
    .await
}

/// (mark, date, meet) of a best, for compact assertions.
fn best(m: &Option<ath::profile::MarkSummary>) -> Option<(String, String, String)> {
    m.as_ref().map(|m| {
        (
            m.mark.clone(),
            m.date.unwrap().to_string(),
            m.meet.clone().unwrap_or_default(),
        )
    })
}

fn some(mark: &str, date: &str, meet: &str) -> Option<(String, String, String)> {
    Some((mark.into(), date.into(), meet.into()))
}

#[tokio::test]
async fn name_gender_teams_and_school() -> anyhow::Result<()> {
    let p = query(ProfileQuery::today(date(TODAY))).await?;
    assert_eq!(p.athlete_id, 2001);
    assert_eq!(p.name, "Dana Diaz");
    assert_eq!(p.gender.as_deref(), Some("F"));
    assert_eq!(
        p.url,
        "https://www.athletic.net/athlete/2001/track-and-field"
    );
    let school = p.school.as_ref().unwrap();
    assert_eq!((school.id, school.name.as_str()), (601, "Harbor High"));
    let teams: Vec<_> = p
        .teams
        .iter()
        .map(|t| (t.name.as_str(), t.kind, t.level))
        .collect();
    assert_eq!(
        teams,
        [
            ("Harbor High", TeamKind::School, 4),
            ("Bayview Middle", TeamKind::School, 2),
            ("Coastal Striders TC", TeamKind::Club, 16),
        ]
    );
    assert!(p.event.is_none());
    Ok(())
}

#[tokio::test]
async fn bests_as_of_today() -> anyhow::Result<()> {
    let p = dana_today("400 Meters").await?;
    let e = p.event.unwrap();
    assert_eq!((e.name.as_str(), e.season), ("400 Meters", 2026));
    assert_eq!(e.as_of, date(TODAY));
    assert!(e.on_profile);
    assert_eq!(e.status, EventStatus::Ok);
    assert_eq!(
        best(&e.season_best),
        some("57.10", "2026-04-18", "Harbor Invite")
    );
    // Last season's 56.90 beats everything this season: the Personal record
    // comes from an earlier season.
    assert_eq!(
        best(&e.personal_record),
        some("56.90", "2025-05-10", "League Finals")
    );
    let pr = e.personal_record.unwrap();
    assert_eq!((pr.sort_int, pr.season), (56904, 2025));
    Ok(())
}

/// v1 counts outdoor marks only: an indoor season is `SeasonID` 10000 + year
/// (12026 = "2026 Indoor") with the same `EventID`, and its 54.80 must not
/// become the Personal record or a Season best.
#[tokio::test]
async fn indoor_marks_never_count() -> anyhow::Result<()> {
    let e = dana_today("400 Meters").await?.event.unwrap();
    assert_eq!(best(&e.personal_record).unwrap().0, "56.90");
    let e = query(ProfileQuery {
        event: Some("400 Meters"),
        season: Some(12026),
        ..ProfileQuery::today(date(TODAY))
    })
    .await?
    .event
    .unwrap();
    assert!(e.season_best.is_none());
    assert_eq!(best(&e.personal_record).unwrap().0, "56.90");
    Ok(())
}

#[tokio::test]
async fn bests_as_of_earlier_dates_differ_from_today() -> anyhow::Result<()> {
    // Before Harbor Invite: the Season best is the Spring Opener mark.
    let e = dana(Some("400 Meters"), "2026-04-01").await?.event.unwrap();
    assert_eq!(
        best(&e.season_best),
        some("58.20", "2026-03-14", "Spring Opener")
    );
    assert_eq!(
        best(&e.personal_record),
        some("56.90", "2025-05-10", "League Finals")
    );

    // Before League Finals 2025: both bests are the Early Bird mark, and the
    // as-of date's season (2025) is the Season best's window.
    let e = dana(Some("400 Meters"), "2025-04-30").await?.event.unwrap();
    assert_eq!(e.season, 2025);
    assert_eq!(
        best(&e.season_best),
        some("59.40", "2025-03-22", "Early Bird Invite")
    );
    assert_eq!(
        best(&e.personal_record),
        some("59.40", "2025-03-22", "Early Bird Invite")
    );

    // A mark set on the as-of date counts.
    let e = dana(Some("400 Meters"), "2026-04-18").await?.event.unwrap();
    assert_eq!(best(&e.season_best).unwrap().0, "57.10");
    Ok(())
}

#[tokio::test]
async fn season_with_no_marks_yet_keeps_the_personal_record() -> anyhow::Result<()> {
    let e = dana(Some("400 Meters"), "2027-01-15").await?.event.unwrap();
    assert_eq!(e.season, 2027);
    assert!(e.season_best.is_none());
    assert_eq!(best(&e.personal_record).unwrap().0, "56.90");
    Ok(())
}

#[tokio::test]
async fn short_form_and_case_resolve_to_the_event_name() -> anyhow::Result<()> {
    for query in ["400m", "400 meters", " 400 METERS "] {
        let e = dana(Some(query), "2026-10-26").await?.event.unwrap();
        assert_eq!(e.name, "400 Meters", "{query}");
        assert_eq!(best(&e.season_best).unwrap().0, "57.10", "{query}");
    }
    Ok(())
}

#[tokio::test]
async fn event_not_on_profile_has_no_bests() -> anyhow::Result<()> {
    let e = dana_today("1600 Meters").await?.event.unwrap();
    assert_eq!(e.name, "1600 Meters");
    assert!(!e.on_profile);
    assert_eq!(e.status, EventStatus::NotOnProfile);
    assert!(e.season_best.is_none() && e.personal_record.is_none());
    Ok(())
}

#[tokio::test]
async fn event_with_only_non_qualifying_rows_says_so() -> anyhow::Result<()> {
    // 800m: a relay split, an exhibition, a DNS and a relay team's result.
    let e = dana_today("800m").await?.event.unwrap();
    assert_eq!(e.name, "800 Meters");
    assert!(e.on_profile);
    assert_eq!(e.status, EventStatus::NoQualifyingMark);
    assert!(e.season_best.is_none() && e.personal_record.is_none());

    // 400m before any 400m mark was set: on the Profile, nothing yet.
    let e = dana(Some("400m"), "2024-01-01").await?.event.unwrap();
    assert_eq!(e.status, EventStatus::NoQualifyingMark);
    Ok(())
}

#[tokio::test]
async fn field_event_is_flagged_not_read() -> anyhow::Result<()> {
    let e = dana_today("long jump").await?.event.unwrap();
    assert_eq!(e.name, "Long Jump");
    assert!(e.on_profile);
    assert_eq!(e.status, EventStatus::FieldEvent);
    assert!(e.season_best.is_none() && e.personal_record.is_none());
    Ok(())
}

#[tokio::test]
async fn undated_marks_count_as_of_today_only() -> anyhow::Result<()> {
    // 200m: 26.40 on 2026-04-18 and an undated 25.90 (it can only be from on
    // or before today).
    for e in [
        dana_today("200 Meters").await?.event.unwrap(),
        dana(Some("200 Meters"), TODAY).await?.event.unwrap(),
        dana(Some("200 Meters"), "2026-12-31").await?.event.unwrap(),
    ] {
        let sb = e.season_best.unwrap();
        assert_eq!((sb.mark.as_str(), sb.date), ("25.90", None), "{}", e.as_of);
        assert_eq!(e.personal_record.unwrap().mark, "25.90");
    }
    // An earlier as-of date: the undated mark may be later, so it never counts.
    let e = dana(Some("200 Meters"), "2026-05-01").await?.event.unwrap();
    assert_eq!(
        best(&e.season_best),
        some("26.40", "2026-04-18", "Harbor Invite")
    );
    assert_eq!(best(&e.personal_record).unwrap().0, "26.40");
    Ok(())
}

#[tokio::test]
async fn season_overrides_the_as_of_year() -> anyhow::Result<()> {
    let e = query(ProfileQuery {
        event: Some("400 Meters"),
        as_of: Some(date("2026-04-01")),
        season: Some(2025),
        today: date(TODAY),
    })
    .await?
    .event
    .unwrap();
    assert_eq!(e.season, 2025);
    assert_eq!(
        best(&e.season_best),
        some("56.90", "2025-05-10", "League Finals")
    );
    assert_eq!(best(&e.personal_record).unwrap().0, "56.90");
    Ok(())
}

/// The JSON contract (ADR 0007), pinned: `schema` first, the Profile URL, and
/// glossary terms (`season_best`, `personal_record`) as field names.
#[tokio::test]
async fn json_shape_is_pinned() -> anyhow::Result<()> {
    let p = dana(Some("400m"), "2026-04-01").await?;
    assert_eq!(Profile::SCHEMA, 1);
    let want = r#"{
  "schema": 1,
  "athlete_id": 2001,
  "name": "Dana Diaz",
  "gender": "F",
  "url": "https://www.athletic.net/athlete/2001/track-and-field",
  "school": {
    "id": 601,
    "name": "Harbor High",
    "kind": "school",
    "level": 4,
    "year": 2026,
    "url": "https://www.athletic.net/team/601/track-and-field-outdoor"
  },
  "teams": [
    {
      "id": 601,
      "name": "Harbor High",
      "kind": "school",
      "level": 4,
      "year": 2026,
      "url": "https://www.athletic.net/team/601/track-and-field-outdoor"
    },
    {
      "id": 602,
      "name": "Bayview Middle",
      "kind": "school",
      "level": 2,
      "year": 2024,
      "url": "https://www.athletic.net/team/602/track-and-field-outdoor"
    },
    {
      "id": 9101,
      "name": "Coastal Striders TC",
      "kind": "club",
      "level": 16,
      "year": 2026,
      "url": "https://www.athletic.net/team/9101/track-and-field-outdoor"
    }
  ],
  "area": null,
  "area_lookup": {
    "status": "state_not_configured",
    "season": 2026,
    "school_id": 601,
    "state": "OR",
    "note": "OR has no Area level configured in ath"
  },
  "event": {
    "name": "400 Meters",
    "on_profile": true,
    "status": "ok",
    "as_of": "2026-04-01",
    "season": 2026,
    "season_best": {
      "mark": "58.20",
      "sort_int": 58195,
      "date": "2026-03-14",
      "season": 2026,
      "meet": "Spring Opener",
      "meet_id": 5004
    },
    "personal_record": {
      "mark": "56.90",
      "sort_int": 56904,
      "date": "2025-05-10",
      "season": 2025,
      "meet": "League Finals",
      "meet_id": 5003
    }
  }
}"#;
    assert_eq!(to_json(&p)?, want);
    Ok(())
}

#[tokio::test]
async fn json_without_event_has_null_event() -> anyhow::Result<()> {
    let p = query(ProfileQuery::today(date(TODAY))).await?;
    let v: serde_json::Value = serde_json::from_str(&to_json(&p)?)?;
    assert_eq!(v["schema"], 1);
    assert!(v["event"].is_null());
    Ok(())
}

#[tokio::test]
async fn table_is_readable() -> anyhow::Result<()> {
    let p = dana(Some("400 Meters"), "2026-04-01").await?;
    let mut out = Vec::new();
    emit(&p, Format::Table, &mut out)?;
    let want = "\
Name     Dana Diaz
Gender   F
School   Harbor High
Area     none for 2026 - OR has no Area level configured in ath
Profile  https://www.athletic.net/athlete/2001/track-and-field

Team                 Kind    Year  URL
Harbor High          school  2026  https://www.athletic.net/team/601/track-and-field-outdoor
Bayview Middle       school  2024  https://www.athletic.net/team/602/track-and-field-outdoor
Coastal Striders TC  club    2026  https://www.athletic.net/team/9101/track-and-field-outdoor

400 Meters as of 2026-04-01 (season 2026)
                 Mark   SortInt  Date        Meet
Season best      58.20  58195    2026-03-14  Spring Opener
Personal record  56.90  56904    2025-05-10  League Finals
";
    assert_eq!(String::from_utf8(out)?, want);
    Ok(())
}

#[tokio::test]
async fn table_explains_a_missing_personal_record() -> anyhow::Result<()> {
    let p = dana_today("Long Jump").await?;
    let mut out = Vec::new();
    emit(&p, Format::Table, &mut out)?;
    let out = String::from_utf8(out)?;
    assert!(
        out.contains(
            "Long Jump as of 2026-10-26 (season 2026)\n\
             This is a field event; ath v1 supports running events only.\n"
        ),
        "{out}"
    );
    assert!(out.contains("Personal record  none"), "{out}");
    Ok(())
}
