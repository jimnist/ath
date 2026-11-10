//! Mode A against the made-up meet 900700 in `tests/fixtures/meets/`
//! (recorded the way `FixtureSource` documents): its events with entries, an
//! individual event's Entrants and the entries left out, a relay's teams and
//! legs, and a seed sheet built from the entries.

use ath::area::Gender;
use ath::contract::{Format, emit};
use ath::meet::{
    EventSelect, LeftOutReason, Located, RelayShape, SelectedEvent, meet_report, meet_resolve,
    read_event, seedable,
};
use ath::roster::RosterRow;
use ath::seed::{FetchOpts, SeedDate, seed};
use ath::seed_relay::seed_relay;
use ath::source::{DataSource, FixtureSource};
use chrono::NaiveDate;

const MEET: i64 = 900_700;

/// An Entrant as the tests compare it: athlete id, name, grade, school and
/// entered `SortInt`.
type EntrantRow<'a> = (i64, &'a str, Option<i64>, &'a str, Option<i64>);

/// A relay team as the tests compare it: team id, school id, school, letter
/// and entered `SortInt`.
type TeamRow<'a> = (Option<i64>, Option<i64>, &'a str, &'a str, Option<i64>);

/// A seeded relay team as the tests compare it: rank, school, letter,
/// aggregate, its `SortInt` and how many runners it is short.
type SeededRow<'a> = (usize, &'a str, &'a str, &'a str, Option<i64>, usize);

fn source() -> FixtureSource {
    FixtureSource::from_dir("tests/fixtures").unwrap()
}

/// The production path to an event's entries (`read_event`), checked with
/// `check`: `seedable` as `ath seed --meet` does, or nothing as `ath meet`.
async fn select(
    src: &FixtureSource,
    select: &EventSelect<'_>,
    check: fn(&Located) -> anyhow::Result<()>,
) -> anyhow::Result<SelectedEvent> {
    let meet = src.meet(select.meet_id).await?;
    let list = src.meet_events(select.meet_id).await?;
    read_event(src, meet, &list, select, check).await
}

fn any(_: &Located) -> anyhow::Result<()> {
    Ok(())
}

fn boys_varsity(event: &str) -> EventSelect<'_> {
    EventSelect {
        meet_id: MEET,
        gender: Gender::M,
        division: "V",
        event,
    }
}

#[tokio::test]
async fn the_meet_lists_its_events_with_entries() -> anyhow::Result<()> {
    let r = meet_report(&source(), MEET, None).await?;
    assert_eq!(r.meet.name, "Riverton Winter Classic");
    assert_eq!(r.meet.date, "2099-01-09".parse().ok());
    assert_eq!(r.meet.season_id, Some(12099));
    assert_eq!(r.season.as_deref(), Some("2099 Indoor"));
    assert!(r.meet.indoor);
    assert_eq!(
        (r.city.as_deref(), r.state.as_deref()),
        (Some("Riverton"), Some("NY"))
    );
    assert_eq!(r.relay_athletes, Some(4));
    assert_eq!(
        r.meet.url,
        "https://www.athletic.net/TrackAndField/meet/900700"
    );
    let divisions: Vec<(i64, &str)> = r
        .divisions
        .iter()
        .map(|d| (d.id, d.name.as_str()))
        .collect();
    assert_eq!(divisions, [(13, "Varsity"), (14, "Junior Varsity")]);

    // Only the event and division pairs with entries: not the 1 mile, not
    // the girls' 55m.
    let events: Vec<(&str, i64, &str, bool)> = r
        .events
        .iter()
        .map(|e| {
            (
                e.gender.as_str(),
                e.division_id,
                e.event_short.as_str(),
                e.relay,
            )
        })
        .collect();
    assert_eq!(
        events,
        [
            ("f", 13, "800m", false),
            ("m", 13, "800m", false),
            ("m", 13, "4x800m", true),
            ("m", 13, "Shot", false),
            // Relays by name, not by code (as seen live on a meet).
            ("m", 13, "sprintmed2248", true),
            ("m", 13, "55shuttleh", true),
            ("m", 14, "55m", false),
        ]
    );
    assert_eq!(
        r.events[1].url,
        "https://www.athletic.net/TrackAndField/meet/900700/entries/m/13/800m"
    );
    assert!(r.entries.is_none());
    Ok(())
}

#[tokio::test]
async fn only_accepted_unscratched_entries_are_entrants() -> anyhow::Result<()> {
    let sel = select(&source(), &boys_varsity("800 Meters"), seedable).await?;
    assert_eq!(sel.event.event, "800 Meters");
    assert_eq!(sel.event.event_short, "800m");
    assert!(!sel.event.relay && sel.running);
    let entrants: Vec<EntrantRow> = sel
        .entries
        .entrants
        .iter()
        .map(|e| {
            (
                e.athlete_id,
                e.name.as_str(),
                e.grade,
                e.school.as_str(),
                e.entered_sort_int,
            )
        })
        .collect();
    assert_eq!(
        entrants,
        [
            (
                900100001,
                "Milo Marsh",
                Some(11),
                "Pinecrest Academy",
                Some(118900)
            ),
            (
                900100002,
                "Theo Quill",
                Some(12),
                "Harbor View High",
                Some(121500)
            ),
            (900100003, "Ravi Stone", Some(10), "Pinecrest Academy", None),
            (
                900100006,
                "Luca Brandt",
                Some(9),
                "Lakemont Prep",
                Some(117500)
            ),
        ]
    );
    let left_out: Vec<(&str, LeftOutReason, Option<i64>)> = sel
        .entries
        .left_out
        .iter()
        .map(|l| (l.name.as_str(), l.reason, l.acceptance_status))
        .collect();
    assert_eq!(
        left_out,
        [
            ("Owen Pike", LeftOutReason::Scratched, Some(10)),
            ("Jace Fenwick", LeftOutReason::NotAccepted, Some(5)),
        ]
    );
    assert!(sel.entries.teams.is_empty());
    Ok(())
}

#[tokio::test]
async fn relay_entries_are_teams_with_their_legs_in_order() -> anyhow::Result<()> {
    let sel = select(&source(), &boys_varsity("4x800m"), any).await?;
    assert!(sel.event.relay);
    assert!(sel.entries.entrants.is_empty());
    let teams: Vec<TeamRow> = sel
        .entries
        .teams
        .iter()
        .map(|t| {
            (
                t.team_id,
                t.school_id,
                t.school.as_str(),
                t.letter.as_str(),
                t.entered_sort_int,
            )
        })
        .collect();
    // Pinecrest's A and B teams, then Harbor View's and Lakemont's A teams.
    assert_eq!(
        teams,
        [
            (
                Some(900501),
                Some(900401),
                "Pinecrest Academy",
                "A",
                Some(482000)
            ),
            (
                Some(900502),
                Some(900401),
                "Pinecrest Academy",
                "B",
                Some(501500)
            ),
            (
                Some(900503),
                Some(900402),
                "Harbor View High",
                "A",
                Some(490300)
            ),
            (
                Some(900504),
                Some(900403),
                "Lakemont Prep",
                "A",
                Some(505000)
            ),
        ]
    );
    // Harbor View's legs were entered out of order.
    let legs: Vec<(Option<u32>, i64, &str, Option<i64>)> = sel.entries.teams[2]
        .legs
        .iter()
        .map(|l| (l.leg, l.athlete_id, l.name.as_str(), l.grade))
        .collect();
    assert_eq!(
        legs,
        [
            (Some(1), 900100021, "Kai Lowell", Some(12)),
            (Some(2), 900100022, "Bram Ostrow", Some(12)),
            (Some(3), 900100023, "Otis Wren", Some(11)),
            (Some(4), 900100024, "Cyrus Hale", Some(11)),
        ]
    );
    // Lakemont entered only three runners.
    let legs: Vec<usize> = sel.entries.teams.iter().map(|t| t.legs.len()).collect();
    assert_eq!(legs, [4, 4, 4, 3]);
    Ok(())
}

/// A relay of four equal legs is seeded on its Seeding event (the 800m for
/// the 4x800m), each runner exactly as an individual Entrant: Ezra Holt's
/// faster relay split and indoor mark never count, Finch Dorsey has no 800m
/// at all, and Lakemont is a runner short, so those two teams have no
/// aggregate and go last, in entry order.
#[tokio::test]
async fn a_relay_is_seeded_on_the_sum_of_its_runners_personal_records() -> anyhow::Result<()> {
    let src = source();
    let sel = select(&src, &boys_varsity("4x800m"), seedable).await?;
    let RelayShape::Equal(legs) = sel.relay_shape.clone() else {
        panic!("a relay of equal legs")
    };
    assert_eq!((legs.legs, legs.seeding_event.as_str()), (4, "800m"));
    let today: NaiveDate = "2026-11-06".parse()?;
    let date = SeedDate::for_meet(
        MEET,
        sel.meet.meet.date().unwrap(),
        sel.meet.meet.end().unwrap(),
        today,
    )?;
    let outcome = seed_relay(
        &src,
        &sel.entries.teams,
        &legs.seeding_event,
        legs.mile,
        date,
        &FetchOpts::default(),
    )
    .await?;
    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    assert_eq!(outcome.runners, 15);
    assert_eq!(outcome.seeding_event_name.as_deref(), Some("800 Meters"));
    let teams: Vec<SeededRow> = outcome
        .teams
        .iter()
        .map(|t| {
            (
                t.rank,
                t.team.as_str(),
                t.letter.as_str(),
                t.aggregate.as_str(),
                t.sort_int,
                t.missing_legs.len(),
            )
        })
        .collect();
    assert_eq!(
        teams,
        [
            (1, "Harbor View High", "A", "7:59.05", Some(479_050), 0),
            (2, "Pinecrest Academy", "A", "8:02.55", Some(482_550), 0),
            (3, "Pinecrest Academy", "B", "", None, 0),
            (4, "Lakemont Prep", "A", "", None, 1),
        ]
    );
    // The aggregate is the sum of the legs' SortInts, never of display
    // strings.
    let pinecrest = &outcome.teams[1];
    let sum: i64 = pinecrest.legs.iter().map(|l| l.sort_int.unwrap()).sum();
    assert_eq!(Some(sum), pinecrest.sort_int);
    let marks: Vec<(&str, &str, Option<i64>)> = pinecrest
        .legs
        .iter()
        .map(|l| (l.athlete.as_str(), l.pr.as_str(), l.pr_season))
        .collect();
    assert_eq!(
        marks,
        [
            ("Ezra Holt", "1:58.20", Some(2026)),
            ("Nico Vance", "2:00.40", Some(2025)),
            ("Silas Reed", "2:01.15", Some(2026)),
            ("Arlo Penn", "2:02.80", Some(2026)),
        ]
    );
    let finch = &outcome.teams[2].legs[3];
    assert_eq!(
        (finch.athlete.as_str(), finch.sort_int),
        ("Finch Dorsey", None)
    );
    assert!(
        outcome.teams.iter().all(|t| !t.estimated),
        "no mile on a 4x800m"
    );
    Ok(())
}

/// A 4xMile seeds each runner on the faster of their 1600m and their mile
/// converted to the 1600m, the mile flagged as an Estimated mark: Wren
/// Talbot has only a mile, Iris Kettering's mile beats her 1600m (her faster
/// mile relay split and indoor mile never count), Juno Halvorsen's 1600m
/// beats her mile, and Tess Amberly has only a 1600m.
#[tokio::test]
async fn a_4xmile_seeds_on_the_better_of_the_1600m_and_the_mile() -> anyhow::Result<()> {
    let src = source();
    let select = EventSelect {
        meet_id: 900_702,
        gender: Gender::F,
        division: "V",
        event: "4xMile",
    };
    let sel = select_event(&src, &select).await?;
    let RelayShape::Equal(legs) = sel.relay_shape.clone() else {
        panic!("a relay of equal legs")
    };
    assert_eq!((legs.seeding_event.as_str(), legs.mile), ("1600m", true));
    let today: NaiveDate = "2026-11-06".parse()?;
    let date = SeedDate::for_meet(
        900_702,
        sel.meet.meet.date().unwrap(),
        sel.meet.meet.end().unwrap(),
        today,
    )?;
    let opts = FetchOpts::default();
    let seed_on = |mile| seed_relay(&src, &sel.entries.teams, "1600m", mile, date, &opts);
    let outcome = seed_on(true).await?;
    assert_eq!(outcome.estimate_from.as_deref(), Some("1 Mile"));
    let cedar = &outcome.teams[0];
    assert_eq!(
        (cedar.team.as_str(), cedar.estimated),
        ("Cedar Hollow High", true)
    );
    type Leg<'a> = (&'a str, &'a str, bool, Option<&'a str>, Option<i64>);
    let marks: Vec<Leg> = cedar
        .legs
        .iter()
        .map(|l| {
            (
                l.athlete.as_str(),
                l.pr.as_str(),
                l.estimated,
                l.estimated_from_pr.as_deref(),
                l.sort_int,
            )
        })
        .collect();
    assert_eq!(
        marks,
        [
            (
                "Wren Talbot",
                "5:08.21",
                true,
                Some("5:10.00"),
                Some(308_210)
            ),
            (
                "Iris Kettering",
                "5:00.35",
                true,
                Some("5:02.10"),
                Some(300_350)
            ),
            ("Juno Halvorsen", "4:58.00", false, None, Some(298_000)),
            ("Tess Amberly", "5:12.00", false, None, Some(312_000)),
        ]
    );
    // The aggregate is the sum in 1600m terms.
    assert_eq!(cedar.sort_int, Some(308_210 + 300_350 + 298_000 + 312_000));
    assert_eq!(cedar.legs[0].estimated_from.as_deref(), Some("1 Mile"));
    assert!(!outcome.teams[1].estimated);

    // Without the mile (as with --seed-on), nothing is converted: Wren has no
    // 1600m, so her team has no aggregate.
    let plain = seed_on(false).await?;
    assert_eq!(plain.estimate_from, None);
    assert!(
        plain
            .teams
            .iter()
            .flat_map(|t| &t.legs)
            .all(|l| !l.estimated)
    );
    assert_eq!(plain.teams[1].team, "Cedar Hollow High");
    assert_eq!(plain.teams[1].sort_int, None);
    Ok(())
}

async fn select_event(
    src: &FixtureSource,
    select: &EventSelect<'_>,
) -> anyhow::Result<SelectedEvent> {
    let meet = src.meet(select.meet_id).await?;
    let list = src.meet_events(select.meet_id).await?;
    read_event(src, meet, &list, select, seedable).await
}

#[tokio::test]
async fn an_event_is_found_by_code_or_name_and_a_missing_one_says_what_there_is() {
    let src = source();
    let err = select(&src, &boys_varsity("1 Mile"), any)
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        err,
        "meet 900700 has no 1mile entries in the Varsity division (yet); its boys' events \
         with entries there are 800m, 4x800m, Shot, sprintmed2248, 55shuttleh"
    );
    let err = select(&src, &boys_varsity("400m"), any)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.starts_with("meet 900700 has no boys' event \"400m\""),
        "{err}"
    );
    let mut jv = boys_varsity("55m");
    jv.division = "Junior Varsity";
    let err = select(&src, &jv, any).await.unwrap_err().to_string();
    // Listed as having entries, but not recorded: the fixture error names
    // the file the layout expects.
    assert_eq!(err, "no fixture meets/900700/entries/m-14-55m.json");
}

/// Mode A seeds the entrants exactly as Mode B seeds pinned roster rows: on
/// their Personal record in the event as the meet names it, as of today
/// (the meet is later). Luca's faster 1:55.00 is a relay split and Milo's
/// 1:57.80 is indoors, so neither counts; Ravi has no 800m at all.
#[tokio::test]
async fn the_entrants_are_seeded_like_a_pinned_roster() -> anyhow::Result<()> {
    let src = source();
    let sel = select(&src, &boys_varsity("800m"), seedable).await?;
    let roster: Vec<RosterRow> = sel
        .entries
        .entrants
        .iter()
        .map(|e| e.roster_row())
        .collect();
    assert!(roster.iter().all(|r| r.athlete_id.is_some()));
    let today: NaiveDate = "2026-11-06".parse()?;
    let meet_date = sel.meet.meet.date().unwrap();
    let meet_end = sel.meet.meet.end().unwrap();
    let date = SeedDate::for_meet(MEET, meet_date, meet_end, today)?;
    assert_eq!((date.as_of(), date.meet_date()), (today, Some(meet_date)));
    let outcome = seed(&src, &roster, &sel.event.event, date, &FetchOpts::default()).await?;
    assert!(outcome.failures.is_empty(), "{:?}", outcome.failures);
    let rows: Vec<(usize, &str, &str, Option<i64>)> = outcome
        .rows
        .iter()
        .map(|r| (r.rank, r.athlete.as_str(), r.pr.as_str(), r.pr_season))
        .collect();
    assert_eq!(
        rows,
        [
            (1, "Luca Brandt", "1:57.95", Some(2026)),
            (2, "Milo Marsh", "1:59.10", Some(2026)),
            (3, "Theo Quill", "2:00.45", Some(2025)),
            (4, "Ravi Stone", "", None),
        ]
    );
    Ok(())
}

#[tokio::test]
async fn the_meet_report_prints_one_events_entries() -> anyhow::Result<()> {
    let r = meet_report(&source(), MEET, Some(&boys_varsity("4x800m"))).await?;
    let mut out = Vec::new();
    emit(&r, Format::Json, &mut out)?;
    let v: serde_json::Value = serde_json::from_slice(&out)?;
    assert_eq!(v["schema"], 1);
    assert_eq!(v["id"], MEET);
    assert_eq!(v["entries"]["event_short"], "4x800m");
    assert_eq!(v["entries"]["relay"], true);
    assert_eq!(v["entries"]["entrants"], serde_json::json!([]));
    assert_eq!(v["entries"]["teams"][1]["letter"], "B");
    assert_eq!(v["entries"]["teams"][1]["legs"][3]["leg"], 4);
    assert_eq!(v["entries"]["teams"][1]["legs"][3]["athlete_id"], 900100018);
    assert_eq!(v["entries"]["left_out"], serde_json::json!([]));
    Ok(())
}

/// `ath meet --resolve` (`meet_resolve`): the event and division a code,
/// flat race or name and a division id, name or short name pick out, as
/// `ath seed --meet` finds them, with no entries read; an event without
/// entries in the division says what there is.
#[tokio::test]
async fn an_event_resolves_without_reading_its_entries() -> anyhow::Result<()> {
    let src = source();
    for (division, event) in [("V", "800m"), ("13", "800 Meters"), ("varsity", " 800 m ")] {
        let select = EventSelect {
            meet_id: MEET,
            gender: Gender::M,
            division,
            event,
        };
        let report = meet_resolve(&src, MEET, &select).await?;
        assert!(report.entries.is_none());
        let mut json = Vec::new();
        emit(&report, Format::Json, &mut json)?;
        let v: serde_json::Value = serde_json::from_slice(&json)?;
        let r = report.resolved.expect("resolved");
        assert_eq!(
            (
                r.division_id,
                r.division.as_str(),
                r.event_short.as_str(),
                r.relay
            ),
            (13, "Varsity", "800m", false),
            "{division} {event}"
        );
        assert_eq!(
            v["resolved"]["url"],
            "https://www.athletic.net/TrackAndField/meet/900700/entries/m/13/800m"
        );
        assert!(v.get("entries").is_none());
    }
    let relay = meet_resolve(&src, MEET, &boys_varsity("4x800m")).await?;
    assert!(relay.resolved.expect("resolved").relay);
    let err = meet_resolve(&src, MEET, &boys_varsity("1mile"))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.starts_with("meet 900700 has no 1mile entries in the Varsity division (yet)"),
        "{err}"
    );
    Ok(())
}
