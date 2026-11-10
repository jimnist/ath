//! Shared by the integration tests: running the `ath` binary, and git on this checkout.

use std::path::Path;
use std::process::Command;

/// The `ath` binary, with none of the user's session settings: no
/// `ATH_COOKIE` (a live transport) and no `ATH_TODAY` (a pinned clock). A test
/// that means one sets it on the command.
#[allow(dead_code)]
pub fn ath() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ath"));
    scrubbed(&mut cmd);
    cmd
}

/// `cmd` without the user's session settings ([`ath`]).
#[allow(dead_code)]
pub fn scrubbed(cmd: &mut Command) -> &mut Command {
    cmd.env_remove("ATH_COOKIE").env_remove("ATH_TODAY")
}

/// Environment variables that point git somewhere other than the folder it runs in (a git hook
/// sets some of them, and `cargo test` may run from one).
const GIT_REDIRECTS: [&str; 8] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
];

/// git run in `dir` and nowhere else.
#[allow(dead_code)]
pub fn git_in(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir);
    for var in GIT_REDIRECTS {
        cmd.env_remove(var);
    }
    cmd
}
