//! seedsheet's meet mode tests (`tools/seedsheet/`) read saved `ath meet`
//! and `ath seed --meet` output for the made-up meet 900700 in
//! `tests/fixtures/meets/`. This fails when ath no longer prints them, so the
//! copies are refreshed (`ATH_BLESS=1 cargo test`) and the Python side then
//! checks the pages still read them (`tests/tools.rs`). `ath seed` reads marks
//! as of today, so each saved report keeps its own `as_of` (pinned), and only
//! that field is not compared.

mod common;

use std::path::Path;

const FIXTURES: &str = "tests/fixtures";
const SAVED: &str = "tools/seedsheet/testdata/meet-900700";
/// The `as_of` a saved report is written with when there is none yet.
const PINNED_AS_OF: &str = "2026-11-07";

fn ath(args: &[&str]) -> Vec<u8> {
    let out = common::ath()
        .args(args)
        .args(["--fixtures-dir", FIXTURES, "--json"])
        .env_remove("ATH_COOKIE")
        .output()
        .expect("running ath");
    assert!(
        out.status.success(),
        "ath {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

fn bless() -> bool {
    std::env::var_os("ATH_BLESS").is_some()
}

fn check(path: &Path, got: &[u8]) -> anyhow::Result<()> {
    if bless() {
        std::fs::write(path, got)?;
    }
    assert!(
        std::fs::read(path)? == got,
        "{} is not what ath prints now; refresh it with ATH_BLESS=1 cargo test",
        path.display()
    );
    Ok(())
}

#[test]
fn seedsheets_saved_meet_is_current() -> anyhow::Result<()> {
    check(
        &Path::new(SAVED).join("ath-meet.json"),
        &ath(&["meet", "900700"]),
    )
}

#[test]
fn seedsheets_saved_meet_seed_sheets_are_current() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    for (gender, event) in [("f", "800m"), ("m", "800m")] {
        let name = format!("{gender}-13-{event}");
        let csv = dir.path().join(format!("{name}.csv"));
        let stdout = ath(&[
            "seed",
            "--meet",
            "900700",
            "--gender",
            gender,
            "--div",
            "13",
            "--event",
            event,
            "--out",
            csv.to_str().unwrap(),
        ]);
        let json_path = Path::new(SAVED).join(format!("{name}.json"));
        let saved: Option<serde_json::Value> = std::fs::read(&json_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        let as_of = saved
            .as_ref()
            .and_then(|s| s["as_of"].as_str().map(str::to_string))
            .unwrap_or_else(|| PINNED_AS_OF.to_string());
        let mut got: serde_json::Value = serde_json::from_slice(&stdout)?;
        got["as_of"] = serde_json::Value::String(as_of);
        if bless() {
            std::fs::write(&json_path, serde_json::to_string_pretty(&got)? + "\n")?;
        } else {
            assert!(
                saved.as_ref() == Some(&got),
                "{} is not what `ath seed --meet` prints now (as_of aside); refresh it with \
                 ATH_BLESS=1 cargo test",
                json_path.display()
            );
        }
        check(
            &Path::new(SAVED).join(format!("{name}.csv")),
            &std::fs::read(&csv)?,
        )?;
    }
    Ok(())
}
