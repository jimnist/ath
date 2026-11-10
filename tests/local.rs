//! Personal configs, collected data, report output, Recordings and handoff notes live in
//! `local/`, which `.gitignore` ignores along with the older places they used to sit (see the
//! README). This fails when git tracks any file `.gitignore` ignores, so the guard is exactly
//! the ignore rules: nothing ignored can be committed by `git add -f` or from before a rule.

mod common;

use common::git_in;
use std::collections::BTreeSet;
use std::path::Path;

/// Tracked files in the checkout at `dir` that its `.gitignore` files ignore. Only the repo's
/// own `.gitignore` rules count, not `.git/info/exclude` or a global excludes file, so the
/// answer is the same on every machine. Panics when git fails.
fn tracked_but_ignored(dir: &Path) -> BTreeSet<String> {
    let out = git_in(dir)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--ignored",
            "--exclude-per-directory=.gitignore",
        ])
        .output()
        .unwrap_or_else(|e| panic!("cannot run git in {} ({e})", dir.display()));
    assert!(
        out.status.success(),
        "git ls-files failed in {}:\n{}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

fn git(dir: &Path, args: &[&str]) {
    let out = git_in(dir)
        .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
        .args(args)
        .output()
        .expect("running git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn nothing_ignored_is_tracked() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    // `.git` is a folder in the main checkout and a file in a linked worktree.
    if !root.join(".git").exists() {
        eprintln!("skipped: {} is not a git checkout", root.display());
        return;
    }
    let bad = tracked_but_ignored(root);
    assert!(
        bad.is_empty(),
        "tracked files that .gitignore ignores; personal ones belong in local/ \
         (git rm --cached them and move them there):\n{}",
        bad.into_iter().collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn the_ignore_rules_tell_personal_files_from_the_tools_own()
-> Result<(), Box<dyn std::error::Error>> {
    let personal = [
        "local/rankmap/rankmap.config.json",
        "local/recordings/x.json",
        "local/handoffs/2026-11-06.md",
        ".claude/handoffs/2026-11-06.md",
        "tools/rankmap/rankmap.config.json",
        "tools/rankmap/rankmap.123-400m.config.json",
        "tools/seedsheet/seedsheet.config.json",
        "tools/rankmap/data/123-400m/2026.json",
        "tools/rankmap/out/123-400m.html",
        "tools/seedsheet/data/meet-girls-400m.json",
        "tools/seedsheet/out/meet-girls-400m.html",
        "tools/seedsheet/girls-400m.csv",
        "tools/arearank/arearank.config.json",
    ];
    let shipped = [
        "tools/rankmap/rankmap.config.example.json",
        "tools/seedsheet/seedsheet.config.example.json",
        "tools/rankmap/rankmap.py",
        "tools/rankmap/testdata/ath-rank-2026.json",
        "tools/seedsheet/testdata/ath-seed.csv",
        "tools/seedsheet/testdata/x.config.json",
        "tools/maps/CA-counties.json",
        "tools/maps/places.csv",
        "tools/report.py",
        "tests/roster_seedsheet.csv",
        "docs/rankmap.md",
        ".claude/skills/ath/SKILL.md",
        ".claude/skills/rankmap/SKILL.md",
        "localize.md",
        "docs/local/notes.md",
    ];
    let dir = tempfile::tempdir()?;
    git(dir.path(), &["init", "-q"]);
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join(".gitignore"),
        dir.path().join(".gitignore"),
    )?;
    for p in personal.iter().chain(&shipped) {
        let path = dir.path().join(p);
        std::fs::create_dir_all(path.parent().expect("a file in a folder"))?;
        std::fs::write(path, "x")?;
    }
    git(dir.path(), &["add", "-f", "--", "."]);
    let want: BTreeSet<String> = personal.iter().map(|p| p.to_string()).collect();
    assert_eq!(tracked_but_ignored(dir.path()), want);
    Ok(())
}

/// Matt Pocock's skills that `docs/agents/skills.md` says how to install; `.gitignore` names
/// the same ones.
const INSTALLED_SKILLS: [&str; 7] = [
    "domain-modeling",
    "grill-with-docs",
    "grilling",
    "implement",
    "setup-matt-pocock-skills",
    "to-spec",
    "to-tickets",
];

/// Installed third-party skills are never committed: `.gitignore` ignores `.agents/`, the
/// `skills-lock.json` the CLI keeps, and each skill's link in `.claude/skills/` by name, while a
/// skill of the repo's own in `.claude/skills/` is tracked by default. Installed skills are laid
/// out as `npx skills` lays them out: files under `.agents/skills/<name>/`, and
/// `.claude/skills/<name>` a symlink to `../../.agents/skills/<name>`.
#[cfg(unix)]
#[test]
fn installed_skills_are_ignored_and_the_repos_own_are_not() -> Result<(), Box<dyn std::error::Error>>
{
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = tempfile::tempdir()?;
    git(dir.path(), &["init", "-q"]);
    std::fs::copy(root.join(".gitignore"), dir.path().join(".gitignore"))?;
    let write = |p: &str| -> std::io::Result<()> {
        let path = dir.path().join(p);
        std::fs::create_dir_all(path.parent().expect("a file in a folder"))?;
        std::fs::write(path, "x")
    };
    std::fs::create_dir_all(dir.path().join(".claude/skills"))?;
    write("skills-lock.json")?;
    let mut installed = BTreeSet::from(["skills-lock.json".to_string()]);
    for name in INSTALLED_SKILLS {
        let file = format!(".agents/skills/{name}/SKILL.md");
        write(&file)?;
        installed.insert(file);
        let link = format!(".claude/skills/{name}");
        std::os::unix::fs::symlink(
            format!("../../.agents/skills/{name}"),
            dir.path().join(&link),
        )?;
        installed.insert(link);
    }
    for own in [
        ".claude/skills/ath/SKILL.md",
        ".claude/skills/rankmap/SKILL.md",
        ".claude/skills/a-new-skill-of-our-own/SKILL.md",
    ] {
        write(own)?;
    }
    git(dir.path(), &["add", "-f", "--", "."]);
    let want = installed;
    assert_eq!(
        tracked_but_ignored(dir.path()),
        want,
        "every installed skill needs its own line in .gitignore, and nothing else in \
         .claude/skills/ may be ignored"
    );
    Ok(())
}
