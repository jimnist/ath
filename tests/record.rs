//! `ath --record-dir` (ADR 0010): a run through the live data source
//! (here over the fixture transport, which answers Athletic.net requests from
//! `tests/fixtures`) writes every response it reads into a Recording, and the
//! same command with `--fixtures-dir <recording>` prints the same output.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn ath(args: &[&str]) -> Output {
    common::ath()
        .args(args)
        .env_remove("ATH_COOKIE")
        .output()
        .expect("running ath")
}

/// `ath` with `args`, its clock pinned to `today` (the anonymized fixtures'
/// season) when given, which must succeed.
fn ok(args: &[&str], today: Option<&str>) -> Output {
    let mut cmd = common::ath();
    if let Some(day) = today {
        cmd.env("ATH_TODAY", day);
    }
    let out = cmd.args(args).output().expect("running ath");
    assert!(
        out.status.success(),
        "ath {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

/// Every file under `dir`, by path relative to it, with its bytes.
fn files(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let rel = path.strip_prefix(root).unwrap().to_path_buf();
                out.insert(rel, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

/// One scenario: `args` (a command, without output or source flags) read
/// from `fixtures`, and the files the Recording must hold.
struct Scenario {
    fixtures: &'static str,
    args: &'static [&'static str],
    /// The clock (`ATH_TODAY`): the anonymized fixtures' season, or the real
    /// one for the made-up fixtures.
    today: Option<&'static str>,
    /// `--out` for `ath seed`, which writes its sheet there.
    out: bool,
    holds: &'static [&'static str],
}

/// The anonymized fixtures' clock: their season is 2099.
const ANON_TODAY: &str = "2099-10-30";

const SCENARIOS: &[Scenario] = &[
    Scenario {
        fixtures: "tests/fixtures",
        args: &["profile", "982389051", "--event", "400m"],
        today: Some(ANON_TODAY),
        out: false,
        holds: &[
            "982389051.json",
            "teamnav/920353302-2099.json",
            "navinfo/2099.json",
        ],
    },
    Scenario {
        fixtures: "tests/fixtures",
        args: &["areas", "CA", "--season", "2026"],
        today: None,
        out: false,
        holds: &["navinfo/2026.json"],
    },
    Scenario {
        fixtures: "tests/fixtures",
        args: &[
            "list", "--area", "CCS", "--event", "400m", "--gender", "f", "--season", "2099",
            "--grades", "9-10",
        ],
        today: Some(ANON_TODAY),
        out: false,
        holds: &[
            "navinfo/2099.json",
            "rankings/996953819-f-400m-9_10-p1.json",
            "rankings/996953819-f-400m-9_10-p6.json",
        ],
    },
    Scenario {
        fixtures: "tests/fixtures",
        args: &[
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
        ],
        today: Some(ANON_TODAY),
        out: false,
        holds: &[
            "982389051.json",
            "teamnav/920353302-2099.json",
            "navinfo/2099.json",
            "rankings/985316289-f-400m-9-p1.json",
            "rankings/996953819-f-400m-9_10_11_12-p1.json",
        ],
    },
    Scenario {
        fixtures: "tests/fixtures",
        args: &[
            "rank",
            "982389051",
            "--event",
            "400m",
            "--season",
            "2099",
            "--in",
            "CA,TX",
        ],
        today: Some(ANON_TODAY),
        out: false,
        holds: &["rankings/921344494-f-400m-9_10_11_12-p4.json"],
    },
    Scenario {
        fixtures: "tests/fixtures",
        args: &["meet", "900700"],
        today: None,
        out: false,
        holds: &["meets/900700/meet.json", "meets/900700/events.json"],
    },
    Scenario {
        fixtures: "tests/fixtures",
        args: &[
            "seed", "--meet", "900700", "--gender", "m", "--div", "13", "--event", "800m",
        ],
        today: None,
        out: true,
        holds: &["meets/900700/entries/m-13-800m.json", "900100001.json"],
    },
    Scenario {
        fixtures: "tests/fixtures",
        args: &[
            "seed", "--meet", "900700", "--gender", "m", "--div", "13", "--event", "4x800m",
        ],
        today: None,
        out: true,
        holds: &["meets/900700/entries/m-13-4x800m.json", "900100011.json"],
    },
    Scenario {
        fixtures: "tests/fixtures",
        args: &[
            "seed", "--meet", "900702", "--gender", "f", "--div", "21", "--event", "4xmile",
        ],
        today: None,
        out: true,
        holds: &["meets/900702/entries/f-21-4xmile.json", "900100051.json"],
    },
];

/// Run `s` with `source` flags, returning its JSON output and, for a seed,
/// the sheet it wrote.
fn run(s: &Scenario, dir: &Path, name: &str, source: &[&str]) -> (Vec<u8>, Vec<u8>) {
    let csv = dir.join(format!("{name}.csv"));
    let mut args: Vec<&str> = s.args.to_vec();
    if s.out {
        args.extend(["--out", csv.to_str().unwrap()]);
    }
    args.push("--json");
    args.extend(source);
    let out = ok(&args, s.today);
    let sheet = if s.out { std::fs::read(&csv).unwrap() } else { Vec::new() };
    (out.stdout, sheet)
}

#[test]
fn a_recorded_run_replays_to_the_same_output() {
    for s in SCENARIOS {
        let tmp = tempfile::tempdir().unwrap();
        let rec = tmp.path().join("rec");
        let rec_s = rec.to_str().unwrap();
        let live = run(
            s,
            tmp.path(),
            "live",
            &[
                "--no-cache",
                "--fixture-transport",
                s.fixtures,
                "--record-dir",
                rec_s,
            ],
        );
        let replay = run(s, tmp.path(), "replay", &["--fixtures-dir", rec_s]);
        assert!(
            live == replay,
            "{:?}: the replay printed something else:\n{}\nvs\n{}",
            s.args,
            String::from_utf8_lossy(&live.0),
            String::from_utf8_lossy(&replay.0)
        );
        // The live path over the fixtures says what the fixture source says.
        let direct = run(s, tmp.path(), "direct", &["--fixtures-dir", s.fixtures]);
        assert!(live == direct, "{:?}: live and fixture runs differ", s.args);

        let recorded = files(&rec);
        for want in s.holds {
            assert!(
                recorded.contains_key(Path::new(want)),
                "{:?}: the recording lacks {want}: {:?}",
                s.args,
                recorded.keys().collect::<Vec<_>>()
            );
        }
        for (path, bytes) in &recorded {
            let text = String::from_utf8_lossy(bytes);
            assert!(
                !text.contains("fixture-token") && !text.contains("fixture-meet-token"),
                "{:?}: {} holds a token",
                s.args,
                path.display()
            );
        }
    }
}

#[test]
fn a_run_the_cache_answers_records_the_same() {
    for s in SCENARIOS {
        let tmp = tempfile::tempdir().unwrap();
        let cache = tmp.path().join("cache");
        let (first, second, uncached) = (
            tmp.path().join("first"),
            tmp.path().join("second"),
            tmp.path().join("uncached"),
        );
        for (rec, cache_flags) in [
            (&first, vec!["--cache-dir", cache.to_str().unwrap()]),
            (&second, vec!["--cache-dir", cache.to_str().unwrap()]),
            (&uncached, vec!["--no-cache"]),
        ] {
            let mut source = cache_flags;
            source.extend([
                "--fixture-transport",
                s.fixtures,
                "--record-dir",
                rec.to_str().unwrap(),
            ]);
            run(s, tmp.path(), "out", &source);
        }
        let first = files(&first);
        assert!(!first.is_empty());
        assert!(
            first == files(&second),
            "{:?}: a cached re-run recorded something else",
            s.args
        );
        assert!(
            first == files(&uncached),
            "{:?}: an uncached run recorded something else",
            s.args
        );
    }
}

#[test]
fn a_record_dir_in_a_work_tree_outside_local_is_refused() {
    // In a git repo of its own, so a guard that cannot decide never writes into this one.
    let repo = tempfile::tempdir().unwrap();
    let init = Command::new("git")
        .args(["init", "-q"])
        .arg(repo.path())
        .status()
        .unwrap();
    assert!(init.success());
    std::fs::write(repo.path().join(".gitignore"), "/local/\n").unwrap();
    let fixtures = std::env::current_dir().unwrap().join("tests/fixtures");
    let dir = repo.path().join("tests/should-not-exist");
    let out = ath(&[
        "meet",
        "900700",
        "--json",
        "--no-cache",
        "--fixture-transport",
        fixtures.to_str().unwrap(),
        "--record-dir",
        dir.to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("local/recordings"), "{stderr}");
    assert!(!dir.exists());
    let ok = repo.path().join("local/recordings/meet");
    let out = ath(&[
        "meet",
        "900700",
        "--json",
        "--no-cache",
        "--fixture-transport",
        fixtures.to_str().unwrap(),
        "--record-dir",
        ok.to_str().unwrap(),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(ok.join("meets/900700/meet.json").is_file());
}

/// An `ATH_TODAY` (or `ATH_COOKIE`) the user exported never reaches the tests'
/// ath runs: the shared helper takes them away. A malformed one would fail
/// this replay.
#[test]
fn the_test_helper_drops_the_users_session_settings() {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ath"));
    cmd.env("ATH_TODAY", "not a date").env("ATH_COOKIE", "x=y");
    common::scrubbed(&mut cmd);
    let out = cmd
        .args([
            "meet",
            "900700",
            "--json",
            "--fixtures-dir",
            "tests/fixtures",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn record_dir_with_fixtures_dir_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let out = ath(&[
        "meet",
        "900700",
        "--json",
        "--fixtures-dir",
        "tests/fixtures",
        "--record-dir",
        tmp.path().join("rec").to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--record-dir"), "{stderr}");
}

#[test]
fn the_fixture_transport_never_touches_the_users_cache() {
    let home = tempfile::tempdir().unwrap();
    let out = common::ath()
        .args([
            "meet",
            "900700",
            "--json",
            "--fixture-transport",
            "tests/fixtures",
        ])
        .env_remove("ATH_COOKIE")
        .env("HOME", home.path())
        .env("XDG_CACHE_HOME", home.path().join("xdg"))
        .output()
        .expect("running ath");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        files(home.path()).is_empty(),
        "wrote {:?}",
        files(home.path()).keys().collect::<Vec<_>>()
    );
}

/// `ATH_TODAY` pins the clock (replaying anonymized data, whose seasons end at
/// 2099), and a malformed one is an error.
#[test]
fn ath_today_pins_the_clock() {
    let tmp = tempfile::tempdir().unwrap();
    let csv = tmp.path().join("s.csv");
    let run = |today: &str| {
        common::ath()
            .args(["seed", "--meet", "900700", "--gender", "m", "--div", "13"])
            .args(["--event", "800m", "--out", csv.to_str().unwrap()])
            .args(["--json", "--fixtures-dir", "tests/fixtures"])
            .env_remove("ATH_COOKIE")
            .env("ATH_TODAY", today)
            .output()
            .expect("running ath")
    };
    let out = run("2098-12-31");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["as_of"], "2098-12-31");
    let bad = run("not a date");
    assert!(!bad.status.success());
    assert!(String::from_utf8_lossy(&bad.stderr).contains("ATH_TODAY"));
}

/// A run that reads Athletic.net never takes a pinned clock: it refuses one,
/// before reading anything (no relay is waited for).
#[test]
fn a_live_run_refuses_ath_today() {
    let started = std::time::Instant::now();
    let out = common::ath()
        .args(["meet", "900700", "--json", "--no-cache"])
        .env_remove("ATH_COOKIE")
        .env("ATH_TODAY", "2099-01-08")
        .output()
        .expect("running ath");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("only for replays"), "{stderr}");
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}
