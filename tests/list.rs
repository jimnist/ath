//! `ath list`, a whole Rankings list, end to end through the
//! fixture-backed data source and the `ath` binary.
//!
//! Fixtures (`tests/fixtures/rankings/`), anonymized from real recordings (ADR
//! 0010, `docs/anonymize.md`): invented names and ids, the fictional season
//! 2099 (2098 the one before), marks moved by a rank-preserving map; lengths,
//! page seams and Athletic.net's way with ties as recorded:
//!
//! - `996953819-f-400m-9_10-p1..p6.json`: the Central Coast Section girls'
//!   400m list for grades 9-10, all six pages: 584 people, 101 rows on page 1
//!   and 102 on the next four (each repeats the two rows before it), 85 on the
//!   last, `minCount` 584 on every page.
//! - `996953819-f-SP-all-p1.json`: what Athletic.net answers for the event
//!   code `SP`, which it does not know: the 100m list (first page only).
//! - `996233208-f-400m-all-p1.json`: list 996233208, Central Coast the season
//!   before (first page only).
//! - `985316289-f-400m-9_10_11_12-p1.json`: California's All HS list, page 1,
//!   whose 11th and 12th rows are two equal marks Athletic.net ranks 11 and
//!   12.
//! - `900400-f-400m-all-p1.json` (made up): a capped list as an
//!   unpaid session sees it, five invented people then a Masked row shaped
//!   like the one seen live 2026-11-06.
//!
//! `--area` resolves through `navinfo/2099.json`.

mod common;

use std::collections::HashSet;
use std::process::Output;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use ath::area::{AreaTree, Gender, Region, TeamNav};
use ath::contract::to_json;
use ath::list::{
    Capped, Grades, ListKind, ListQuery, PageQuery, RankingsList, RankingsPage, RankingsRow, Which,
    rankings_list,
};
use ath::meet::{EntriesData, EntriesQuery, MeetData, MeetEventList};
use ath::model::{Bio, Candidate};
use ath::source::{DataSource, FixtureSource};

const FIXTURES: &str = "tests/fixtures";
/// The anonymized fixtures' season, and their clock (`ATH_TODAY`).
const SEASON: i64 = 2099;
const TODAY: &str = "2099-10-30";
const CENTRAL_COAST: i64 = 996_953_819;
const CALIFORNIA: &str = "985316289";

fn source() -> FixtureSource {
    FixtureSource::from_dir(FIXTURES).unwrap()
}

fn query<'a>(which: Which<'a>, event: &str, grades: &str) -> ListQuery<'a> {
    ListQuery {
        which,
        season: 2026,
        gender: Gender::F,
        event: event.to_string(),
        grades: Grades::parse(grades).unwrap(),
        limit: None,
        from_list: 0,
    }
}

/// [`query`] in the anonymized fixtures' season.
fn anon_query<'a>(which: Which<'a>, event: &str, grades: &str) -> ListQuery<'a> {
    ListQuery {
        season: SEASON,
        ..query(which, event, grades)
    }
}

/// The athletes on the recorded Central Coast 9-10 pages, page by page in
/// Athletic.net's order, each once (the pages overlap), and who stands at
/// each `rowNum`.
fn recorded_ccs() -> (Vec<i64>, std::collections::HashMap<i64, i64>) {
    let (mut ids, mut at) = (Vec::new(), std::collections::HashMap::new());
    let mut seen = HashSet::new();
    for n in 1..=6 {
        let path = format!("{FIXTURES}/rankings/996953819-f-400m-9_10-p{n}.json");
        let page: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        for row in page["groupedRankings"][0].as_array().unwrap() {
            let id = row["AthleteID"].as_i64().unwrap();
            at.entry(row["rowNum"].as_i64().unwrap()).or_insert(id);
            if seen.insert(id) {
                ids.push(id);
            }
        }
    }
    (ids, at)
}

fn ccs() -> ListQuery<'static> {
    anon_query(
        Which::Area {
            name: "CCS",
            state: None,
        },
        "400m",
        "9-10",
    )
}

/// Wraps the fixtures, counting the pages asked for.
struct Counting {
    inner: FixtureSource,
    pages: AtomicU64,
}

impl Counting {
    fn new() -> Self {
        Self {
            inner: source(),
            pages: AtomicU64::new(0),
        }
    }
}

#[async_trait]
impl DataSource for Counting {
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

async fn read(src: &impl DataSource, q: &ListQuery<'_>) -> anyhow::Result<RankingsList> {
    rankings_list(src, q).await
}

#[tokio::test]
async fn the_whole_list_is_read_across_its_pages() -> anyhow::Result<()> {
    let src = Counting::new();
    let list = read(&src, &ccs()).await?;
    assert_eq!(src.pages.load(Ordering::Relaxed), 6, "every page, once");

    // The list's real length: 584 people, though the six pages hold 594 rows.
    assert_eq!(
        (list.count, list.total, list.complete),
        (584, Some(584), true)
    );
    assert_eq!(list.rows.len(), 584);
    assert_eq!(list.note, None);

    // Nobody twice, even at the seams (rows 100-101, 200-201, ... repeat).
    let ids: HashSet<i64> = list.rows.iter().map(|r| r.athlete_id).collect();
    assert_eq!(ids.len(), 584);
    let (recorded, at) = recorded_ccs();
    for n in [100, 101, 200, 201, 500, 501] {
        assert_eq!(
            list.rows.iter().filter(|r| r.athlete_id == at[&n]).count(),
            1,
            "row {n}"
        );
    }

    // In SortIntCalc order, best first, and in Athletic.net's own order.
    assert!(list.rows.windows(2).all(|w| w[0].sort_int <= w[1].sort_int));
    assert_eq!(
        list.rows.iter().map(|r| r.athlete_id).collect::<Vec<_>>(),
        recorded
    );

    let first = &list.rows[0];
    assert_eq!(
        (
            first.rank,
            first.mark.as_str(),
            first.sort_int,
            first.grade,
            first.team.as_deref(),
            first.date.as_deref(),
        ),
        (
            Some(1),
            "55.39",
            55390,
            Some(10),
            Some("Cinderview Falls"),
            Some("2099-05-19")
        )
    );
    assert_eq!(
        first.url,
        format!(
            "https://www.athletic.net/athlete/{}/track-and-field",
            first.athlete_id
        )
    );
    // Tied marks share Athletic.net's rank.
    let tied: Vec<_> = list.rows.iter().filter(|r| r.rank == Some(159)).collect();
    assert_eq!(tied.len(), 2);
    assert_eq!(tied[0].sort_int, tied[1].sort_int);
    assert_eq!(list.rows[583].rank, Some(584));
    Ok(())
}

#[tokio::test]
async fn the_list_says_what_it_is() -> anyhow::Result<()> {
    let list = read(&source(), &ccs()).await?;
    assert_eq!(list.list.id, CENTRAL_COAST);
    assert_eq!(list.list.name, "Central Coast");
    assert_eq!(list.list.kind, ListKind::Area);
    assert_eq!(list.list.state.as_deref(), Some("CA"));
    assert_eq!(list.list.level.as_deref(), Some("Section"));
    assert_eq!(
        (list.event.as_str(), list.event_name.as_deref()),
        ("400m", Some("400 Meters"))
    );
    assert_eq!(list.grades, Some(vec![9, 10]));
    // The defaults Athletic.net applied, which ath keeps.
    assert_eq!((list.fat, list.official), (Some(true), Some(true)));
    assert_eq!(
        list.url,
        "https://www.athletic.net/TrackAndField/rankings/list/996953819/f/400m?grades=9,10"
    );
    Ok(())
}

#[tokio::test]
async fn an_area_is_found_by_name_or_abbreviation_and_a_list_by_id() -> anyhow::Result<()> {
    for which in [
        Which::Area {
            name: "central coast",
            state: Some("ca"),
        },
        Which::Area {
            name: " ccs ",
            state: None,
        },
        Which::List(CENTRAL_COAST),
    ] {
        let list = read(&source(), &anon_query(which, "400m", "9-10")).await?;
        assert_eq!(
            (list.list.id, list.count),
            (CENTRAL_COAST, 584),
            "{which:?}"
        );
    }
    let list = read(
        &source(),
        &anon_query(Which::List(CENTRAL_COAST), "400m", "9-10"),
    )
    .await?;
    assert_eq!(list.list.kind, ListKind::List);
    assert_eq!(list.list.state, None);
    Ok(())
}

#[tokio::test]
async fn limit_reads_only_the_pages_it_needs() -> anyhow::Result<()> {
    let src = Counting::new();
    let q = ListQuery {
        limit: Some(150),
        ..ccs()
    };
    let list = read(&src, &q).await?;
    assert_eq!(src.pages.load(Ordering::Relaxed), 2);
    assert_eq!((list.count, list.limit), (150, Some(150)));
    // The whole list was not read, so its length is not known.
    assert_eq!((list.total, list.complete), (None, false));
    assert_eq!(list.rows.last().unwrap().athlete_id, recorded_ccs().0[149]);

    // A limit past the end reads the whole list, which is then complete.
    let q = ListQuery {
        limit: Some(1000),
        ..ccs()
    };
    let list = read(&source(), &q).await?;
    assert_eq!(
        (list.count, list.total, list.complete),
        (584, Some(584), true)
    );
    Ok(())
}

#[tokio::test]
async fn an_unknown_event_code_is_refused() {
    // Athletic.net answers `SP` with its 100m list.
    let q = anon_query(Which::List(CENTRAL_COAST), "SP", "all");
    let err = format!("{:#}", read(&source(), &q).await.unwrap_err());
    assert!(
        err.contains("does not know the event code \"SP\": it answered with the 100m list"),
        "{err}"
    );
}

#[tokio::test]
async fn a_list_in_another_season_is_refused() {
    let q = anon_query(Which::List(996_233_208), "400m", "all");
    let err = format!("{:#}", read(&source(), &q).await.unwrap_err());
    assert!(
        err.contains("rankings list 996233208 is in season 2098, not 2099"),
        "{err}"
    );
}

#[tokio::test]
async fn an_unknown_area_lists_the_areas_there_are() {
    let q = anon_query(
        Which::Area {
            name: "Bay",
            state: None,
        },
        "400m",
        "9-10",
    );
    let err = format!("{:#}", read(&source(), &q).await.unwrap_err());
    assert!(err.contains("no 2099 Area is named \"Bay\""), "{err}");
    assert!(err.contains("Central Coast (CCS)"), "{err}");
}

// ---------------------------------------------------------------------------
// Synthetic pages, for answers the recorded list does not show

const SYNTH: i64 = 4242;

/// A page of list [`SYNTH`] holding rows `rows` (by `rowNum`), each person's
/// id from `id(rowNum)`, with `extra` merged over the top-level fields.
fn page(
    rows: impl IntoIterator<Item = i64>,
    id: impl Fn(i64) -> i64,
    extra: serde_json::Value,
) -> RankingsPage {
    serde_json::from_value(page_json(rows, id, extra)).unwrap()
}

/// [`page`] as Athletic.net would send it, before parsing.
fn page_json(
    rows: impl IntoIterator<Item = i64>,
    id: impl Fn(i64) -> i64,
    extra: serde_json::Value,
) -> serde_json::Value {
    let rows: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|n| {
            serde_json::json!({
                "rank": n, "rowNum": n, "AthleteID": id(n), "AthleteName": format!("P{n}"),
                "display": "1:00.00", "SortIntCalc": 60_000 + n, "Event": "400 Meters",
            })
        })
        .collect();
    let mut json = serde_json::json!({
        "eventShort": "400m",
        "division": {"ID": SYNTH, "SeasonID": 2026, "Name": "Synthetic"},
        "minCount": 0,
        "settings": {"fat": true, "official": true},
        "groupedRankings": [rows],
    });
    for (k, v) in extra.as_object().unwrap() {
        json[k] = v.clone();
    }
    json
}

fn synth_page(n: u32) -> PageQuery<'static> {
    static ALL: std::sync::LazyLock<Grades> = std::sync::LazyLock::new(Grades::all);
    PageQuery {
        list_id: SYNTH,
        season: 2026,
        gender: Gender::F,
        event: "400m",
        grades: &ALL,
        page: n,
    }
}

async fn read_synth(pages: Vec<RankingsPage>) -> anyhow::Result<RankingsList> {
    let src = pages
        .into_iter()
        .zip(1..)
        .fold(FixtureSource::new(), |src, (p, n)| {
            src.with_rankings_page(&synth_page(n), p)
        });
    read(&src, &query(Which::List(SYNTH), "400m", "all")).await
}

fn person(n: i64) -> i64 {
    100_000 + n
}

#[tokio::test]
async fn an_empty_page_past_the_end_says_nothing_about_the_length() -> anyhow::Result<()> {
    // Exactly 101 rows: page 1 is full, so page 2 is read, and is empty.
    let list = read_synth(vec![
        page(1..=101, person, serde_json::json!({"minCount": 101})),
        page([], person, serde_json::json!({"minCount": 0})),
    ])
    .await?;
    assert_eq!((list.count, list.total, list.note), (101, Some(101), None));

    // Such a page may not even say its list: only page 1 must.
    let list = read_synth(vec![
        page(1..=101, person, serde_json::json!({"minCount": 101})),
        page([], person, serde_json::json!({"division": null})),
    ])
    .await?;
    assert_eq!(list.count, 101);
    Ok(())
}

#[tokio::test]
async fn a_length_that_disagrees_with_the_rows_is_noted() -> anyhow::Result<()> {
    // The rows reach row 41, the length stated, but row 20 is missing (the
    // list changed while it was read). Rows that stop short of the length
    // are a capped list instead (below).
    let list = read_synth(vec![page(
        (1..=41).filter(|n| *n != 20),
        person,
        serde_json::json!({"minCount": 41}),
    )])
    .await?;
    let note = list.note.unwrap();
    assert!(
        note.contains("says the list has 41 rows but its pages gave 40"),
        "{note}"
    );
    Ok(())
}

#[tokio::test]
async fn pages_that_disagree_at_their_seam_are_noted() -> anyhow::Result<()> {
    // Page 2 repeats rows 100 and 101, but row 100's mark is another now.
    let mut later = page(100..=150, person, serde_json::json!({"minCount": 150}));
    later.grouped_rankings[0][0].sort_int -= 500;
    let list = read_synth(vec![
        page(1..=101, person, serde_json::json!({"minCount": 150})),
        later,
    ])
    .await?;
    let note = list.note.unwrap();
    assert!(
        note.contains("pages 1 and 2 disagree where they overlap"),
        "{note}"
    );

    // Agreeing seams say nothing.
    let list = read_synth(vec![
        page(1..=101, person, serde_json::json!({"minCount": 150})),
        page(100..=150, person, serde_json::json!({"minCount": 150})),
    ])
    .await?;
    assert_eq!((list.count, list.note), (150, None));

    // Nor do tied people trading places between requests: same marks.
    let swapped = |n| match n {
        100 => person(101),
        101 => person(100),
        n => person(n),
    };
    let mut first = page(1..=101, person, serde_json::json!({"minCount": 150}));
    let mut second = page(100..=150, swapped, serde_json::json!({"minCount": 150}));
    for p in [&mut first, &mut second] {
        for row in &mut p.grouped_rankings[0] {
            if row.row_num == Some(101) {
                row.sort_int -= 1;
            }
        }
    }
    let list = read_synth(vec![first, second]).await?;
    assert_eq!((list.count, list.note), (150, None));
    Ok(())
}

#[tokio::test]
async fn a_list_athletic_net_does_not_have_is_refused() {
    // What a made-up list id gets: no division and no rows (checked live).
    let err = read_synth(vec![page(
        [],
        person,
        serde_json::json!({"division": null}),
    )])
    .await
    .unwrap_err();
    assert!(
        format!("{err:#}").contains("Athletic.net has no rankings list 4242"),
        "{err:#}"
    );
    let err = read_synth(vec![page(
        1..=3,
        person,
        serde_json::json!({"division": {"ID": 7, "SeasonID": 2026}}),
    )])
    .await
    .unwrap_err();
    assert!(
        format!("{err:#}").contains("answered for rankings list 4242 with list 7"),
        "{err:#}"
    );
}

#[tokio::test]
async fn a_relay_list_is_refused() {
    // `4x400m` rows are teams, with their runners in `relayTeams`.
    let relay = serde_json::json!({"relayTeams": {"274132165": {"Members": []}}});
    let err = read_synth(vec![page(1..=3, person, relay)])
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("is a relay"), "{err:#}");
}

// ---------------------------------------------------------------------------
// Capped lists: Masked rows past what an unpaid session may see

/// A Masked row as seen live 2026-11-06 (signed in, unpaid), at `row_num`.
fn masked_json(row_num: i64) -> serde_json::Value {
    serde_json::json!({
        "rank": row_num, "rowNum": row_num, "AthleteID": 0, "AthleteName": "Xxxxxx Xxxxxx",
        "GradeID": 99, "display": "", "SortIntCalc": 60_000 + row_num, "TeamID": 0,
        "TeamName": "Xxxxxx Xxxxxx", "MeetID": 0, "MeetName": "XXX Xxxxxxx Xxxxxx",
        "ResultDate": "2026-05-23T00:00:00", "Event": "400 Meters",
    })
}

fn masked(row_num: i64) -> RankingsRow {
    serde_json::from_value(masked_json(row_num)).unwrap()
}

/// The top-level fields of a Testland page saying the list has at least 696.
fn testland() -> serde_json::Value {
    serde_json::json!({"minCount": 696,
        "division": {"ID": SYNTH, "SeasonID": 2026, "Name": "Testland"}})
}

/// A page of `rows` people then `tail` Masked rows, saying the list has at
/// least 696.
fn capped_page(rows: std::ops::RangeInclusive<i64>, tail: i64) -> RankingsPage {
    let next = rows.end() + 1;
    let mut json = page_json(rows, person, testland());
    json["groupedRankings"][0]
        .as_array_mut()
        .unwrap()
        .extend((next..next + tail).map(masked_json));
    serde_json::from_value(json).unwrap()
}

#[tokio::test]
async fn a_masked_tail_is_a_capped_list_not_a_short_one() {
    // What an unpaid session got for a 696-row list: 25 people, then one
    // Masked row. Not a 26-row list with a note: an error, nothing ranked.
    let err = read_synth(vec![capped_page(1..=25, 1)]).await.unwrap_err();
    assert!(err.downcast_ref::<Capped>().is_some(), "{err:#}");
    assert_eq!(
        format!("{err:#}"),
        "Athletic.net returned only 25 of at least 696 rows for Testland 2026 girls 400m (all \
         grades); full lists need a paid Athletic.net subscription, signed in where ath reads \
         Athletic.net (the relay tab's browser, or the browser the cookie came from)"
    );

    // Several Masked rows (they share AthleteID 0) are the same cap.
    let err = read_synth(vec![capped_page(1..=5, 20)]).await.unwrap_err();
    assert!(
        format!("{err:#}").starts_with("Athletic.net returned only 5 of at least 696 rows"),
        "{err:#}"
    );
}

#[tokio::test]
async fn a_short_page_ending_at_a_known_cap_is_capped() {
    // A session may get its 5 (signed out) or 25 (unpaid) rows with no Masked
    // row after them: they still stop at a cap, short of the stated length.
    for cap in [5, 25] {
        let p = page(1..=cap, person, testland());
        assert!(p.is_capped_after(0), "{cap}");
        let err = read_synth(vec![p]).await.unwrap_err();
        assert!(err.downcast_ref::<Capped>().is_some(), "{err:#}");
        assert!(
            format!("{err:#}").starts_with(&format!(
                "Athletic.net returned only {cap} of at least 696 rows for Testland 2026 girls \
                 400m"
            )),
            "{err:#}"
        );
    }
}

#[tokio::test]
async fn a_short_page_a_little_below_its_length_is_only_noted() -> anyhow::Result<()> {
    // A paid list whose count lags its rows: 101 rows read, 102 stated. Not
    // a known cap, so the whole list with a note, not an error.
    let list = read_synth(vec![
        page(1..=101, person, serde_json::json!({"minCount": 102})),
        page(100..=101, person, serde_json::json!({"minCount": 102})),
    ])
    .await?;
    assert_eq!((list.count, list.complete), (101, true));
    assert!(
        list.note
            .unwrap()
            .contains("says the list has 102 rows but its pages gave 101")
    );
    // The same on a first page.
    let list = read_synth(vec![page(
        1..=40,
        person,
        serde_json::json!({"minCount": 42}),
    )])
    .await?;
    assert_eq!((list.count, list.complete), (40, true));
    Ok(())
}

#[tokio::test]
async fn an_empty_page_below_the_stated_length_is_capped() {
    // Signed out, page 1 may hold nobody at all.
    let err = read_synth(vec![page([], person, testland())])
        .await
        .unwrap_err();
    assert!(err.downcast_ref::<Capped>().is_some(), "{err:#}");
    assert!(
        format!("{err:#}").starts_with("Athletic.net returned only 0 of at least 696 rows"),
        "{err:#}"
    );

    // A page 1 cached by a paid run, then page 2 fetched live by an unpaid
    // session: nobody, though the list is longer than the rows read.
    let err = read_synth(vec![
        page(1..=101, person, serde_json::json!({"minCount": 696})),
        page([], person, testland()),
    ])
    .await
    .unwrap_err();
    assert!(
        format!("{err:#}").starts_with("Athletic.net returned only 101 of at least 696 rows"),
        "{err:#}"
    );
}

#[tokio::test]
async fn a_short_list_that_reaches_its_length_is_whole() -> anyhow::Result<()> {
    // A small list on one page: its rows reach the length it states.
    let p = page(1..=40, person, serde_json::json!({"minCount": 40}));
    assert!(!p.is_capped_after(0));
    let list = read_synth(vec![p]).await?;
    assert_eq!(
        (list.count, list.total, list.complete, list.note),
        (40, Some(40), true, None)
    );
    // Even one that happens to be 25 long.
    let list = read_synth(vec![page(
        1..=25,
        person,
        serde_json::json!({"minCount": 25}),
    )])
    .await?;
    assert_eq!((list.count, list.complete), (25, true));
    // Or past it (a stated length below the rows is only noted).
    let list = read_synth(vec![page(
        1..=40,
        person,
        serde_json::json!({"minCount": 39}),
    )])
    .await?;
    assert_eq!((list.count, list.complete), (40, true));
    assert!(
        list.note
            .unwrap()
            .contains("says the list has 39 rows but its pages gave 40")
    );
    Ok(())
}

#[tokio::test]
async fn a_limit_within_the_rows_shown_is_answered_from_them() -> anyhow::Result<()> {
    let src = FixtureSource::new().with_rankings_page(&synth_page(1), capped_page(1..=25, 1));
    let q = |limit| ListQuery {
        limit: Some(limit),
        ..query(Which::List(SYNTH), "400m", "all")
    };
    let list = read(&src, &q(10)).await?;
    assert_eq!((list.count, list.total, list.complete), (10, None, false));
    assert!(list.rows.iter().all(|r| r.athlete_id != 0));
    assert_eq!(list.rows[9].athlete_id, person(10));
    assert_eq!(list.note, None, "row 11 shows row 10's tie");

    // All 25 rows shown: printed, but the 25th row's tie is past the cap.
    let list = read(&src, &q(25)).await?;
    assert_eq!((list.count, list.complete), (25, false));
    assert!(list.rows.iter().all(|r| r.athlete_id != 0));
    let note = list.note.unwrap();
    assert!(
        note.contains("whether row 25 ties the row after it is unknown"),
        "{note}"
    );

    // More than are shown needs rows past the cap.
    let err = read(&src, &q(26)).await.unwrap_err();
    assert!(err.downcast_ref::<Capped>().is_some(), "{err:#}");
    Ok(())
}

#[tokio::test]
async fn a_masked_row_in_the_middle_of_a_page_is_refused() {
    // Masked rows are only ever a tail; one with people after it is a shape
    // ath does not know, and dropping it would shift everyone's place.
    let mut json = page_json(1..=5, person, testland());
    let rows = json["groupedRankings"][0].as_array_mut().unwrap();
    rows.insert(2, masked_json(3));
    let p: RankingsPage = serde_json::from_value(json).unwrap();
    assert!(p.masked_mid);
    let err = read_synth(vec![p]).await.unwrap_err();
    assert!(
        format!("{err:#}").starts_with(
            "unexpected masked row in the middle of Testland 2026 girls 400m (all grades)"
        ),
        "{err:#}"
    );
}

#[tokio::test]
async fn capped_counts_rows_as_athletic_net_numbers_them() {
    // The same person twice in the rows shown (rows 2 and 3): 4 people, but
    // Athletic.net showed 5 rows.
    let twice = |n| if n == 3 { person(2) } else { person(n) };
    let mut json = page_json(1..=5, twice, testland());
    json["groupedRankings"][0]
        .as_array_mut()
        .unwrap()
        .push(masked_json(6));
    let err = read_synth(vec![serde_json::from_value(json).unwrap()])
        .await
        .unwrap_err();
    assert!(
        format!("{err:#}").starts_with("Athletic.net returned only 5 of at least 696 rows"),
        "{err:#}"
    );
}

#[tokio::test]
async fn a_masked_row_on_a_later_page_counts_the_people_before_it() {
    // Page 2 repeats rows 100 and 101 before its new people.
    let err = read_synth(vec![
        page(1..=101, person, serde_json::json!({"minCount": 150})),
        capped_page(100..=124, 1),
    ])
    .await
    .unwrap_err();
    assert!(
        format!("{err:#}").starts_with("Athletic.net returned only 124 of at least 696 rows"),
        "{err:#}"
    );
}

#[tokio::test]
async fn a_capped_list_without_a_length_or_name_still_says_so() {
    let mut p = capped_page(1..=5, 1);
    p.min_count = None;
    p.division.as_mut().unwrap().name = None;
    let err = read_synth(vec![p]).await.unwrap_err();
    assert!(
        format!("{err:#}").starts_with(
            "Athletic.net returned only 5 rows for rankings list 4242 2026 girls 400m (all \
             grades); full lists"
        ),
        "{err:#}"
    );
}

#[test]
fn a_row_without_an_athlete_id_key_is_a_parse_error_not_a_masked_row() {
    // A renamed field on a paid list must not read as a paywall.
    let row = serde_json::from_value::<RankingsRow>(
        serde_json::json!({"AthleteName": "Rosa Pell", "display": "55.10", "SortIntCalc": 1}),
    );
    assert!(row.is_err());
}

#[test]
fn masked_rows_are_told_by_their_id_or_their_x_name_without_a_mark() {
    let row = |v: serde_json::Value| serde_json::from_value::<RankingsRow>(v).unwrap();
    assert!(masked(26).is_masked());
    // A null id is no Profile either.
    assert!(row(serde_json::json!({"AthleteID": null, "SortIntCalc": 1})).is_masked());
    // An X name with no mark, even with an id.
    assert!(
        row(
            serde_json::json!({"AthleteID": 7, "AthleteName": " Xxx  XX ", "display": "",
            "SortIntCalc": 1})
        )
        .is_masked()
    );
    // A person with a mark is never masked, whatever their name; nor is a
    // person whose mark is missing.
    assert!(
        !row(
            serde_json::json!({"AthleteID": 7, "AthleteName": "Xx Xxx", "display": "55.10",
            "SortIntCalc": 1})
        )
        .is_masked()
    );
    assert!(
        !row(
            serde_json::json!({"AthleteID": 7, "AthleteName": "Rosa Pell", "display": "",
            "SortIntCalc": 1})
        )
        .is_masked()
    );
}

#[test]
fn no_recorded_page_is_capped() -> anyhow::Result<()> {
    // The paid session's recordings (anonymized): the capped rule must not
    // fire on real lists, not even on a list recorded only in part (short of
    // its length, but at no known cap). Skipped: the made-up capped list.
    for entry in std::fs::read_dir("tests/fixtures/rankings")? {
        let path = entry?.path();
        if path
            .file_stem()
            .is_some_and(|s| s == "900400-f-400m-all-p1")
        {
            continue;
        }
        let page: RankingsPage = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
        assert!(!page.is_capped_after(0), "{}", path.display());
    }
    Ok(())
}

#[tokio::test]
async fn a_grade_filter_athletic_net_did_not_apply_is_refused() {
    let grades = Grades::parse("9-10").unwrap();
    let q = PageQuery {
        grades: &grades,
        ..synth_page(1)
    };
    let applied = serde_json::json!({"settings": {"fat": true, "grades": [9]}});
    let src = FixtureSource::new().with_rankings_page(&q, page(1..=3, person, applied));
    let err = read(&src, &query(Which::List(SYNTH), "400m", "9-10"))
        .await
        .unwrap_err();
    assert!(
        format!("{err:#}")
            .contains("filtered rankings list 4242 to grade 9 instead of grades 9-10"),
        "{err:#}"
    );

    // The same grades written as strings are the grades asked for.
    let applied = serde_json::json!({"settings": {"grades": ["10", "9"]}});
    let src = FixtureSource::new().with_rankings_page(&q, page(1..=3, person, applied));
    let list = read(&src, &query(Which::List(SYNTH), "400m", "9-10")).await;
    assert_eq!(list.unwrap().count, 3);
}

// ---------------------------------------------------------------------------
// The binary

/// `ath`, its clock pinned to the anonymized fixtures' season.
fn ath(args: &[&str]) -> Output {
    common::ath()
        .args(args)
        .env("ATH_TODAY", TODAY)
        .output()
        .expect("running ath")
}

const CCS_ARGS: [&str; 11] = [
    "list", "--area", "CCS", "--event", "400m", "--gender", "f", "--season", "2099", "--grades",
    "9-10",
];

#[test]
fn ath_list_prints_the_list_as_json() -> anyhow::Result<()> {
    let mut args = CCS_ARGS.to_vec();
    args.extend(["--fixtures-dir", FIXTURES]);
    let out = ath(&args);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rt = tokio::runtime::Runtime::new()?;
    let want = to_json(&rt.block_on(read(&source(), &ccs()))?)?;
    let stdout = String::from_utf8(out.stdout)?;
    assert_eq!(stdout, format!("{want}\n"));
    let json: serde_json::Value = serde_json::from_str(&stdout)?;
    assert_eq!(json["schema"], 1);
    assert_eq!(json["rows"].as_array().map(Vec::len), Some(584));
    assert_eq!(json["rows"][0]["sort_int"], 55390);
    Ok(())
}

#[test]
fn ath_list_table_with_a_limit() -> anyhow::Result<()> {
    let mut args = CCS_ARGS.to_vec();
    args.extend(["--limit", "3", "--table", "--fixtures-dir", FIXTURES]);
    let out = ath(&args);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout)?;
    let mut lines = stdout.lines();
    assert_eq!(
        lines.next(),
        Some(
            "Central Coast (CA Section) 2099 girls 400 Meters, grades 9-10: first 3 (--limit) \
             (FAT, official marks)"
        )
    );
    assert_eq!(
        lines.next(),
        Some("https://www.athletic.net/TrackAndField/rankings/list/996953819/f/400m?grades=9,10")
    );
    assert_eq!(lines.next(), Some(""));
    assert!(lines.next().unwrap().starts_with("Place  Name"), "{stdout}");
    assert_eq!(lines.count(), 3, "{stdout}");
    Ok(())
}

/// `ath list` on the California All HS list, `--limit` rows, in `format`.
fn ca_all_hs(limit: &str, format: &str) -> Output {
    let out = ath(&[
        "list",
        "--list",
        CALIFORNIA,
        "--event",
        "400m",
        "--gender",
        "f",
        "--season",
        "2099",
        "--grades",
        "9-12",
        "--limit",
        limit,
        format,
        "--fixtures-dir",
        FIXTURES,
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// (Athletic.net's `rank`, `place`, `tied`) for each row of `ath list` JSON.
fn places(out: &Output) -> anyhow::Result<Vec<(i64, i64, bool)>> {
    let json: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    Ok(json["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["rank"].as_i64().unwrap(),
                r["place"].as_i64().unwrap(),
                r["tied"].as_bool().unwrap(),
            )
        })
        .collect())
}

/// Athletic.net numbers equal marks one after another (11 and 12 for two
/// 53.97s); `place` and the table share the place (T11).
#[test]
fn equal_marks_share_a_place_in_json_and_table() -> anyhow::Result<()> {
    let rows = places(&ca_all_hs("13", "--json"))?;
    assert_eq!(
        &rows[9..12],
        [(10, 10, false), (11, 11, true), (12, 11, true)]
    );
    let table = String::from_utf8(ca_all_hs("13", "--table").stdout)?;
    let t11: Vec<&str> = table.lines().filter(|l| l.starts_with("T11 ")).collect();
    assert_eq!(t11.len(), 2, "{table}");
    Ok(())
}

/// `--limit 11` ends between the two 53.97s: the last row kept is still T11.
#[test]
fn a_limit_between_ties_keeps_the_tie() -> anyhow::Result<()> {
    let rows = places(&ca_all_hs("11", "--json"))?;
    assert_eq!(rows.len(), 11);
    assert_eq!(rows[10], (11, 11, true));
    Ok(())
}

#[test]
fn ath_list_refuses_an_unknown_event_code() {
    let out = ath(&[
        "list",
        "--list",
        "996953819",
        "--event",
        "SP",
        "--gender",
        "f",
        "--season",
        "2099",
        "--fixtures-dir",
        FIXTURES,
    ]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("does not know the event code \"SP\""),
        "{stderr}"
    );
}

#[test]
fn ath_list_needs_a_list_and_a_gender() {
    let out = ath(&["list", "--event", "400m", "--gender", "f"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--state"), "{stderr}");
    let out = ath(&["list", "--state", "CA", "--event", "400m"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--gender"), "{stderr}");
}

#[test]
fn ath_list_stops_on_a_capped_list_and_prints_no_rows() {
    for format in ["--json", "--table"] {
        let out = ath(&[
            "list",
            "--list",
            "900400",
            "--event",
            "400m",
            "--gender",
            "f",
            "--season",
            "2026",
            format,
            "--fixtures-dir",
            FIXTURES,
        ]);
        assert!(!out.status.success());
        assert!(
            out.stdout.is_empty(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(
                "Athletic.net returned only 5 of at least 696 rows for Capped State 2026 girls \
                 400m (all grades); full lists need a paid Athletic.net subscription"
            ),
            "{stderr}"
        );
    }
}
