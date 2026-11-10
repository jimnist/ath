//! The Python tools under `tools/` (the reports, ADR 0007, and the anonymizer,
//! ADR 0010) read `ath`'s JSON or its fixtures, so their
//! tests run with `cargo test`: a contract change that breaks a report fails
//! here, not on the next live run. So do the published examples' (`examples/`,
//! ADR 0010): an Athletic.net link in their pages fails here.

use std::path::PathBuf;
use std::process::Command;

/// Each report's folder under `tools/` with tests (`test_*.py`) in it, then
/// `examples/`.
fn report_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir("tools")
        .expect("reading tools/")
        .map(|e| e.expect("reading tools/").path())
        .filter(|p| {
            p.is_dir()
                && std::fs::read_dir(p)
                    .expect("reading a tools/ folder")
                    .any(|f| {
                        let name = f.expect("reading a tools/ folder").file_name();
                        let name = name.to_string_lossy();
                        name.starts_with("test_") && name.ends_with(".py")
                    })
        })
        .collect();
    dirs.sort();
    dirs.push(PathBuf::from("examples"));
    dirs
}

#[test]
fn python_report_tests_pass() {
    let dirs = report_dirs();
    assert!(!dirs.is_empty(), "no report tests found under tools/");
    let python = std::env::var("PYTHON").unwrap_or_else(|_| "python3".to_string());
    for dir in dirs {
        // ATH_TEST_BIN: the tests that run the real ath (on the fixtures) run this
        // build of it.
        let out = Command::new(&python)
            .args(["-m", "unittest", "discover", "-s"])
            .arg(&dir)
            .env("ATH_TEST_BIN", env!("CARGO_BIN_EXE_ath"))
            // The user's session settings never reach the tests' ath runs.
            .env_remove("ATH_COOKIE")
            .env_remove("ATH_TODAY")
            .output()
            .unwrap_or_else(|e| {
                panic!(
                    "cannot run {python} for the tools/ tests ({e}); set PYTHON to a Python 3.9+"
                )
            });
        assert!(
            out.status.success(),
            "{} Python tests failed:\n{}{}",
            dir.display(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
