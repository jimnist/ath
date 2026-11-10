//! Recording (`ath --record-dir`): every Athletic.net response a run reads is
//! also written to a directory, in exactly the layout `--fixtures-dir` reads
//! ([`crate::source::FixtureSource`], [`crate::source::layout`]), so the same
//! command with `--fixtures-dir <DIR>` replays the run offline.
//!
//! A Recording holds real people, so [`Recorder::new`] refuses a directory in
//! a git work tree that git does not ignore ([`check_private`]: `local/` is
//! ignored), and no token is ever written: rankings pages lose their
//! `jwtTFTopReport` value (every token-like key is emptied: [`redact_tokens`]) and meet data is written
//! as the cache keeps it, without `jwtMeet` (ADR 0010, `docs/anonymize.md`).
//! A search carries its query ([`SEARCH_QUERY_FIELD`]), so the anonymizer can
//! rename its file exactly.

use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, bail};

/// Writes responses into a Recording directory.
#[derive(Debug, Clone)]
pub struct Recorder {
    dir: PathBuf,
}

impl Recorder {
    /// A recorder into `dir`, created when missing. Refused when `dir` is in
    /// a git work tree that does not ignore it ([`check_private`]).
    pub fn new(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        check_private(dir)?;
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating record dir {}", dir.display()))?;
        Ok(Self {
            dir: dir.to_path_buf(),
        })
    }

    /// Whether `rel` (a [`crate::source::layout`] path) is in the Recording.
    pub fn exists(&self, rel: &Path) -> bool {
        self.dir.join(rel).is_file()
    }

    /// Write `raw` to `rel` (a [`crate::source::layout`] path) in the
    /// Recording, replacing what is there. The write goes through a temporary
    /// file renamed into place, so a reader never sees half a file.
    ///
    /// Every token in it (any key holding `token` or starting `jwt`, at any
    /// depth) is emptied first ([`redact_tokens`]).
    pub fn write(&self, rel: &Path, raw: &str) -> Result<()> {
        let raw = redact_tokens(raw);
        let raw = raw.as_str();
        // Every part a plain name: nothing written can leave the record dir.
        if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            bail!(
                "refusing to record {}: not a path inside the record dir",
                rel.display()
            );
        }
        let path = self.dir.join(rel);
        let parent = path.parent().unwrap_or(&self.dir);
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating record dir {}", parent.display()))?;
        // Unique per process and per write, so concurrent writes of one file
        // never share a temp file; not `.json`, so a reader skips it.
        static WRITES: AtomicU64 = AtomicU64::new(0);
        let n = WRITES.fetch_add(1, Ordering::Relaxed);
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let tmp = parent.join(format!(".{name}.{}.{n}.tmp", std::process::id()));
        std::fs::write(&tmp, raw).with_context(|| format!("recording {}", path.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("recording {}", path.display()))?;
        Ok(())
    }

    /// [`Self::write`], but only when `rel` is not in the Recording yet: an
    /// exclusive create (a hard link of the finished temp file, which fails
    /// when the name exists), so a [`Self::write`] that comes first or races
    /// it always wins. False when the file was already there.
    pub fn write_new(&self, rel: &Path, raw: &str) -> Result<bool> {
        if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            bail!(
                "refusing to record {}: not a path inside the record dir",
                rel.display()
            );
        }
        let raw = redact_tokens(raw);
        let path = self.dir.join(rel);
        let parent = path.parent().unwrap_or(&self.dir);
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating record dir {}", parent.display()))?;
        static WRITES: AtomicU64 = AtomicU64::new(0);
        let n = WRITES.fetch_add(1, Ordering::Relaxed);
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let tmp = parent.join(format!(".{name}.{}.new{n}.tmp", std::process::id()));
        std::fs::write(&tmp, raw).with_context(|| format!("recording {}", path.display()))?;
        let linked = std::fs::hard_link(&tmp, &path);
        let _ = std::fs::remove_file(&tmp);
        match linked {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(e).with_context(|| format!("recording {}", path.display())),
        }
    }
}

/// Refuse `dir` when it is in a git work tree that does not ignore it: real
/// people must never land in a path git could track. The innermost work tree
/// (a folder above `dir`, or `dir`, holding `.git`) decides, by `git
/// check-ignore`; outside any work tree anything goes. Symlinks are resolved
/// first (through the nearest folder that exists), so a link cannot sneak
/// around it. Without a working `git`, only the work tree's `local/` passes.
/// `tools/anonymize/anonymize.py` applies the same rule to its `--map-out`.
pub fn check_private(dir: &Path) -> Result<()> {
    let abs = resolved(dir)?;
    let Some(root) = abs.ancestors().find(|a| a.join(".git").exists()) else {
        return Ok(());
    };
    let ignored = match std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["check-ignore", "-q", "--"])
        .arg(&abs)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(status) => status.code() == Some(0),
        Err(_) => {
            let rel = abs.strip_prefix(root).unwrap_or(Path::new(""));
            matches!(rel.components().next(), Some(Component::Normal(first)) if first == "local")
                && rel.components().count() > 1
        }
    };
    if !ignored {
        bail!(
            "refusing to record into {}: it is inside the git work tree {}, which does not \
             ignore it. A Recording holds real people's names and marks, so it goes in \
             {}/local/recordings/<scenario>/ (git ignores local/) or outside any git work tree",
            dir.display(),
            root.display(),
            root.display()
        );
    }
    Ok(())
}

/// `dir` made absolute with every symlink in it resolved, part by part, a
/// dangling one too (to where it points), and `.` and `..` taken as the OS
/// takes them (after the link before them).
fn resolved(dir: &Path) -> Result<PathBuf> {
    let joined = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        std::env::current_dir()
            .context("reading the current dir")?
            .join(dir)
    };
    let parts = |p: &Path| -> Vec<PathBuf> {
        p.components()
            .map(|c| PathBuf::from(c.as_os_str()))
            .collect()
    };
    let mut pending: std::collections::VecDeque<PathBuf> = parts(&joined).into();
    let mut out = PathBuf::new();
    let mut hops = 0;
    while let Some(part) = pending.pop_front() {
        if part.as_os_str() == "." {
            continue;
        }
        if part.as_os_str() == ".." {
            out.pop();
            continue;
        }
        let next = out.join(&part);
        match std::fs::symlink_metadata(&next) {
            Ok(meta) if meta.file_type().is_symlink() => {
                hops += 1;
                if hops > 40 {
                    bail!("too many symlinks resolving {}", joined.display());
                }
                let target = std::fs::read_link(&next)
                    .with_context(|| format!("reading the link {}", next.display()))?;
                let target = if target.is_absolute() { target } else { out.join(target) };
                for (i, p) in parts(&target).into_iter().enumerate() {
                    pending.insert(i, p);
                }
                out = PathBuf::new();
            }
            _ => out = next,
        }
    }
    Ok(out)
}

/// The field a recorded search carries its query in, so the anonymizer can
/// rename the file exactly (`search/<query>.json` is named by the query).
pub const SEARCH_QUERY_FIELD: &str = "athQuery";

/// `raw` (a JSON object's text) with `key: value` added as its first field,
/// the rest of the text as it was.
pub fn with_first_field(raw: &str, key: &str, value: &serde_json::Value) -> String {
    let Some(at) = raw.find('{') else {
        return raw.to_string();
    };
    let rest = &raw[at + 1..];
    let sep = if rest.trim_start().starts_with('}') { "" } else { ", " };
    format!(
        "{}{{{}: {value}{sep}{rest}",
        &raw[..at],
        serde_json::Value::String(key.to_string())
    )
}

/// Where the string value of `raw`'s top-level `key` is, as a byte span
/// (its quotes included), when `raw` is a JSON object that has one. Only the
/// object's own keys count, never one in a nested value.
fn top_level_string(raw: &str, key: &str) -> Option<(usize, usize)> {
    let bytes = raw.as_bytes();
    // The end of the string starting at `i` (a quote), past its closing quote.
    let string_end = |i: usize| -> Option<usize> {
        let mut j = i + 1;
        while j < bytes.len() {
            match bytes[j] {
                b'\\' => j += 2,
                b'"' => return Some(j + 1),
                _ => j += 1,
            }
        }
        None
    };
    let skip_ws = |mut i: usize| {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        i
    };
    let mut i = skip_ws(0);
    if bytes.get(i) != Some(&b'{') {
        return None;
    }
    let mut depth = 0usize;
    let mut expect_key = false;
    while i < bytes.len() {
        match bytes[i] {
            b'{' | b'[' => {
                depth += 1;
                expect_key = depth == 1 && bytes[i] == b'{';
                i += 1;
            }
            b'}' | b']' => {
                depth = depth.checked_sub(1)?;
                i += 1;
            }
            b',' => {
                expect_key = depth == 1;
                i += 1;
            }
            b'"' => {
                let end = string_end(i)?;
                if depth == 1 && expect_key {
                    expect_key = false;
                    let name: String = serde_json::from_str(&raw[i..end]).ok()?;
                    let colon = skip_ws(end);
                    if bytes.get(colon) != Some(&b':') {
                        return None;
                    }
                    let value = skip_ws(colon + 1);
                    if name == key {
                        return (bytes.get(value) == Some(&b'"'))
                            .then(|| string_end(value).map(|e| (value, e)))
                            .flatten();
                    }
                    i = value;
                } else {
                    i = end;
                }
            }
            _ => i += 1,
        }
    }
    None
}

/// `raw` (a JSON object's text) with its top-level `key` set to the string
/// `value`: the old string's text replaced in place, or the field added
/// first when the object has no such string. The rest stays as it was.
pub fn set_top_level_string(raw: &str, key: &str, value: &str) -> String {
    let encoded = serde_json::Value::String(value.to_string());
    match top_level_string(raw, key) {
        Some((start, end)) => format!("{}{encoded}{}", &raw[..start], &raw[end..]),
        None => with_first_field(raw, key, &encoded),
    }
}

/// Whether a key names a token: it holds `token` or starts `jwt` (any case).
fn is_token_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("token") || key.starts_with("jwt")
}

/// `raw` (JSON text) with the string value of every token key ([`is_token_key`],
/// at any depth: `jwtTFTopReport`, `jwtMeet`, a TeamNav's `mascotToken2`)
/// emptied, the rest of the text as it was. Text that is not JSON is returned
/// as it is.
pub fn redact_tokens(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let string_end = |i: usize| -> Option<usize> {
        let mut j = i + 1;
        while j < bytes.len() {
            match bytes[j] {
                b'\\' => j += 2,
                b'"' => return Some(j + 1),
                _ => j += 1,
            }
        }
        None
    };
    let skip_ws = |mut i: usize| {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        i
    };
    // A stack of open containers: true for an object.
    let mut stack: Vec<bool> = Vec::new();
    let mut expect_key = false;
    let mut spans = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => {
                stack.push(true);
                expect_key = true;
                i += 1;
            }
            b'[' => {
                stack.push(false);
                expect_key = false;
                i += 1;
            }
            b'}' | b']' => {
                stack.pop();
                expect_key = false;
                i += 1;
            }
            b',' => {
                expect_key = stack.last() == Some(&true);
                i += 1;
            }
            b'"' => {
                let Some(end) = string_end(i) else {
                    return raw.to_string();
                };
                if expect_key && stack.last() == Some(&true) {
                    expect_key = false;
                    let name: String = match serde_json::from_str(&raw[i..end]) {
                        Ok(name) => name,
                        Err(_) => return raw.to_string(),
                    };
                    let colon = skip_ws(end);
                    if bytes.get(colon) != Some(&b':') {
                        return raw.to_string();
                    }
                    let value = skip_ws(colon + 1);
                    if is_token_key(&name)
                        && bytes.get(value) == Some(&b'"')
                        && let Some(value_end) = string_end(value)
                    {
                        if value_end - value > 2 {
                            spans.push((value, value_end));
                        }
                        i = value_end;
                    } else {
                        i = value;
                    }
                } else {
                    i = end;
                }
            }
            _ => i += 1,
        }
    }
    let mut out = String::with_capacity(raw.len());
    let mut last = 0;
    for (start, end) in spans {
        out.push_str(&raw[last..start]);
        out.push_str("\"\"");
        last = end;
    }
    out.push_str(&raw[last..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dir_outside_any_work_tree_is_fine() {
        let tmp = tempfile::tempdir().unwrap();
        check_private(&tmp.path().join("rec")).unwrap();
    }

    /// A git repo in `dir` that ignores `/local/`, as this one does.
    fn repo(dir: &Path) {
        let ok = std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(dir)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        std::fs::write(dir.join(".gitignore"), "/local/\n").unwrap();
    }

    #[test]
    fn a_dir_in_a_work_tree_must_be_one_git_ignores() {
        let tmp = tempfile::tempdir().unwrap();
        repo(tmp.path());
        for bad in ["rec", "tests/fixtures", "src/local/x", "local/../rec"] {
            let err = check_private(&tmp.path().join(bad)).unwrap_err();
            assert!(
                err.to_string().contains("does not ignore it"),
                "{bad}: {err}"
            );
        }
        for good in ["local/recordings/ca-400m", "local/x"] {
            check_private(&tmp.path().join(good)).unwrap();
        }
        // Whatever the work tree's own .gitignore says goes.
        std::fs::write(tmp.path().join(".gitignore"), "/scratch/\n").unwrap();
        check_private(&tmp.path().join("scratch/rec")).unwrap();
        assert!(check_private(&tmp.path().join("local/rec")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_local_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        repo(tmp.path());
        std::fs::create_dir_all(tmp.path().join("local")).unwrap();
        std::fs::create_dir_all(tmp.path().join("tests")).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("tests"), tmp.path().join("local/link"))
            .unwrap();
        assert!(check_private(&tmp.path().join("local/link/rec")).is_err());
        // `..` after a link goes where the OS takes it: here out of local/.
        std::fs::create_dir_all(tmp.path().join("tests/deep")).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("tests/deep"), tmp.path().join("local/deep"))
            .unwrap();
        assert!(check_private(&tmp.path().join("local/deep/../rec")).is_err());
        // A dangling link counts by where it points: into tests/, refused.
        std::os::unix::fs::symlink(
            tmp.path().join("tests/not-yet"),
            tmp.path().join("local/dangling"),
        )
        .unwrap();
        assert!(check_private(&tmp.path().join("local/dangling")).is_err());
        assert!(check_private(&tmp.path().join("local/dangling/rec")).is_err());
        assert!(Recorder::new(tmp.path().join("local/dangling")).is_err());
        assert!(!tmp.path().join("tests/not-yet").exists());
    }

    #[test]
    fn adds_a_first_field() {
        let q = serde_json::Value::String("Jane".into());
        assert_eq!(
            with_first_field("{\"a\": 1}", "q", &q),
            "{\"q\": \"Jane\", \"a\": 1}"
        );
        assert_eq!(with_first_field("{}", "q", &q), "{\"q\": \"Jane\"}");
    }

    #[test]
    fn writes_create_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let rec = Recorder::new(tmp.path().join("rec")).unwrap();
        rec.write(Path::new("meets/1/entries/m-1-800m.json"), "{}")
            .unwrap();
        let got =
            std::fs::read_to_string(tmp.path().join("rec/meets/1/entries/m-1-800m.json")).unwrap();
        assert_eq!(got, "{}");
    }

    #[test]
    fn nothing_is_written_outside_the_record_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let rec = Recorder::new(tmp.path().join("rec")).unwrap();
        for bad in ["../x.json", "a/../../x.json", "/tmp/x.json", "./x.json"] {
            assert!(rec.write(Path::new(bad), "{}").is_err(), "{bad}");
        }
        assert!(!tmp.path().join("x.json").exists());
        // A server's event code with a '/' or '..' is one file name, in place.
        let q = crate::meet::EntriesQuery {
            meet_id: 1,
            gender: crate::area::Gender::M,
            div_id: 13,
            event_short: "../../evil/800m",
        };
        let rel = crate::source::layout::meet_entries(&q);
        rec.write(&rel, "{}").unwrap();
        assert_eq!(rel.components().count(), 4, "{}", rel.display());
        assert!(tmp.path().join("rec").join(&rel).is_file());
    }

    #[test]
    fn redacts_every_token_and_nothing_else() {
        let raw = r#"{"b": 1, "jwtTFTopReport": "aaa.bbb", "z": "aaa.bbb", "team": {"mascotToken2": "m\"x", "Name": "token"}, "list": [{"JWTMeet": "s"}], "tokens": null}"#;
        assert_eq!(
            redact_tokens(raw),
            r#"{"b": 1, "jwtTFTopReport": "", "z": "aaa.bbb", "team": {"mascotToken2": "", "Name": "token"}, "list": [{"JWTMeet": ""}], "tokens": null}"#
        );
        assert_eq!(redact_tokens("not json"), "not json");
        assert_eq!(redact_tokens(r#"{"b": 1}"#), r#"{"b": 1}"#);
    }

    #[test]
    fn write_new_never_replaces() {
        let tmp = tempfile::tempdir().unwrap();
        let rec = Recorder::new(tmp.path().join("rec")).unwrap();
        let rel = Path::new("navinfo/2026.json");
        assert!(rec.write_new(rel, "{\"a\": 1}").unwrap());
        assert!(!rec.write_new(rel, "{\"a\": 2}").unwrap());
        rec.write(rel, "{\"a\": 3}").unwrap();
        assert!(!rec.write_new(rel, "{\"a\": 4}").unwrap());
        let got = std::fs::read_to_string(tmp.path().join("rec").join(rel)).unwrap();
        assert_eq!(got, "{\"a\": 3}");
    }

    #[test]
    fn only_the_top_level_key_is_set() {
        // A nested key of the same name, earlier in the text, stays.
        let raw = r#"{"a": {"t": "inner", "x": [1, {"t": "deep"}]}, "t": "old", "z": "t"}"#;
        assert_eq!(
            set_top_level_string(raw, "t", "new"),
            r#"{"a": {"t": "inner", "x": [1, {"t": "deep"}]}, "t": "new", "z": "t"}"#
        );
        assert_eq!(
            set_top_level_string(r#"{"a": {"t": "inner"}}"#, "t", "x"),
            r#"{"t": "x", "a": {"t": "inner"}}"#
        );
    }
}
