//! `ath rank`, an Athlete's places by Grade group, end to end through
//! the fixture-backed data source and the `ath` binary.
//!
//! Fixtures (`tests/fixtures/`), anonymized from real recordings (ADR 0010,
//! `docs/anonymize.md`), in the fictional season 2099 (so the clock is pinned
//! to 2099-10-30):
//!
//! - `982389051.json`: the subject, Jamie Uddumcombe, a 9th grader at
//!   Cranevale Heights (school 920353302, in the Central Coast Section), 400m
//!   58.98.
//! - `rankings/985316289-f-400m-*.json` and `rankings/921344494-f-400m-*.json`:
//!   the girls' 400m California and Texas lists, every page `ath rank` reads
//!   for her (one to four per Grade group); `rankings/996953819-f-400m-*.json`:
//!   the Central Coast's.
//! - `navinfo/2099.json` and `teamnav/920353302-2099.json`: the season's tree
//!   and the school's chain.
//!
//! Her places, counted from those pages alone: California 42 / 109 / T195 /
//! T303, Texas T56 / T132 / T262 / T379, Central Coast 6 / 12 / 20 / 36.
//!
//! The made-up people (3001, 3002) are in the made-up 2026 tree
//! (`navinfo/2026.json`, school 5103).

mod common;

use std::collections::BTreeSet;
use std::process::Output;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use ath::area::{AreaTree, Gender, Region, TeamNav};
use ath::list::{Grades, ListKind, PageQuery, RankingsPage};
use ath::meet::{EntriesData, EntriesQuery, MeetData, MeetEventList};
use ath::model::{Bio, Candidate};
use ath::rank::{Ahead, RankQuery, RankStatus, Ranks, Targets, rank};
use ath::source::{DataSource, FixtureSource};
use chrono::NaiveDate;

const FIXTURES: &str = "tests/fixtures";
/// The subject of the anonymized lists.
const SUBJECT: i64 = 982_389_051;
const SEASON: i64 = 2099;
/// The anonymized season's clock (`ATH_TODAY` for the binary).
const TODAY: &str = "2099-10-30";
const CALIFORNIA: i64 = 985_316_289;
const TEXAS: i64 = 921_344_494;
const CENTRAL_COAST: i64 = 996_953_819;
/// Her season's best.
const MARK: i64 = 58_980;

fn today() -> NaiveDate {
    NaiveDate::parse_from_str(TODAY, "%Y-%m-%d").unwrap()
}

fn query(athlete_id: i64, targets: Targets) -> RankQuery<'static> {
    RankQuery {
        athlete_id,
        event: "400m".to_string(),
        season: SEASON,
        targets,
        grade: None,
        home: None,
        ahead: Ahead::Neighborhood,
        from_list: 0,
        today: today(),
    }
}

/// A made-up athlete's query, in the made-up 2026 tree.
fn made_up_query(athlete_id: i64, targets: Targets) -> RankQuery<'static> {
    RankQuery {
        season: 2026,
        today: NaiveDate::from_ymd_opt(2026, 10, 27).unwrap(),
        ..query(athlete_id, targets)
    }
}

fn ca_tx() -> Targets {
    Targets::parse("CA,TX").unwrap()
}

/// Wraps a source, counting the pages asked for.
struct Counting<S> {
    inner: S,
    pages: AtomicU64,
}

impl<S> Counting<S> {
    fn new(inner: S) -> Self {
        Self {
            inner,
            pages: AtomicU64::new(0),
        }
    }
}

#[async_trait]
impl<S: DataSource> DataSource for Counting<S> {
    async fn get_bio(&self, id: i64) -> anyhow::Result<Bio> {
        self.inner.get_bio(id).await
    }
    async fn search(&self, name: &str) -> anyhow::Result<Vec<Candidate>> {
        self.inner.search(name).await
    }
    async fn area_tree(&self, season: i64, from: i64) -> anyhow::Result<Option<AreaTree>> {
        self.inner.area_tree(season, from).await
    }
    async fn season_area_tree(&self, season: i64, list: i64) -> anyhow::Result<AreaTree> {
        self.inner.season_area_tree(season, list).await
    }
    async fn state_tree(&self, season: i64, list: i64) -> anyhow::Result<Vec<Region>> {
        self.inner.state_tree(season, list).await
    }
    async fn team_divisions(&self, school: i64, season: i64) -> anyhow::Result<TeamNav> {
        self.inner.team_divisions(school, season).await
    }
    async fn rankings_page(&self, q: &PageQuery<'_>) -> anyhow::Result<RankingsPage> {
        self.pages.fetch_add(1, Ordering::Relaxed);
        self.inner.rankings_page(q).await
    }
    async fn meet(&self, id: i64) -> anyhow::Result<MeetData> {
        self.inner.meet(id).await
    }
    async fn meet_events(&self, id: i64) -> anyhow::Result<MeetEventList> {
        self.inner.meet_events(id).await
    }
    async fn meet_entries(&self, q: &EntriesQuery<'_>) -> anyhow::Result<EntriesData> {
        self.inner.meet_entries(q).await
    }
}

fn source() -> FixtureSource {
    FixtureSource::from_dir(FIXTURES).unwrap()
}

async fn subject() -> anyhow::Result<Ranks> {
    rank(&source(), &query(SUBJECT, ca_tx())).await
}

/// `home_area` is reported only when `--in` asks for the home Area (`home`
/// or `areas`), though the school is looked up to find the State either way.
#[tokio::test]
async fn the_home_area_is_reported_only_when_asked_for() -> anyhow::Result<()> {
    assert!(subject().await?.home_area.is_none());
    let asked = rank(&source(), &query(SUBJECT, Targets::parse("home").unwrap())).await?;
    assert_eq!(
        asked.home_area.map(|a| a.name).as_deref(),
        Some("Central Coast")
    );
    Ok(())
}

/// `--in areas`: every Area of her home State, California's ten Sections
/// (`areas/CA.json`), in name order. The fixtures record only Central
/// Coast's lists, so the other nine are `lookup_failed` with their error, and
/// Central Coast is placed as usual.
#[tokio::test]
async fn areas_places_her_on_every_area_of_her_state() -> anyhow::Result<()> {
    let r = rank(&source(), &query(SUBJECT, Targets::parse("areas").unwrap())).await?;
    let mut lists: Vec<&str> = r.results.iter().map(|x| x.list.name.as_str()).collect();
    lists.dedup();
    assert_eq!(
        lists,
        [
            "Central",
            "Central Coast",
            "Los Angeles",
            "North Coast",
            "Northern",
            "Oakland",
            "Sac-Joaquin",
            "San Diego",
            "San Francisco",
            "Southern"
        ]
    );
    assert!(r.results.iter().all(|x| x.list.kind == ListKind::Area));
    assert_eq!(labels(&r, CENTRAL_COAST), ["6", "12", "20", "36"]);
    let failed = r
        .results
        .iter()
        .filter(|x| x.status == RankStatus::LookupFailed)
        .count();
    assert_eq!(failed, 9 * 4);
    Ok(())
}

/// The Areas of a State with none configured, asked for by name, are an
/// error. (`areas` for such a home State adds nothing, with a note: the
/// `areas_follows_the_home_state` unit test.)
#[tokio::test]
async fn areas_of_a_state_without_areas_is_an_error() -> anyhow::Result<()> {
    let err = rank(
        &source(),
        &query(SUBJECT, Targets::parse("areas:TX").unwrap()),
    )
    .await
    .unwrap_err();
    assert!(
        format!("{err:#}").contains("TX has no Areas configured (add an areas/TX.json)"),
        "{err:#}"
    );
    Ok(())
}

/// The labels for one list, Grade group by Grade group.
fn labels(r: &Ranks, list_id: i64) -> Vec<String> {
    r.results
        .iter()
        .filter(|x| x.list.id == list_id)
        .map(|x| {
            x.place
                .as_ref()
                .map_or("-".to_string(), |p| p.label.clone())
        })
        .collect()
}

/// Her places agree with the counts worked out from the recorded pages alone
/// (the module comment).
#[tokio::test]
async fn places_her_as_the_pages_count() -> anyhow::Result<()> {
    let r = subject().await?;
    assert_eq!(r.status, RankStatus::Ok);
    assert_eq!((r.grade, r.gender), (Some(9), Gender::F));
    let mark = r.mark.as_ref().unwrap();
    assert_eq!((mark.mark.as_str(), mark.sort_int), ("58.98", MARK));
    // Found on her own grade's California list.
    assert_eq!(
        mark.list.url,
        "https://www.athletic.net/TrackAndField/rankings/list/985316289/f/400m?grades=9"
    );
    assert_eq!(
        r.groups,
        [vec![9], vec![9, 10], vec![9, 10, 11], vec![9, 10, 11, 12]]
    );
    assert_eq!(labels(&r, CALIFORNIA), ["42", "109", "T195", "T303"]);
    assert_eq!(labels(&r, TEXAS), ["T56", "T132", "T262", "T379"], "Texas");
    Ok(())
}

/// A list capped with Masked rows (an unpaid session) stops the whole
/// run with the paid-subscription error, instead of one list's lookup
/// failing or a place being counted against the rows shown.
#[tokio::test]
async fn a_capped_list_stops_the_run() -> anyhow::Result<()> {
    let grade_9 = Grades::only(9);
    let q = PageQuery {
        list_id: TEXAS,
        season: SEASON,
        gender: Gender::F,
        event: "400m",
        grades: &grade_9,
        page: 1,
    };
    // Texas' 9th grade list as an unpaid session would see it: five made-up
    // people, then a Masked row.
    let capped: RankingsPage = serde_json::from_value(serde_json::json!({
        "eventShort": "400m",
        "division": {"ID": TEXAS, "SeasonID": SEASON, "Name": "Texas"},
        "minCount": 1032,
        "settings": {"fat": true, "official": true, "grades": [9]},
        "groupedRankings": [(1..=6).map(|n| if n < 6 {
            serde_json::json!({"rank": n, "rowNum": n, "AthleteID": 940_000_100 + n,
                "AthleteName": format!("Made Up {n}"), "display": "55.00",
                "SortIntCalc": 55_000 + n})
        } else {
            serde_json::json!({"rank": n, "rowNum": n, "AthleteID": 0,
                "AthleteName": "Xxxxx Xxxxxxx", "display": "", "SortIntCalc": 55_500})
        }).collect::<Vec<_>>()],
    }))?;
    let src = source().with_rankings_page(&q, capped);
    let err = rank(&src, &query(SUBJECT, ca_tx())).await.unwrap_err();
    assert!(
        format!("{err:#}").starts_with(
            "Athletic.net returned only 5 of at least 1032 rows for Texas 2099 girls 400m \
             (grade 9); full lists need a paid Athletic.net subscription"
        ),
        "{err:#}"
    );
    Ok(())
}

/// A capped list whose rows shown already place the Athlete and fill
/// her Neighborhood is still answered (her T56 on Texas 9th).
#[tokio::test]
async fn a_capped_list_that_places_her_within_its_rows_is_answered() -> anyhow::Result<()> {
    let grade_9 = Grades::only(9);
    let q = PageQuery {
        list_id: TEXAS,
        season: SEASON,
        gender: Gender::F,
        event: "400m",
        grades: &grade_9,
        page: 1,
    };
    // The recorded page cut to its first 60 rows (her 55 ahead, the one tied
    // and 3 behind her mark), then a Masked row.
    let mut page: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(
        "tests/fixtures/rankings/921344494-f-400m-9-p1.json",
    )?)?;
    let rows = page["groupedRankings"][0].as_array_mut().unwrap();
    rows.truncate(60);
    rows.push(serde_json::json!({"rank": 61, "rowNum": 61, "AthleteID": 0,
        "AthleteName": "Xxxxx Xxxxxxx", "display": "", "SortIntCalc": 1}));
    let capped: RankingsPage = serde_json::from_value(page)?;
    assert!(capped.is_capped_after(0));
    let src = source().with_rankings_page(&q, capped);
    let r = rank(&src, &query(SUBJECT, ca_tx())).await?;
    assert_eq!(labels(&r, TEXAS)[0], "T56", "Texas 9th");
    Ok(())
}

#[tokio::test]
async fn only_the_pages_needed_are_read() -> anyhow::Result<()> {
    let src = Counting::new(source());
    let r = rank(&src, &query(SUBJECT, ca_tx())).await?;
    // Her row (1 page), then each list and group down past her mark: one to
    // four pages of 100.
    let pages: Vec<u32> = r.results.iter().map(|x| x.pages).collect();
    assert_eq!(pages, [1, 2, 2, 4, 1, 2, 3, 4]);
    assert_eq!(src.pages.load(Ordering::Relaxed), 1 + 19);
    Ok(())
}

#[tokio::test]
async fn each_result_carries_its_neighborhood() -> anyhow::Result<()> {
    let r = subject().await?;
    let at = |list, top: usize| {
        r.results
            .iter()
            .find(|x| x.list.id == list && x.grades.len() == top - 8)
            .unwrap()
    };

    // California 9th, #42 (50 or better): everyone ahead, the 3 just behind.
    let ca9 = at(CALIFORNIA, 9);
    assert_eq!(ca9.group, "9th");
    assert_eq!(
        ca9.url,
        "https://www.athletic.net/TrackAndField/rankings/list/985316289/f/400m?grades=9"
    );
    assert_eq!(ca9.list.kind, ListKind::State);
    let p = ca9.place.as_ref().unwrap();
    let n = &p.neighborhood;
    assert_eq!(
        (n.ahead.len(), n.skipped, n.tied.len(), n.behind.len()),
        (41, 0, 0, 3)
    );
    assert!(n.ahead.windows(2).all(|w| w[0].sort_int <= w[1].sort_int));
    assert!(n.ahead.iter().all(|x| x.sort_int < MARK));
    assert!(n.behind.iter().all(|x| x.sort_int > MARK));
    // She is never her own neighbor.
    let everyone = n.ahead.iter().chain(&n.tied).chain(&n.behind);
    assert!(everyone.clone().all(|x| x.athlete_id != SUBJECT));

    // Texas All HS, T379: the leader, the 3 just ahead, the tie, 3 behind.
    let tx = at(TEXAS, 12);
    assert_eq!(tx.group, "All HS");
    let p = tx.place.as_ref().unwrap();
    assert_eq!((p.rank, p.tied, p.ahead, p.ties), (379, true, 378, 1));
    let n = &p.neighborhood;
    assert_eq!(
        (n.ahead.len(), n.skipped, n.tied.len(), n.behind.len()),
        (4, 374, 1, 3)
    );
    assert_eq!(n.ahead[0].rank, Some(1), "the leader");
    assert_eq!(n.tied[0].sort_int, MARK);
    assert_eq!(n.ahead[3].sort_int, 58_960, "the one just ahead");
    Ok(())
}

#[tokio::test]
async fn ahead_all_lists_everyone_ahead_from_the_same_pages() -> anyhow::Result<()> {
    let src = Counting::new(source());
    let mut q = query(SUBJECT, ca_tx());
    q.ahead = Ahead::All;
    let all = rank(&src, &q).await?;
    assert_eq!(src.pages.load(Ordering::Relaxed), 1 + 19, "no extra pages");
    let usual = subject().await?;
    for (a, u) in all.results.iter().zip(&usual.results) {
        let (pa, pu) = (a.place.as_ref().unwrap(), u.place.as_ref().unwrap());
        assert_eq!(
            (&pa.label, pa.ahead, pa.ties),
            (&pu.label, pu.ahead, pu.ties)
        );
        let n = &pa.neighborhood;
        assert_eq!(
            (n.ahead.len(), n.skipped),
            (pa.ahead, 0),
            "{} {}",
            a.list.name,
            a.group
        );
        assert_eq!(n.tied, pu.neighborhood.tied);
        assert_eq!(n.behind, pu.neighborhood.behind);
    }
    // Texas All HS, T379: all 378 ahead, best first, where the usual
    // Neighborhood shows 4 and skips 374.
    let tx = all
        .results
        .iter()
        .find(|x| x.list.id == TEXAS && x.grades.len() == 4)
        .unwrap();
    let ahead = &tx.place.as_ref().unwrap().neighborhood.ahead;
    assert_eq!(ahead.len(), 378);
    assert!(ahead.windows(2).all(|w| w[0].sort_int <= w[1].sort_int));
    assert!(
        ahead
            .iter()
            .all(|x| x.sort_int < MARK && x.athlete_id != SUBJECT)
    );
    let ids: BTreeSet<i64> = ahead.iter().map(|x| x.athlete_id).collect();
    assert_eq!(ids.len(), 378, "nobody twice across the page overlaps");
    Ok(())
}

#[tokio::test]
async fn no_mark_yet_is_a_result_not_an_error() -> anyhow::Result<()> {
    // Bio 3001: school 5103, no 400m results, no grade on the Profile.
    let src = Counting::new(source());
    let r = rank(&src, &made_up_query(3001, ca_tx())).await?;
    assert_eq!(r.status, RankStatus::NoMark);
    assert!(r.mark.is_none());
    assert_eq!(src.pages.load(Ordering::Relaxed), 0, "no list is read");
    // No grade: all of high school only, and a note saying so.
    assert_eq!(r.groups, [vec![9, 10, 11, 12]]);
    assert!(
        r.results
            .iter()
            .all(|x| x.status == RankStatus::NoMark && x.place.is_none())
    );
    assert_eq!(r.results.len(), 2);
    let note = r.note.unwrap();
    assert!(
        note.contains("no 2026 400m mark on the California list yet"),
        "{note}"
    );
    assert!(note.contains("grade for 2026 is unknown"), "{note}");
    Ok(())
}

#[tokio::test]
async fn home_is_the_schools_state_and_area() -> anyhow::Result<()> {
    let r = rank(&source(), &made_up_query(3001, Targets::home())).await?;
    let lists: Vec<(&str, ListKind)> = r
        .results
        .iter()
        .map(|x| (x.list.name.as_str(), x.list.kind))
        .collect();
    assert_eq!(
        lists,
        [
            ("California", ListKind::State),
            ("Central Coast", ListKind::Area)
        ]
    );
    Ok(())
}

#[tokio::test]
async fn a_grade_off_athlete_is_found_on_the_every_grade_list() -> anyhow::Result<()> {
    // As a 10th grader (--grade 10), her grade's list does not have her.
    let page = |rows: serde_json::Value| -> RankingsPage {
        serde_json::from_value(serde_json::json!({
            "eventShort": "400m",
            "division": {"ID": CALIFORNIA, "SeasonID": SEASON, "Name": "California"},
            "groupedRankings": [rows],
        }))
        .unwrap()
    };
    let q = |grades| PageQuery {
        list_id: CALIFORNIA,
        season: SEASON,
        gender: Gender::F,
        event: "400m",
        grades,
        page: 1,
    };
    let (tenth, all) = (Grades::only(10), Grades::all());
    let row = |id: i64, mark: i64| serde_json::json!({"AthleteID": id, "AthleteName": "X", "display": "x", "SortIntCalc": mark, "GradeID": 10});
    let src = source()
        .with_rankings_page(&q(&tenth), page(serde_json::json!([row(1, 55_000)])))
        .with_rankings_page(
            &q(&all),
            page(serde_json::json!([
                row(1, 55_000),
                {"AthleteID": SUBJECT, "AthleteName": "Jamie Uddumcombe", "display": "58.98",
                 "SortIntCalc": MARK, "GradeID": 9}
            ])),
        );
    let mut rq = query(SUBJECT, Targets::parse("TX").unwrap());
    rq.grade = Some(10);
    let r = rank(&src, &rq).await?;
    let mark = r.mark.as_ref().unwrap();
    assert_eq!((mark.sort_int, mark.grade), (MARK, Some(9)));
    assert_eq!(
        mark.list.url,
        "https://www.athletic.net/TrackAndField/rankings/list/985316289/f/400m"
    );
    // Grade groups follow the grade asked for.
    assert_eq!(r.grade, Some(10));
    assert_eq!(labels(&r, TEXAS), ["T132", "T262", "T379"]);
    Ok(())
}

#[tokio::test]
async fn the_search_for_her_row_stops_past_her_slowest_mark() -> anyhow::Result<()> {
    // A 9th grader at her school whose only 400m is 58.00, on no list.
    // California's 9th-grade page 1 ends at 1:00.83, past her, so page 2 is
    // never read (there is no page 2 recorded: reading it would fail this
    // test); the every-grade list likewise stops at its first page.
    let bio: Bio = serde_json::from_value(serde_json::json!({
        "athlete": {"IDAthlete": 4001, "SchoolID": 920_353_302, "FirstName": "Off", "LastName": "List", "Gender": "F"},
        "allTeams": {"920353302": {"IDSchool": 920_353_302, "SchoolName": "Cranevale Heights", "Level": 4, "Year": SEASON}},
        "grades": {"920353302_2099": 9},
        "eventsTF": [{"Event": "400 Meters", "IDEvent": 21, "IDEventType": 0}],
        "resultsTF": [{"AthleteID": 4001, "Result": "58.00a", "SortInt": 58000, "EventID": 21,
                       "EventTypeID": 0, "SeasonID": SEASON, "Exhibition": false,
                       "ResultDate": "2099-04-01T00:00:00"}]
    }))?;
    let all = Grades::all();
    let full_page: RankingsPage = serde_json::from_value(serde_json::json!({
        "eventShort": "400m",
        "division": {"ID": CALIFORNIA, "SeasonID": SEASON, "Name": "California"},
        "groupedRankings": [(1..=101).map(|n| serde_json::json!({
            "AthleteID": n, "AthleteName": "X", "display": "x", "SortIntCalc": 50_000 + 100 * n
        })).collect::<Vec<_>>()],
    }))?;
    let all_q = PageQuery {
        list_id: CALIFORNIA,
        season: SEASON,
        gender: Gender::F,
        event: "400m",
        grades: &all,
        page: 1,
    };
    let src = Counting::new(source().with_bio(bio).with_rankings_page(&all_q, full_page));
    let r = rank(&src, &query(4001, Targets::parse("TX").unwrap())).await?;
    assert_eq!(r.status, RankStatus::NoMark);
    assert_eq!(
        src.pages.load(Ordering::Relaxed),
        2,
        "one page of each list"
    );
    let note = r.note.unwrap();
    assert!(
        note.contains("read only down past the slowest 2099 mark") && note.contains("--refresh"),
        "{note}"
    );
    Ok(())
}

#[tokio::test]
async fn a_home_state_from_another_seasons_school_is_noted() -> anyhow::Result<()> {
    // Bio 3002's high school is listed only through 2025.
    let r = rank(
        &source(),
        &made_up_query(3002, Targets::parse("CA").unwrap()),
    )
    .await?;
    let note = r.note.unwrap();
    assert!(
        note.contains(
            "home State CA is from Seacliff Prep (last listed 2025), as Jordan Sample's \
             Profile lists no school for 2026"
        ),
        "{note}"
    );
    assert!(note.contains("pass --home"), "{note}");
    Ok(())
}

#[tokio::test]
async fn an_unknown_list_name_is_an_error() {
    let err = rank(
        &source(),
        &query(SUBJECT, Targets::parse("CA,Bay").unwrap()),
    )
    .await
    .unwrap_err();
    assert!(
        format!("{err:#}").contains("no 2099 Area is named \"Bay\""),
        "{err:#}"
    );
}

/// Every key path in a JSON value: `a.b`, `a[].c` (arrays merged).
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

/// rankmap reads this JSON: a renamed or removed field breaks it, so
/// the shape is pinned here. Bump `Ranks::SCHEMA` with any change to it.
#[tokio::test]
async fn the_json_shape_is_pinned() -> anyhow::Result<()> {
    // `home` so `home_area` is filled in and its shape pinned too.
    let ranks = rank(
        &source(),
        &query(SUBJECT, Targets::parse("home,CA,TX").unwrap()),
    )
    .await?;
    let json: serde_json::Value = serde_json::from_str(&ath::contract::to_json(&ranks)?)?;
    assert_eq!(json["schema"], 1);
    let mut paths = BTreeSet::new();
    key_paths(&json, "", &mut paths);
    let row = [
        "athlete_id",
        "date",
        "grade",
        "mark",
        "meet",
        "meet_id",
        "name",
        "place",
        "rank",
        "sort_int",
        "team",
        "team_id",
        "tied",
        "url",
    ];
    let mut want: BTreeSet<String> = [
        "schema",
        "athlete",
        "athlete.id",
        "athlete.name",
        "athlete.first_name",
        "athlete.url",
        "season",
        "gender",
        "event",
        "event_name",
        "grade",
        "status",
        "mark",
        "mark.mark",
        "mark.sort_int",
        "mark.grade",
        "mark.team",
        "mark.team_id",
        "mark.list",
        "mark.list.id",
        "mark.list.name",
        "mark.list.url",
        "groups",
        "home_area",
        "home_area.id",
        "home_area.name",
        "home_area.level",
        "home_area.state",
        "home_area.season",
        "home_area.url",
        "note",
        "results",
        "results[].list",
        "results[].list.id",
        "results[].list.name",
        "results[].list.kind",
        "results[].list.state",
        "results[].list.level",
        "results[].grades",
        "results[].group",
        "results[].url",
        "results[].status",
        "results[].place",
        "results[].place.rank",
        "results[].place.tied",
        "results[].place.label",
        "results[].place.ahead",
        "results[].place.ties",
        "results[].place.neighborhood",
        "results[].place.neighborhood.ahead",
        "results[].place.neighborhood.skipped",
        "results[].place.neighborhood.tied",
        "results[].place.neighborhood.behind",
        "results[].pages",
        "results[].note",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    for part in ["ahead", "tied", "behind"] {
        for key in row {
            want.insert(format!("results[].place.neighborhood.{part}[].{key}"));
        }
    }
    assert_eq!(paths, want);
    Ok(())
}

// ---------------------------------------------------------------------------
// The binary

/// `ath` on the anonymized fixtures, its clock pinned to their season.
fn ath(args: &[&str]) -> Output {
    common::ath()
        .args(args)
        .env("ATH_TODAY", TODAY)
        .output()
        .expect("running ath")
}

#[test]
fn ath_rank_prints_a_grid() -> anyhow::Result<()> {
    let out = ath(&[
        "rank",
        "982389051",
        "--event",
        "400m",
        "--season",
        "2099",
        "--in",
        "CA,TX",
        "--table",
        "--fixtures-dir",
        FIXTURES,
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8(out.stdout)?,
        "Jamie Uddumcombe 2099 girls 400 Meters, grade 9: 58.98 on the California list\n\
         https://www.athletic.net/athlete/982389051/track-and-field\n\
         \n\
         List             9th  9th-10th  9th-11th  All HS\n\
         California (CA)  42   109       T195      T303\n\
         Texas (TX)       T56  T132      T262      T379\n"
    );
    Ok(())
}

#[test]
fn ath_rank_fails_when_no_list_can_be_read() -> anyhow::Result<()> {
    // New York's lists are not recorded: every one fails to read.
    let out = ath(&[
        "rank",
        "982389051",
        "--event",
        "400m",
        "--season",
        "2099",
        "--in",
        "NY",
        "--json",
        "--fixtures-dir",
        FIXTURES,
    ]);
    assert!(!out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    assert!(
        json["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["status"] == "lookup_failed")
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("none of the 1 list(s) could be read, for any Grade group (4 results)"),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn ath_rank_json_is_the_report() -> anyhow::Result<()> {
    let out = ath(&[
        "rank",
        "982389051",
        "--event",
        "400m",
        "--season",
        "2099",
        "--in",
        "CA,TX",
        "--json",
        "--fixtures-dir",
        FIXTURES,
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rt = tokio::runtime::Runtime::new()?;
    let want = ath::contract::to_json(&rt.block_on(subject())?)?;
    assert_eq!(String::from_utf8(out.stdout)?, format!("{want}\n"));
    Ok(())
}

/// The Python reports' tests read saved `ath rank` reports, so a change to the
/// JSON fails here until the copy at `path` is refreshed (`ATH_BLESS=1 cargo
/// test`), and then fails on the Python side if the report cannot read it
/// (`tests/tools.rs`).
fn assert_saved_report_is_current(path: &str, args: &[&str]) -> anyhow::Result<()> {
    let out = ath(args);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    if std::env::var_os("ATH_BLESS").is_some() {
        std::fs::write(path, &out.stdout)?;
    }
    assert!(
        std::fs::read(path)? == out.stdout,
        "{path} is not what `ath rank` prints now; refresh it with ATH_BLESS=1 cargo test"
    );
    Ok(())
}

/// rankmap's: a State and its Area (California, and Central Coast), from the
/// anonymized fixtures.
#[test]
fn rankmaps_saved_report_is_current() -> anyhow::Result<()> {
    assert_saved_report_is_current(
        "tools/rankmap/testdata/ath-rank-2099.json",
        &[
            "rank",
            "982389051",
            "--event",
            "400m",
            "--season",
            "2099",
            "--in",
            "CA,CCS",
            "--ahead",
            "all",
            "--json",
            "--fixtures-dir",
            FIXTURES,
        ],
    )
}
