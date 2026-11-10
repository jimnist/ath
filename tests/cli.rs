//! The `ath` binary as a user runs it. `ath seed` (offline, against the
//! recorded fixtures) must produce the same seed sheet and unresolved rows the
//! library pipeline does, global flags work on either side of the subcommand,
//! and the removed bare form points at `ath seed`.

mod common;

use std::path::Path;
use std::process::Output;

use ath::area::Gender;
use ath::contract::{Format, emit};
use ath::meet::{EventSelect, meet_report, meet_resolve};
use ath::output::{seed_csv_string, write_unresolved_csv};
use ath::profile::{ProfileQuery, profile};
use ath::resolve::resolve;
use ath::roster::read_roster;
use ath::seed::{FetchOpts, SeedDate, seed};
use ath::source::FixtureSource;

const FIXTURES: &str = "tests/fixtures";

fn ath(args: &[&str]) -> Output {
    common::ath()
        .args(args)
        .env_remove("ATH_COOKIE")
        .output()
        .expect("running ath")
}

/// The pinned 400m roster plus one name-only row the fixtures can't resolve.
fn write_roster(dir: &Path) -> String {
    let base = std::fs::read_to_string("tests/roster_400.csv").unwrap();
    let path = dir.join("roster.csv");
    std::fs::write(&path, format!("{base}Nobody Known,,,,\n")).unwrap();
    path.to_str().unwrap().to_string()
}

/// What the seed pipeline produces for `roster` through the library seam.
async fn expected(roster: &str, unresolved_out: &Path, date: SeedDate) -> anyhow::Result<String> {
    let source = FixtureSource::from_dir(FIXTURES)?;
    let rows = read_roster(roster)?;
    let resolution = resolve(&source, &rows).await?;
    write_unresolved_csv(unresolved_out, &resolution.unresolved)?;
    let outcome = seed(
        &source,
        &resolution.resolved,
        "400 Meters",
        date,
        &FetchOpts::default(),
    )
    .await?;
    seed_csv_string(&outcome.rows)
}

/// Today, from the same local-date source `ath` uses.
fn today() -> chrono::NaiveDate {
    chrono::Local::now().date_naive()
}

fn run_seed(global_before: &[&str], global_after: &[&str]) -> anyhow::Result<()> {
    run_seed_with("400 Meters", None, global_before, global_after)
}

/// Run `ath seed --event <event> [--meet-date <meet_date>]` and check it
/// matches the library pipeline seeding "400 Meters" for the same meet date.
fn run_seed_with(
    event: &str,
    meet_date: Option<&str>,
    global_before: &[&str],
    global_after: &[&str],
) -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let roster = write_roster(dir.path());
    // The binary writes resolved ids back into the roster, so the library side
    // must start from an untouched copy of the same input.
    let pristine = dir.path().join("pristine.csv");
    std::fs::copy(&roster, &pristine)?;
    let pristine = pristine.to_str().unwrap();
    let out = dir.path().join("seed.csv");
    let out = out.to_str().unwrap();

    let mut args: Vec<&str> = global_before.to_vec();
    args.extend([
        "seed",
        "--roster",
        &roster,
        "--event",
        event,
        "--out",
        out,
        "--fixtures-dir",
        FIXTURES,
    ]);
    if let Some(meet) = meet_date {
        args.extend(["--meet-date", meet]);
    }
    args.extend(global_after);
    let before = today();
    let result = ath(&args);
    let after = today();
    let stderr = String::from_utf8_lossy(&result.stderr);
    if !result.status.success() && before != after && stderr.contains("in the past") {
        // A meet "today" became yesterday mid-run: refusing it is correct.
        return Ok(());
    }
    assert!(result.status.success(), "ath {args:?} failed: {stderr}");
    assert!(stderr.contains("1 row(s) did not resolve"), "{stderr}");

    // `before` and `after` differ only for a run that straddles midnight; the
    // binary read its own today somewhere in between.
    let meet = meet_date.map(|d| d.parse()).transpose()?;
    let want_unresolved = dir.path().join("want.unresolved.csv");
    let rt = tokio::runtime::Runtime::new()?;
    let mut wants = Vec::new();
    for day in [before, after] {
        // A meet that is past for one of the two days can't be what ran.
        if let Ok(date) = SeedDate::new(meet, day) {
            wants.push(rt.block_on(expected(pristine, &want_unresolved, date))?);
        }
    }
    let want = wants
        .first()
        .expect("the binary's own today allowed this meet");

    let got = std::fs::read_to_string(out)?;
    assert!(wants.contains(&got), "{got}\nwant:\n{want}");
    assert_eq!(got.lines().count(), 4, "header + 3 seeded athletes");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("roster.unresolved.csv"))?,
        std::fs::read_to_string(&want_unresolved)?
    );
    Ok(())
}

#[test]
fn seed_takes_the_flat_race_short_form() -> anyhow::Result<()> {
    run_seed_with("400m", None, &[], &[])
}

/// `ath seed` still writes the CSV, and prints the same sheet under the
/// output contract: JSON with a `schema`, entrants in the CSV's order with
/// their Profile links and `SortInt`; a table with `--table`.
#[test]
fn seed_prints_the_sheet_it_writes() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let out = dir.path().join("seed.csv");
    let out = out.to_str().unwrap();
    // A copy: ath writes the unresolved row's candidates beside the roster.
    let roster = dir.path().join("roster.csv");
    std::fs::copy("tests/roster_seedsheet.csv", &roster)?;
    let args = [
        "seed",
        "--roster",
        roster.to_str().unwrap(),
        "--event",
        "400m",
        "--out",
        out,
        "--fixtures-dir",
        FIXTURES,
    ];
    let before = today();
    let result = ath(&[&args[..], &["--json"]].concat());
    let after = today();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&result.stdout)?;
    assert_eq!(json["schema"], 1);
    assert_eq!(json["event"], "400m");
    assert_eq!(json["event_name"], "400 Meters");
    assert_eq!(json["meet_date"], serde_json::Value::Null);
    let as_of = json["as_of"].as_str().unwrap();
    assert!(
        [before, after].iter().any(|d| d.to_string() == as_of),
        "{as_of}"
    );

    let csv = std::fs::read_to_string(out)?;
    let entrants = json["entrants"].as_array().unwrap();
    assert_eq!(entrants.len(), csv.lines().count() - 1);
    for (e, line) in entrants.iter().zip(csv.lines().skip(1)) {
        let cols: Vec<&str> = line.split(',').collect();
        assert_eq!(e["rank"].to_string(), cols[0]);
        assert_eq!(e["athlete_id"].to_string(), cols[2]);
        assert_eq!(e["athlete_url"], cols[3]);
        assert_eq!(e["pr"], cols[4]);
        // The mark and its SortInt are both there or both missing.
        assert_eq!(e["sort_int"].is_null(), cols[4].is_empty(), "{line}");
    }
    let last = entrants.last().unwrap();
    assert!(last["sort_int"].is_null(), "no-mark entrants come last");
    // The roster row that matched no Profile is not on the sheet, and says so.
    assert_eq!(json["unresolved"][0]["name"], "Ivy Ironwood");
    assert_eq!(json["unresolved"][0]["reason"], "no_match");
    assert_eq!(json["unresolved"].as_array().unwrap().len(), 1);

    let table = ath(&[&args[..], &["--table"]].concat());
    let table = String::from_utf8(table.stdout)?;
    assert!(
        table.starts_with("400 Meters seed sheet: Personal records as of "),
        "{table}"
    );
    assert!(table.contains("8 entrants, 5 with a mark"), "{table}");
    assert!(table.contains("Seed  Athlete"), "{table}");
    assert!(table.contains("no mark"), "{table}");
    assert!(
        table.contains("Not on the sheet:\nIvy Ironwood: no Profile matches"),
        "{table}"
    );
    Ok(())
}

/// With nothing resolved, `ath seed` writes no CSV (an existing one is left
/// alone) but still prints the sheet under the output contract: no entrants,
/// and every roster row listed as unresolved with why.
#[test]
fn seed_with_nothing_resolved_still_prints_the_sheet() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let roster = dir.path().join("roster.csv");
    std::fs::write(
        &roster,
        "name,athlete_id,school,city,state\nIvy Ironwood,,,,\nJude Juniper,,,,\n",
    )?;
    let out = dir.path().join("seed.csv");
    let result = ath(&[
        "seed",
        "--roster",
        roster.to_str().unwrap(),
        "--event",
        "400m",
        "--out",
        out.to_str().unwrap(),
        "--fixtures-dir",
        FIXTURES,
        "--json",
    ]);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "{stderr}");
    assert!(stderr.contains("0 of 2 rows resolved"), "{stderr}");
    assert!(!out.exists(), "no seed sheet CSV with nobody to seed");
    let json: serde_json::Value = serde_json::from_slice(&result.stdout)?;
    assert_eq!(json["schema"], 1);
    assert_eq!(json["entrants"], serde_json::json!([]));
    let names: Vec<&str> = json["unresolved"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Ivy Ironwood", "Jude Juniper"]);
    Ok(())
}

/// The sheet is printed last, after every warning, so a reader that closes
/// stdout early (`ath seed ... | head`) still gets them all on stderr.
#[test]
fn seed_warns_before_printing_the_sheet() -> anyhow::Result<()> {
    use std::process::Stdio;
    let dir = tempfile::tempdir()?;
    let roster = dir.path().join("roster.csv");
    std::fs::copy("tests/roster_seedsheet.csv", &roster)?;
    let out = dir.path().join("seed.csv");
    let mut child = common::ath()
        .args([
            "seed",
            "--roster",
            roster.to_str().unwrap(),
            "--event",
            "1600m",
            "--out",
            out.to_str().unwrap(),
            "--fixtures-dir",
            FIXTURES,
            "--json",
        ])
        .env_remove("ATH_COOKIE")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // Close the reading end at once: printing the sheet hits a broken pipe.
    drop(child.stdout.take());
    let result = child.wait_with_output()?;
    let stderr = String::from_utf8_lossy(&result.stderr);
    // Nobody on the roster has run a 1600m: ath's last warning, then the sheet.
    assert!(stderr.contains("ath: wrote 8 rows"), "{stderr}");
    assert!(
        stderr.contains("0 of 8 athletes had a qualifying mark"),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn seed_matches_library_pipeline() -> anyhow::Result<()> {
    run_seed(&[], &[])
}

#[test]
fn seed_accepts_a_future_meet_date() -> anyhow::Result<()> {
    // Far enough out that the run's own today never matters. A meet today
    // (seeded as of the day before) is covered in tests/pipeline.rs, where
    // today is fixed.
    run_seed_with("400 Meters", Some("2999-12-31"), &[], &[])
}

#[test]
fn seed_accepts_a_meet_today() -> anyhow::Result<()> {
    // Seeded as of the day before (tests/pipeline.rs pins what that excludes);
    // here the binary's own today and the --meet-date wiring are what's tested.
    let meet = today().to_string();
    run_seed_with("400 Meters", Some(&meet), &[], &[])
}

#[test]
fn seed_refuses_a_past_meet_and_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("seed.csv");
    let yesterday = (today() - chrono::Days::new(1)).to_string();
    // A missing roster proves the refusal comes first, before any other work.
    let result = ath(&[
        "seed",
        "--roster",
        "/nonexistent/roster.csv",
        "--event",
        "400 Meters",
        "--meet-date",
        &yesterday,
        "--out",
        out.to_str().unwrap(),
        "--fixtures-dir",
        FIXTURES,
    ]);
    assert_eq!(result.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains(&format!("--meet-date {yesterday} is in the past")),
        "{stderr}"
    );
    assert!(stderr.contains("only for upcoming meets"), "{stderr}");
    assert!(!stderr.contains("/nonexistent/roster.csv"), "{stderr}");
    assert!(!out.exists(), "no seed sheet for a past meet");
}

/// `ath meet` prints what the library reports for the made-up meet 900700
/// (`tests/meet.rs` pins its content).
#[test]
fn meet_prints_the_meet_and_its_events_with_entries() -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    let library = |select: Option<EventSelect<'_>>, format| -> anyhow::Result<String> {
        let report = rt.block_on(meet_report(
            &FixtureSource::from_dir(FIXTURES)?,
            900700,
            select.as_ref(),
        ))?;
        let mut out = Vec::new();
        emit(&report, format, &mut out)?;
        Ok(String::from_utf8(out)?)
    };
    let meet = ["meet", "900700", "--fixtures-dir", FIXTURES];
    assert_eq!(stdout_of(&meet), library(None, Format::Json)?);
    let table = stdout_of(&[&meet[..], &["--table"]].concat());
    assert_eq!(table, library(None, Format::Table)?);
    assert!(
        table.starts_with("Meet 900700: Riverton Winter Classic, 2099-01-09"),
        "{table}"
    );

    let relay = [
        &meet[..],
        &["--gender", "m", "--div", "V", "--event", "4x800m"],
    ]
    .concat();
    let select = EventSelect {
        meet_id: 900700,
        gender: Gender::M,
        division: "V",
        event: "4x800m",
    };
    assert_eq!(stdout_of(&relay), library(Some(select), Format::Json)?);
    let table = stdout_of(&[&relay[..], &["--table"]].concat());
    assert!(
        table.contains("Pinecrest Academy B  8:21.50  1    Jude Calloway"),
        "{table}"
    );

    // --event needs --gender and --div.
    let result = ath(&[&meet[..], &["--event", "800m"]].concat());
    assert_eq!(result.status.code(), Some(2));

    // --resolve prints the event found, from the event list alone.
    let resolve = [
        &meet[..],
        &[
            "--gender",
            "m",
            "--div",
            "V",
            "--event",
            "800 Meters",
            "--resolve",
        ],
    ]
    .concat();
    let select = EventSelect {
        meet_id: 900700,
        gender: Gender::M,
        division: "V",
        event: "800 Meters",
    };
    let report = rt.block_on(meet_resolve(
        &FixtureSource::from_dir(FIXTURES)?,
        900700,
        &select,
    ))?;
    let mut want = Vec::new();
    emit(&report, Format::Json, &mut want)?;
    assert_eq!(stdout_of(&resolve), String::from_utf8(want)?);
    let table = stdout_of(&[&resolve[..], &["--table"]].concat());
    assert!(
        table.contains("800 Meters (800m), boys' Varsity (division 13)"),
        "{table}"
    );
    // --resolve needs --event.
    let result = ath(&[&meet[..], &["--resolve"]].concat());
    assert_eq!(result.status.code(), Some(2));
    Ok(())
}

/// The arguments of `ath seed --meet` for the made-up meet's boys' Varsity
/// `event`.
fn seed_meet_args<'a>(event: &'a str, out: &'a str) -> Vec<&'a str> {
    vec![
        "seed",
        "--meet",
        "900700",
        "--gender",
        "m",
        "--div",
        "13",
        "--event",
        event,
        "--out",
        out,
        "--fixtures-dir",
        FIXTURES,
    ]
}

/// Mode A: the meet's accepted, unscratched 800m entrants are seeded on
/// their Personal records, the sheet says which meet and event they came
/// from, and the entries left out are listed with why.
#[test]
fn seed_from_a_meet_seeds_its_accepted_entrants() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let out = dir.path().join("seed.csv");
    let out = out.to_str().unwrap();
    let args = seed_meet_args("800m", out);
    let result = ath(&[&args[..], &["--json"]].concat());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "{stderr}");
    assert!(
        stderr.contains("ath: wrote 4 rows (3 with a 800m mark)"),
        "{stderr}"
    );
    assert!(
        stderr.contains("not seeded: Owen Pike (athlete 900100004): scratched"),
        "{stderr}"
    );
    assert!(
        stderr.contains(
            "not seeded: Jace Fenwick (athlete 900100005): not accepted (AcceptanceStatus 5)"
        ),
        "{stderr}"
    );

    let json: serde_json::Value = serde_json::from_slice(&result.stdout)?;
    assert_eq!(json["schema"], 1);
    assert!(
        json.get("kind").is_none(),
        "an individual sheet has no kind"
    );
    assert_eq!(json["event"], "800m");
    assert_eq!(json["event_name"], "800 Meters");
    assert_eq!(json["meet_date"], "2099-01-09");
    assert_eq!(json["unresolved"], serde_json::json!([]));
    assert_eq!(json["meet"]["id"], 900700);
    assert_eq!(json["meet"]["name"], "Riverton Winter Classic");
    assert_eq!(
        json["meet"]["url"],
        "https://www.athletic.net/TrackAndField/meet/900700"
    );
    assert_eq!(json["meet"]["event"]["division"], "Varsity");
    assert_eq!(
        json["meet"]["event"]["url"],
        "https://www.athletic.net/TrackAndField/meet/900700/entries/m/13/800m"
    );
    let left_out: Vec<(&str, &str)> = json["meet"]["left_out"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| (l["name"].as_str().unwrap(), l["reason"].as_str().unwrap()))
        .collect();
    assert_eq!(
        left_out,
        [("Owen Pike", "scratched"), ("Jace Fenwick", "not_accepted")]
    );
    let entrants: Vec<(&str, &str)> = json["entrants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["athlete"].as_str().unwrap(), e["pr"].as_str().unwrap()))
        .collect();
    assert_eq!(
        entrants,
        [
            ("Luca Brandt", "1:57.95"),
            ("Milo Marsh", "1:59.10"),
            ("Theo Quill", "2:00.45"),
            ("Ravi Stone", ""),
        ]
    );
    let csv = std::fs::read_to_string(out)?;
    assert_eq!(csv.lines().count(), 5, "header + 4 entrants");
    assert!(
        csv.lines()
            .nth(1)
            .unwrap()
            .starts_with("1,Luca Brandt,900100006,"),
        "{csv}"
    );

    let table = stdout_of(&[&args[..], &["--table"]].concat());
    assert!(
        table.contains(
            "for Riverton Winter Classic (meet 900700) on 2099-01-09, boys Varsity\n\
             4 entrants, 3 with a mark"
        ),
        "{table}"
    );
    assert!(
        table.ends_with(
            "Not on the sheet:\nOwen Pike (athlete 900100004): scratched\n\
             Jace Fenwick (athlete 900100005): not accepted (AcceptanceStatus 5)\n"
        ),
        "{table}"
    );
    Ok(())
}

/// `ath seed --meet` refuses what it cannot seed, saying why, and writes
/// nothing.
#[test]
fn seed_from_a_meet_refuses_mixed_relays_field_events_and_past_meets() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("seed.csv");
    let out = out.to_str().unwrap();
    let refused = |args: &[&str], why: &str| {
        let result = ath(args);
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert_eq!(result.status.code(), Some(1), "{stderr}");
        assert!(stderr.contains(why), "{stderr}");
        assert!(!Path::new(out).exists(), "nothing written");
    };
    // A medley relay by its name only: the code says nothing. Refused from
    // the event list, before any entries are read (the fixtures record none
    // for it).
    refused(
        &seed_meet_args("sprintmed2248", out),
        "SMR 1600m (sprintmed2248) is a relay whose legs are not all one flat race (a medley \
         or shuttle relay), and ath seed seeds only relays of four equal legs (such as 4x800m \
         or 4xMile) so far; `ath meet 900700 --gender m --div 13 --event sprintmed2248` shows \
         its teams and legs",
    );
    refused(
        &seed_meet_args("Shot", out),
        "Shot Put (Shot) is a field event; ath seeds running events only",
    );
    // A shuttle relay only run indoors is refused as a relay first, with the
    // pointer to its teams; an indoor-only individual event by the meet's
    // own code.
    refused(
        &seed_meet_args("55shuttleh", out),
        "4x55 Shuttle Hurdles (55shuttleh) is a relay whose legs are not all one flat race (a \
         medley or shuttle relay), and ath seed seeds only relays of four equal legs (such as \
         4x800m or 4xMile) so far; `ath meet 900700 --gender m --div 13 --event 55shuttleh` \
         shows its teams and legs (it is also only run indoors, and ath seeds on outdoor \
         Personal records)",
    );
    // --seed-on is for relays, and never an indoor-only event.
    refused(
        &[&seed_meet_args("800m", out)[..], &["--seed-on", "1600m"]].concat(),
        "--seed-on \"1600m\" is for relays, and 800 Meters (800m) is not one: its entrants \
         are seeded on the event itself",
    );
    refused(
        &[&seed_meet_args("4x800m", out)[..], &["--seed-on", "600m"]].concat(),
        "--seed-on \"600m\" is only run indoors, and ath seeds on outdoor Personal records",
    );
    // --seed-on takes one individual flat race: not a field event, not a relay.
    refused(
        &[
            &seed_meet_args("4x800m", out)[..],
            &["--seed-on", "Shot Put"],
        ]
        .concat(),
        "--seed-on \"Shot Put\" is not a flat running race",
    );
    refused(
        &[&seed_meet_args("4x800m", out)[..], &["--seed-on", "4x400m"]].concat(),
        "--seed-on \"4x400m\" is a relay; it takes one individual flat race",
    );
    // A bare number names no event, in either mode.
    refused(
        &seed_meet_args("400", out),
        "--event \"400\" is a bare number; did you mean 400m?",
    );
    refused(
        &[
            "seed",
            "--roster",
            "tests/roster_400.csv",
            "--event",
            "400",
            "--out",
            out,
            "--fixtures-dir",
            FIXTURES,
        ],
        "--event \"400\" is a bare number; did you mean 400m?",
    );
    // A roster names people, not teams: a relay needs --meet.
    for relay in ["4x800m", "4xMile", "SMR 1600m"] {
        refused(
            &[
                "seed",
                "--roster",
                "tests/roster_400.csv",
                "--event",
                relay,
                "--out",
                out,
                "--fixtures-dir",
                FIXTURES,
            ],
            &format!(
                "--event {relay:?} is a relay, and relays are seeded from a meet's entries: use \
                 --meet"
            ),
        );
    }
    let mut jv = seed_meet_args("55m", out);
    jv[6] = "JV";
    refused(
        &jv,
        "55 Meters (55m) is only run indoors, and ath seeds on outdoor",
    );
    let mut past = seed_meet_args("800m", out);
    past[2] = "900701";
    refused(
        &past,
        "meet 900701 was on 2020-01-11, in the past (today is ",
    );
}

/// A relay of four equal legs is seeded per team on the sum of its runners'
/// Personal records in its Seeding event (the 800m for the 4x800m): the
/// relay CSV, the JSON's `teams` and the table agree, and teams without an
/// aggregate (a runner with no 800m mark, a runner missing) go last.
#[test]
fn seed_from_a_meet_seeds_a_relay_on_its_runners_personal_records() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let out = dir.path().join("relay.csv");
    let out = out.to_str().unwrap();
    let args = seed_meet_args("4x800m", out);
    let result = ath(&[&args[..], &["--json"]].concat());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "{stderr}");
    assert!(
        stderr.contains("ath: wrote 4 teams (2 with an aggregate on the 800 Meters)"),
        "{stderr}"
    );

    let json: serde_json::Value = serde_json::from_slice(&result.stdout)?;
    assert_eq!(json["schema"], 1);
    assert_eq!(json["kind"], "relay");
    assert_eq!(json["event"], "4x800m");
    assert_eq!(json["event_name"], "4x800 Relay");
    assert_eq!(json["seeding_event"], "800m");
    assert_eq!(json["seeding_event_name"], "800 Meters");
    assert_eq!(json["meet"]["event"]["relay"], true);
    assert!(json.get("entrants").is_none(), "a relay sheet has teams");
    let teams: Vec<(u64, String, &str, Option<i64>, usize)> = json["teams"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["rank"].as_u64().unwrap(),
                format!(
                    "{} {}",
                    t["team"].as_str().unwrap(),
                    t["letter"].as_str().unwrap()
                ),
                t["aggregate"].as_str().unwrap(),
                t["sort_int"].as_i64(),
                t["missing_legs"].as_array().unwrap().len(),
            )
        })
        .collect();
    assert_eq!(
        teams,
        [
            (1, "Harbor View High A".into(), "7:59.05", Some(479_050), 0),
            (2, "Pinecrest Academy A".into(), "8:02.55", Some(482_550), 0),
            (3, "Pinecrest Academy B".into(), "", None, 0),
            (4, "Lakemont Prep A".into(), "", None, 1),
        ]
    );
    // Ezra Holt's faster relay split (1:55.00) and indoor 1:56.90 never count.
    let ezra = &json["teams"][1]["legs"][0];
    assert_eq!(
        (
            ezra["athlete"].as_str(),
            ezra["pr"].as_str(),
            ezra["sort_int"].as_i64()
        ),
        (Some("Ezra Holt"), Some("1:58.20"), Some(118_200))
    );
    // Finch Dorsey has only a 1600m and a relay split: no 800m mark.
    let finch = &json["teams"][2]["legs"][3];
    assert_eq!(
        (
            finch["leg"].as_u64(),
            finch["pr"].as_str(),
            finch["sort_int"].as_i64()
        ),
        (Some(4), Some(""), None)
    );

    let csv = std::fs::read_to_string(out)?;
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines.len(), 5, "header + 4 teams");
    assert_eq!(
        lines[0],
        "rank,team,letter,team_url,aggregate,aggregate_seconds,aggregate_estimated,no_aggregate_reason,\
         runner1,runner1_id,runner1_url,runner1_pr,runner1_estimated_from,\
         runner2,runner2_id,runner2_url,runner2_pr,runner2_estimated_from,\
         runner3,runner3_id,runner3_url,runner3_pr,runner3_estimated_from,\
         runner4,runner4_id,runner4_url,runner4_pr,runner4_estimated_from"
    );
    assert!(
        lines[1].starts_with(
            "1,Harbor View High,A,https://www.athletic.net/team/900402/track-and-field-outdoor,\
             7:59.05,479.05,false,,Kai Lowell,900100021,"
        ),
        "{csv}"
    );
    assert!(
        lines[4].starts_with("4,Lakemont Prep,A,https://www.athletic.net/team/900403/track-and-field-outdoor,,,false,Leg 4 has no runner,"),
        "{csv}"
    );
    assert!(
        lines[4].ends_with(",2:05.00,,,,,,"),
        "no fourth runner: {csv}"
    );

    let table = stdout_of(&[&args[..], &["--table"]].concat());
    assert!(
        table.starts_with(
            "4x800 Relay seed sheet: the sum of each team's 800 Meters Personal records as of "
        ),
        "{table}"
    );
    assert!(
        table.contains(
            ", for Riverton Winter Classic (meet 900700) on 2099-01-09, boys Varsity\n\
             4 teams, 2 with an aggregate\n"
        ),
        "{table}"
    );
    assert!(
        table.lines().any(|l| l.trim_start() == "4    (no runner)"),
        "{table}"
    );
    Ok(())
}

/// A 4xMile's runners are seeded on the faster of their 1600m and their mile
/// converted to the 1600m, the mile flagged as an Estimated mark in the
/// JSON, the CSV and the table; `--seed-on` seeds on one event, unconverted.
#[test]
fn a_4xmile_flags_its_estimated_mile_legs() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let out = dir.path().join("mile.csv");
    let out = out.to_str().unwrap();
    let args = [
        "seed",
        "--meet",
        "900702",
        "--gender",
        "f",
        "--div",
        "21",
        "--event",
        "4xmile",
        "--out",
        out,
        "--fixtures-dir",
        FIXTURES,
    ];
    let result = ath(&[&args[..], &["--json"]].concat());
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&result.stdout)?;
    assert_eq!(json["kind"], "relay");
    assert_eq!(json["seeding_event"], "1600m");
    assert_eq!(json["estimate_from"], "1 Mile");
    let cedar = &json["teams"][0];
    assert_eq!(cedar["estimated"], true);
    assert_eq!(cedar["aggregate"], "20:18.56");
    let iris = &cedar["legs"][1];
    assert_eq!(iris["pr"], "5:00.35");
    assert_eq!(iris["estimated"], true);
    assert_eq!(iris["estimated_from"], "1 Mile");
    assert_eq!(iris["estimated_from_pr"], "5:02.10");
    assert_eq!(iris["estimated_from_sort_int"], 302_100);
    assert_eq!(cedar["legs"][2]["estimated"], false);
    assert_eq!(json["teams"][1]["estimated"], false);

    let csv = std::fs::read_to_string(out)?;
    let lines: Vec<&str> = csv.lines().collect();
    assert!(
        lines[0].starts_with("rank,team,letter,team_url,aggregate,aggregate_seconds,aggregate_estimated,no_aggregate_reason,runner1,runner1_id,runner1_url,runner1_pr,runner1_estimated_from,"),
        "{csv}"
    );
    assert!(
        lines[1].contains(",20:18.56,1218.56,true,,Wren Talbot,"),
        "{csv}"
    );
    assert!(
        lines[1].contains(",5:00.35,1 Mile 5:02.10,Juno Halvorsen,"),
        "{csv}"
    );
    assert!(lines[2].contains(",21:30.00,1290.0,false,,"), "{csv}");

    let table = stdout_of(&[&args[..], &["--table"]].concat());
    assert!(table.contains("20:18.56 (estimated legs)"), "{table}");
    assert!(
        table.contains("5:00.35 (estimated from 1 Mile 5:02.10)"),
        "{table}"
    );

    let plain = ath(&[&args[..], &["--seed-on", "1600m", "--json"]].concat());
    let json: serde_json::Value = serde_json::from_slice(&plain.stdout)?;
    assert_eq!(json["estimate_from"], serde_json::Value::Null);
    assert!(
        json["teams"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["estimated"] == false),
        "{json}"
    );
    Ok(())
}

/// `--seed-on` seeds a relay's runners on another event: here the 1600m,
/// which only two runners have, so no team has an aggregate.
#[test]
fn seed_on_overrides_a_relays_seeding_event() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let out = dir.path().join("relay.csv");
    let args = seed_meet_args("4x800m", out.to_str().unwrap());
    let result = ath(&[&args[..], &["--seed-on", "1600 Meters", "--json"]].concat());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "{stderr}");
    let json: serde_json::Value = serde_json::from_slice(&result.stdout)?;
    assert_eq!(json["seeding_event"], "1600 Meters");
    assert_eq!(json["seeding_event_name"], "1600 Meters");
    let marks: Vec<(&str, &str)> = json["teams"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|t| t["legs"].as_array().unwrap())
        .filter(|l| l["pr"] != "")
        .map(|l| (l["athlete"].as_str().unwrap(), l["pr"].as_str().unwrap()))
        .collect();
    assert_eq!(
        marks,
        [("Arlo Penn", "4:41.50"), ("Finch Dorsey", "4:40.00")]
    );
    assert!(
        json["teams"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["aggregate"] == ""),
        "{json}"
    );
    Ok(())
}

/// In Mode A the event name comes from the meet, so a sheet with no marks
/// says that, not "fix --event".
#[test]
fn seed_from_a_meet_with_no_marks_does_not_blame_the_event_flag() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("seed.csv");
    let mut args = seed_meet_args("800m", out.to_str().unwrap());
    args[4] = "f";
    let result = ath(&args);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "{stderr}");
    assert!(
        stderr.contains(
            "0 of 1 athletes had a qualifying mark. The event name \"800 Meters\" came from \
             the meet; the entrants' Profiles may name the event differently, or none of them \
             has an outdoor mark in it."
        ),
        "{stderr}"
    );
    assert!(!stderr.contains("Check that --event"), "{stderr}");
}

#[test]
fn seed_takes_a_roster_or_a_meet_but_not_both() {
    let usage = |args: &[&str], why: &str| {
        let result = ath(args);
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert_eq!(result.status.code(), Some(2), "{stderr}");
        assert!(stderr.contains(why), "{stderr}");
    };
    let meet = seed_meet_args("800m", "seed.csv");
    usage(
        &[&meet[..], &["--roster", "tests/roster_400.csv"]].concat(),
        "'--meet <MEET_ID>' cannot be used with '--roster <FILE>'",
    );
    usage(
        &[&meet[..], &["--meet-date", "2099-01-09"]].concat(),
        "'--meet <MEET_ID>' cannot be used with '--meet-date <YYYY-MM-DD>'",
    );
    usage(
        &["seed", "--event", "800m"],
        "<--roster <FILE>|--meet <MEET_ID>>",
    );
    // Relays are seeded from a meet's entries only (a roster names no teams).
    usage(
        &[
            "seed",
            "--roster",
            "tests/roster_400.csv",
            "--event",
            "4x800m",
            "--seed-on",
            "800m",
        ],
        "--meet <MEET_ID>",
    );
    usage(
        &["seed", "--meet", "900700", "--event", "800m", "--div", "13"],
        "--gender <GENDER>",
    );
    usage(
        &[
            "seed", "--roster", "r.csv", "--event", "800m", "--gender", "m",
        ],
        "--meet <MEET_ID>",
    );
}

#[test]
fn seed_season_flag_was_removed() {
    let result = ath(&[
        "seed",
        "--roster",
        "tests/roster_400.csv",
        "--event",
        "400 Meters",
        "--season",
        "2026",
        "--fixtures-dir",
        FIXTURES,
    ]);
    assert_eq!(result.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("unexpected argument '--season'"),
        "{stderr}"
    );
}

#[test]
fn seed_accepts_global_flags_before_subcommand() -> anyhow::Result<()> {
    run_seed(
        &["--concurrency", "1", "--delay-ms", "0", "--no-cache"],
        &[],
    )
}

#[test]
fn seed_accepts_global_flags_after_subcommand() -> anyhow::Result<()> {
    run_seed(
        &[],
        &["--concurrency", "1", "--delay-ms", "0", "--no-cache"],
    )
}

#[test]
fn global_cookie_flag_applies_to_seed() {
    // Without --fixtures-dir, `seed` needs a cookie; a --cookie-file given
    // before the subcommand must reach it (a missing file proves it was read).
    let result = ath(&[
        "--cookie-file",
        "/nonexistent/ath-cookie.txt",
        "seed",
        "--roster",
        "tests/roster_400.csv",
        "--event",
        "400 Meters",
    ]);
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("reading cookie file /nonexistent/ath-cookie.txt"),
        "{stderr}"
    );
}

#[test]
fn bare_form_exits_with_pointer_to_seed() {
    let result = ath(&[
        "--roster",
        "tests/roster_400.csv",
        "--event",
        "400 Meters",
        "--fixtures-dir",
        FIXTURES,
    ]);
    assert_eq!(result.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("ath seed --roster"), "{stderr}");
}

#[test]
fn bare_ath_shows_help_even_with_ath_cookie_set() {
    // The README tells users to export ATH_COOKIE; an env-sourced flag must
    // not turn plain `ath` into a "requires a subcommand" error.
    let result = common::ath()
        .env("ATH_COOKIE", "cf_clearance=x")
        .output()
        .expect("running ath");
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(all.contains("Commands:") && all.contains("seed"), "{all}");
    assert!(!all.contains("requires a subcommand"), "{all}");
}

#[test]
fn help_lists_subcommands() {
    let result = ath(&["--help"]);
    assert!(result.status.success());
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        ["Commands:", "seed", "profile", "areas", "relay"]
            .iter()
            .all(|w| stdout.contains(w)),
        "{stdout}"
    );
}

/// What the library prints for the synthetic Profile 2001 (see tests/profile.rs).
fn library_profile(event: Option<&str>, as_of: &str, format: Format) -> String {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let p = rt
        .block_on(async {
            let source = FixtureSource::from_dir(FIXTURES)?;
            let query = ProfileQuery {
                event,
                as_of: Some(as_of.parse()?),
                season: None,
                today: chrono::Local::now().date_naive(),
            };
            profile(&source, 2001, &query).await
        })
        .unwrap();
    let mut out = Vec::new();
    emit(&p, format, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

/// `ath profile --event 400` is refused like the other commands' bare
/// numbers, not answered with an empty event.
#[test]
fn profile_refuses_a_bare_number_event() {
    let result = ath(&[
        "profile",
        "2001",
        "--event",
        "400",
        "--fixtures-dir",
        FIXTURES,
    ]);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert_eq!(result.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains("--event \"400\" is a bare number; did you mean 400m?"),
        "{stderr}"
    );
}

/// Relay entries with no leg numbers on any team would seed nothing: `ath
/// seed --meet` stops, says why and writes nothing.
#[test]
fn a_relay_whose_entries_carry_no_leg_numbers_is_refused() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let meet = dir.path().join("meets/900702");
    std::fs::create_dir_all(meet.join("entries"))?;
    let from = Path::new(FIXTURES).join("meets/900702");
    for f in ["meet.json", "events.json"] {
        std::fs::copy(from.join(f), meet.join(f))?;
    }
    let entries = std::fs::read_to_string(from.join("entries/f-21-4xmile.json"))?;
    let unlettered = ["A1", "A2", "A3", "A4"].iter().fold(entries, |s, l| {
        s.replace(&format!("\"Letter\": \"{l}\""), "\"Letter\": \"A\"")
    });
    std::fs::write(meet.join("entries/f-21-4xmile.json"), unlettered)?;
    let out = dir.path().join("relay.csv");
    let result = ath(&[
        "seed",
        "--meet",
        "900702",
        "--gender",
        "f",
        "--div",
        "21",
        "--event",
        "4xmile",
        "--out",
        out.to_str().unwrap(),
        "--fixtures-dir",
        dir.path().to_str().unwrap(),
    ]);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert_eq!(result.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains(
            "meet 900702's 4xmile entries carry no leg numbers (Letter A1 to A4) on any of its 2 \
             teams"
        ),
        "{stderr}"
    );
    assert!(!out.exists(), "nothing written");
    Ok(())
}

fn stdout_of(args: &[&str]) -> String {
    let result = ath(args);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "ath {args:?} failed: {stderr}");
    String::from_utf8(result.stdout).unwrap()
}

const PROFILE: [&str; 8] = [
    "profile",
    "2001",
    "--event",
    "400m",
    "--as-of",
    "2026-04-01",
    "--fixtures-dir",
    FIXTURES,
];

#[test]
fn profile_prints_json_when_stdout_is_not_a_terminal() {
    // The test captures stdout, so it is never a terminal here.
    let stdout = stdout_of(&PROFILE);
    assert_eq!(
        stdout,
        library_profile(Some("400m"), "2026-04-01", Format::Json)
    );
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(v["schema"], 1);
    assert_eq!(
        v["url"],
        "https://www.athletic.net/athlete/2001/track-and-field"
    );
    assert_eq!(v["event"]["season_best"]["mark"], "58.20");
    assert_eq!(v["event"]["personal_record"]["mark"], "56.90");
}

#[test]
fn profile_table_and_json_flags_force_the_format() {
    let table = library_profile(Some("400m"), "2026-04-01", Format::Table);
    // Global flags go before or after the subcommand.
    let before: Vec<&str> = ["--table"].into_iter().chain(PROFILE).collect();
    assert_eq!(stdout_of(&before), table);
    let after: Vec<&str> = PROFILE.into_iter().chain(["--table"]).collect();
    assert_eq!(stdout_of(&after), table);

    let json: Vec<&str> = PROFILE.into_iter().chain(["--json"]).collect();
    assert_eq!(
        stdout_of(&json),
        library_profile(Some("400m"), "2026-04-01", Format::Json)
    );

    // Both given: the later one wins, on either side of the subcommand.
    let both: Vec<&str> = PROFILE.into_iter().chain(["--json", "--table"]).collect();
    assert_eq!(stdout_of(&both), table);
    let both: Vec<&str> = ["--table"]
        .into_iter()
        .chain(PROFILE)
        .chain(["--json"])
        .collect();
    assert_eq!(
        stdout_of(&both),
        library_profile(Some("400m"), "2026-04-01", Format::Json)
    );
}

#[test]
fn profile_as_of_defaults_to_today() {
    let before = chrono::Local::now().date_naive().to_string();
    let stdout = stdout_of(&[
        "profile",
        "2001",
        "--event",
        "400 Meters",
        "--fixtures-dir",
        FIXTURES,
    ]);
    let after = chrono::Local::now().date_naive().to_string();
    let v: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    // `before` and `after` differ only for a run that straddles midnight.
    let as_of = v["event"]["as_of"].as_str().unwrap();
    assert!(as_of == before || as_of == after, "{stdout}");
}

#[test]
fn profile_warns_about_an_event_not_on_the_profile() {
    let result = ath(&[
        "profile",
        "2001",
        "--event",
        "LJ",
        "--fixtures-dir",
        FIXTURES,
    ]);
    assert!(result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("Dana Diaz has no \"LJ\" results") && stderr.contains("\"400 Meters\""),
        "{stderr}"
    );
    let v: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(v["event"]["season_best"].is_null());
}

#[test]
fn profile_without_fixtures_or_a_cookie_uses_the_relay() {
    let result = ath(&[
        "--relay-port",
        "0",
        "--relay-timeout",
        "1",
        "--no-cache",
        "profile",
        "2001",
    ]);
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("no Athletic.net browser tab connected"),
        "{stderr}"
    );
}

/// `ath profile 2001 --event <event>` succeeds with null bests and this warning.
fn assert_warns(event: &str, warning: &str) {
    let result = ath(&[
        "profile",
        "2001",
        "--event",
        event,
        "--fixtures-dir",
        FIXTURES,
    ]);
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(result.status.success(), "{stderr}");
    assert!(stderr.contains(warning), "{stderr}");
    let v: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(v["event"]["personal_record"].is_null());
}

#[test]
fn profile_warns_that_field_events_are_not_supported() {
    assert_warns(
        "Long Jump",
        "\"Long Jump\" is a field event; ath v1 supports running events only",
    );
}

#[test]
fn profile_warns_when_no_result_is_a_qualifying_mark() {
    assert_warns(
        "800m",
        "Dana Diaz has \"800 Meters\" results but no Qualifying mark on or before",
    );
}

#[test]
fn profile_season_overrides_the_as_of_year() {
    let args: Vec<&str> = PROFILE.into_iter().chain(["--season", "2025"]).collect();
    let v: serde_json::Value = serde_json::from_str(&stdout_of(&args)).unwrap();
    assert_eq!(v["event"]["season"], 2025);
    assert_eq!(v["event"]["season_best"]["mark"], "56.90");
}

#[test]
fn relay_install_prints_the_bookmarklet_and_setup_steps() {
    let result = ath(&["relay", "install"]);
    assert!(result.status.success());
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("Open https://www.athletic.net"), "{stdout}");
    assert!(stdout.contains("click Allow"), "{stdout}");
    let bookmarklet = stdout.lines().last().unwrap();
    assert!(bookmarklet.starts_with("javascript:"), "{bookmarklet}");
    assert!(bookmarklet.contains("127.0.0.1%3A8767"), "{bookmarklet}");
}

#[test]
fn relay_install_console_prints_the_script_for_the_chosen_port() {
    let result = ath(&["--relay-port", "9123", "relay", "install", "--console"]);
    assert!(result.status.success());
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(
        stdout.contains("const ATH = \"http://127.0.0.1:9123\";"),
        "{stdout}"
    );
}

#[test]
fn a_bad_roster_fails_at_once_without_waiting_for_a_tab() {
    let started = std::time::Instant::now();
    let result = ath(&[
        "--relay-port",
        "0",
        "seed",
        "--roster",
        "/nonexistent/roster.csv",
        "--event",
        "400 Meters",
    ]);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "waited for the relay timeout"
    );
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("/nonexistent/roster.csv"), "{stderr}");
    assert!(!stderr.contains("browser tab"), "{stderr}");
}

#[test]
fn seed_without_a_cookie_uses_the_relay_and_stops_with_setup_steps() {
    // Port 0 picks a free port, so this never collides with a running ath.
    let started = std::time::Instant::now();
    let result = ath(&[
        "--relay-port",
        "0",
        "--relay-timeout",
        "1",
        "--no-cache",
        "seed",
        "--roster",
        "tests/roster_400.csv",
        "--event",
        "400 Meters",
    ]);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "hung"
    );
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        stderr.contains("fetching through the browser relay"),
        "{stderr}"
    );
    assert!(
        stderr.contains("no Athletic.net browser tab connected"),
        "{stderr}"
    );
    assert!(stderr.contains("relay install"), "{stderr}");
}
