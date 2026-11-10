//! The repo is public (ADR 0009), so no tracked file may point at the earlier private issue
//! tracker, the maintainer's machine or anyone's email: an issue id or branch name of that
//! tracker, a link to it, a home folder path, a claude.ai link or an email address. Nor may it
//! hold an em dash, in any spelling (the house style uses a plain dash). This fails when one comes
//! back, naming the file and line (the real-value guard, `tools/anonymize/real_values.py`, covers
//! real people's names and ids).
//!
//! Pull request and issue citations (`#12`) are fine: they point at this repo's own.
//!
//! This file is scanned too: every pattern here is spelled in pieces, so it never matches itself
//! (a test checks that it passes its own scan).

mod common;

use common::git_in;
use std::path::Path;

/// Joined pieces: how this file spells every pattern, so it passes its own scan.
fn cat(pieces: &[&str]) -> String {
    pieces.concat()
}

/// The private tracker's team key, lowercase: its issue ids and branch names are the key, a dash
/// and digits (`<key>-12`, `<key>/<key>-12-topic`).
fn team_key() -> String {
    cat(&["j", "im"])
}

const TRACKER: &str = "a link to the private issue tracker";
const ISSUE_ID: &str = "a private issue tracker id or branch name";
const HOME: &str = "a home folder path";
const PRIVATE_REPO: &str = "a link to a private repo";
const CLAUDE: &str = "a claude.ai link (artifact, chat, code session, share or project)";
const EM_DASH: &str = "an em dash";
const EMAIL: &str = "an email address";

/// Lowercase substrings that may not appear in a lowercased line, each with why.
fn substrings() -> Vec<(String, &'static str)> {
    let mut all = vec![
        (cat(&["li", "near.app"]), TRACKER),
        (cat(&["~/co", "de"]), HOME),
        (cat(&["c:\\us", "ers\\"]), HOME),
        (cat(&["c:\\\\us", "ers\\\\"]), HOME),
        (cat(&["c:/us", "ers/"]), HOME),
        (cat(&["claude", ".ai/"]), CLAUDE),
        (cat(&["github.com/jimnist/ath", "-archive"]), PRIVATE_REPO),
        (cat(&["github.com/jimnist/ath", "-local"]), PRIVATE_REPO),
    ];
    let dash = char::from_u32(0x2014).expect("em dash").to_string();
    for spelling in [
        dash,
        cat(&["&md", "ash;"]),
        cat(&["&#82", "12;"]),
        cat(&["&#x20", "14;"]),
        cat(&["\\u20", "14"]),
    ] {
        all.push((spelling, EM_DASH));
    }
    all
}

/// Lowercase prefixes of a home folder, followed by a user's folder name and a slash.
fn home_folders() -> Vec<String> {
    vec![cat(&["/us", "ers/"]), cat(&["/ho", "me/"])]
}

/// Addresses and domains that are no one's email: git's SSH user, the commit trailer's no-reply
/// address, and the reserved example domains.
fn email_allowed(address: &str) -> bool {
    let allowed = [
        cat(&["git", "@github.com"]),
        cat(&["noreply", "@anthropic.com"]),
    ];
    let domain = address.rsplit('@').next().unwrap_or("");
    allowed.contains(&address.to_string())
        || ["example.com", "example.org", "example.net"].contains(&domain)
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The byte offsets in `hay` where `needle` starts.
fn starts<'a>(hay: &'a str, needle: &'a str) -> impl Iterator<Item = usize> + 'a {
    hay.match_indices(needle).map(|(at, _)| at)
}

/// Whether `prefix` occurs in `lower` at the start of a word, with a digit right after it.
fn word_start_then_digit(lower: &str, prefix: &str) -> bool {
    starts(lower, prefix).any(|at| {
        let before_ok = lower[..at].chars().next_back().is_none_or(|c| !is_word(c));
        let digit_after = lower[at + prefix.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit());
        before_ok && digit_after
    })
}

/// A home folder prefix followed by a folder name and a slash.
fn has_home_folder(lower: &str) -> bool {
    home_folders().iter().any(|prefix| {
        starts(lower, prefix).any(|at| {
            let rest = &lower[at + prefix.len()..];
            let name = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
                .count();
            name > 0 && rest[name..].starts_with('/')
        })
    })
}

/// Whether `lower` holds an email address that is someone's: a name, an at sign and a domain
/// with a dot and a top-level domain of letters, not one `email_allowed` lets through.
fn has_email(lower: &str) -> bool {
    let local = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '%' | '+' | '-');
    let host = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-');
    lower.match_indices('@').any(|(at, _)| {
        let name_len = lower[..at].chars().rev().take_while(|&c| local(c)).count();
        let domain: String = lower[at + 1..].chars().take_while(|&c| host(c)).collect();
        let domain = domain.trim_end_matches('.');
        let tld = domain.rsplit('.').next().unwrap_or("");
        let looks_like = name_len > 0
            && domain.contains('.')
            && tld.len() >= 2
            && tld.chars().all(|c| c.is_ascii_alphabetic());
        looks_like && !email_allowed(&format!("{}@{domain}", &lower[at - name_len..at]))
    })
}

/// Every place in `text` that names a private thing: (line number, why).
fn findings(text: &str) -> Vec<(usize, &'static str)> {
    let substrings = substrings();
    let key_dash = format!("{}-", team_key());
    let mut found = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let lower = line.to_lowercase();
        let mut hit = |why| found.push((n + 1, why));
        for (pattern, why) in &substrings {
            if lower.contains(pattern.as_str()) {
                hit(*why);
            }
        }
        if word_start_then_digit(&lower, &key_dash) {
            hit(ISSUE_ID);
        }
        if has_home_folder(&lower) {
            hit(HOME);
        }
        if has_email(&lower) {
            hit(EMAIL);
        }
    }
    found
}

/// Extensions of binary files: only their printable text is checked.
const BINARY: [&str; 9] = [
    "png", "jpg", "jpeg", "gif", "ico", "pdf", "woff", "woff2", "ttf",
];

/// The printable ASCII runs of six or more characters in `bytes`, one per line (a PNG's text
/// chunks, a PDF's metadata, strings in any binary).
fn printable_runs(bytes: &[u8]) -> String {
    bytes
        .split(|b| !(b.is_ascii_graphic() || *b == b' '))
        .filter(|run| run.len() >= 6)
        .map(|run| String::from_utf8_lossy(run).into_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The tracked files of the checkout at `root`.
fn tracked(root: &Path) -> Vec<String> {
    let out = git_in(root)
        .args(["ls-files", "-z"])
        .output()
        .expect("running git ls-files");
    assert!(
        out.status.success(),
        "git ls-files failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

/// What of a tracked path is checked, and whether its lines are a binary file's printable runs:
/// a symlink's target (never followed), a text file's contents, a binary file's printable runs,
/// or None for a file deleted but not yet staged.
fn checked_text(root: &Path, path: &str) -> Option<(String, bool)> {
    let full = root.join(path);
    let meta = std::fs::symlink_metadata(&full).ok()?;
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(&full).ok()?;
        return Some((target.to_string_lossy().into_owned(), false));
    }
    let bytes = std::fs::read(&full).ok()?;
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if ext.is_some_and(|e| BINARY.contains(&e.as_str())) || bytes.contains(&0) {
        return Some((printable_runs(&bytes), true));
    }
    Some((String::from_utf8_lossy(&bytes).into_owned(), false))
}

/// Every finding in the tracked files of the checkout at `root`, as `path:line: why`.
fn scan(root: &Path) -> Vec<String> {
    let mut bad = Vec::new();
    for path in tracked(root) {
        let Some((text, binary)) = checked_text(root, &path) else {
            continue;
        };
        for (line, why) in findings(&text) {
            let place = if binary { "printable run " } else { "" };
            bad.push(format!("{path}:{place}{line}: {why}"));
        }
    }
    bad
}

#[test]
fn no_tracked_file_names_a_private_thing() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    // `.git` is a folder in the main checkout and a file in a linked worktree.
    if !root.join(".git").exists() {
        eprintln!("skipped: {} is not a git checkout", root.display());
        return;
    }
    let bad = scan(root);
    assert!(
        bad.is_empty(),
        "tracked files name a private thing; reword the reference (keep the reason, drop the \
         id or link) or move the file to local/:\n{}",
        bad.join("\n")
    );
}

#[test]
fn this_file_passes_its_own_scan() {
    let me = include_str!("public_tree.rs");
    assert_eq!(findings(me), [], "spell the pattern in pieces");
}

/// The reasons `findings` gives for `text`, one line.
fn why(text: &str) -> Vec<&'static str> {
    findings(text).into_iter().map(|(_, why)| why).collect()
}

#[test]
fn tracker_ids_in_any_case_and_branch_form() {
    let key = team_key();
    let upper = key.to_uppercase();
    assert_eq!(why(&format!("see {upper}-12 for why")), [ISSUE_ID]);
    assert_eq!(why(&format!("see {key}-12")), [ISSUE_ID]);
    assert_eq!(why(&format!("branch {key}/{key}-42-scrub")), [ISSUE_ID]);
    assert_eq!(why(&format!("({upper}-7)")), [ISSUE_ID]);
    // Not an id: the key alone, before a non-digit, or inside a longer word.
    assert_eq!(why(&format!("team `{upper}`, {upper}-x")), [] as [&str; 0]);
    assert_eq!(why(&format!("{key}my-12")), [] as [&str; 0]);
    assert_eq!(why(&format!("a{key}-12")), [] as [&str; 0]);
    let link = format!("https://{}{key}nist/project", cat(&["li", "near.app/"]));
    assert_eq!(why(&link), [TRACKER]);
    // Line numbers count from 1.
    assert_eq!(
        findings(&format!("first\nsecond {upper}-3\n")),
        [(2, ISSUE_ID)]
    );
}

#[test]
fn home_paths_in_any_case_and_spelling() {
    for path in [
        cat(&["/Us", "ers/someone/x"]),
        cat(&["/us", "ers/someone/x"]),
        cat(&["/US", "ERS/someone/x"]),
        cat(&["/ho", "me/someone/x"]),
        cat(&["~/co", "de/x"]),
        cat(&["~/Co", "de/x"]),
        cat(&["C:\\Us", "ers\\someone"]),
        cat(&["c:\\us", "ers\\someone"]),
        cat(&["\"C:\\\\Us", "ers\\\\someone\""]),
        cat(&["C:/Us", "ers/someone"]),
    ] {
        assert_eq!(why(&format!("cd {path}")), [HOME], "{path}");
    }
    // A bare folder prefix with no user folder is no home path.
    assert_eq!(why(&cat(&["/ho", "me/"])), [] as [&str; 0]);
    assert_eq!(why(&cat(&["/us", "ers/ alone"])), [] as [&str; 0]);
}

#[test]
fn claude_links_of_every_kind() {
    for kind in ["artifact", "public", "chat", "code", "share", "project"] {
        let link = format!("https://{}/{kind}/x", cat(&["claude", ".ai"]));
        assert_eq!(why(&link), [CLAUDE], "{kind}");
    }
}

#[test]
fn private_repo_links_in_any_case() {
    for link in [
        cat(&["https://github.com/jimnist/ath", "-archive/pull/3"]),
        cat(&["git@github.com:jimnist/ath", "-local.git"]).replace(':', "/"),
        cat(&["https://GitHub.com/JimNist/Ath", "-Local"]),
    ] {
        assert_eq!(why(&link), [PRIVATE_REPO], "{link}");
    }
    // The repo names alone, and the public repo's own links, are fine.
    assert_eq!(why(&cat(&["`jimnist/ath", "-archive`"])), [] as [&str; 0]);
    assert_eq!(
        why("https://github.com/jimnist/ath/issues"),
        [] as [&str; 0]
    );
}

#[test]
fn pull_request_and_issue_citations_are_fine() {
    for cite in [
        "PR #28",
        "pull request #28",
        "jimnist/ath#28",
        "https://github.com/jimnist/ath/pull/18",
        "https://github.com/jimnist/ath/issues/7",
        "Closes #42",
    ] {
        assert_eq!(
            why(&format!("(decided, {cite})")),
            [] as [&str; 0],
            "{cite}"
        );
    }
}

#[test]
fn em_dashes_in_every_spelling() {
    let dash = char::from_u32(0x2014).expect("em dash");
    for spelling in [
        dash.to_string(),
        cat(&["&md", "ash;"]),
        cat(&["&MD", "ASH;"]),
        cat(&["&#82", "12;"]),
        cat(&["&#x20", "14;"]),
        cat(&["&#X20", "14;"]),
        cat(&["\\u20", "14"]),
        cat(&["\\U20", "14"]),
    ] {
        assert_eq!(why(&format!("a {spelling} b")), [EM_DASH], "{spelling}");
    }
    // An en dash and a plain dash are fine.
    assert_eq!(why("a \u{2013} b - c"), [] as [&str; 0]);
}

#[test]
fn email_addresses_but_the_allowed_ones() {
    for address in [
        cat(&["someone", "@mail.com"]),
        cat(&["Some.One+tag", "@Mail.Co.UK"]),
        cat(&["<someone", "@sub.domain.org>"]),
    ] {
        assert_eq!(why(&format!("by {address}")), [EMAIL], "{address}");
    }
    for fine in [
        cat(&["git clone git", "@github.com:you/x.git"]),
        cat(&["no", "reply", "@anthropic.com"]),
        cat(&["someone", "@example.com"]),
        cat(&["a", "@example.org"]),
        "@scope/package and @decorator".to_string(),
        "user@localhost and x@1.2".to_string(),
    ] {
        assert_eq!(why(&fine), [] as [&str; 0], "{fine}");
    }
}

/// A PNG made of its signature and one chunk.
fn png_with_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend((data.len() as u32).to_be_bytes());
    png.extend(kind);
    png.extend(data);
    png.extend([0u8; 4]); // the CRC, which nothing here checks
    png
}

#[test]
fn binary_files_by_their_text_and_symlinks_by_their_target() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let root = dir.path();
    let planted = cat(&["/Us", "ers/someone/notes.md"]);
    let text_chunk = [b"Comment\0".as_slice(), planted.as_bytes()].concat();
    std::fs::write(root.join("image.png"), png_with_chunk(b"tEXt", &text_chunk)).expect("write");
    std::fs::write(
        root.join("blob.bin"),
        [b"x\0\x01".as_slice(), planted.as_bytes()].concat(),
    )
    .expect("write");
    std::fs::write(
        root.join("clean.png"),
        png_with_chunk(b"IDAT", b"\x01\x02ab\x03"),
    )
    .expect("write");
    std::fs::write(root.join("notes.md"), &planted).expect("write");

    let found = |path: &str| {
        let (text, binary) = checked_text(root, path).expect("a file");
        (why(&text), binary)
    };
    assert_eq!(found("image.png"), (vec![HOME], true));
    assert_eq!(found("blob.bin"), (vec![HOME], true));
    assert_eq!(found("clean.png"), (vec![], true));
    assert_eq!(found("notes.md"), (vec![HOME], false));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&planted, root.join("link")).expect("symlink");
        assert_eq!(found("link"), (vec![HOME], false));
    }
    assert_eq!(checked_text(root, "missing.md"), None);
}
