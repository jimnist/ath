#!/usr/bin/env python3
"""The real-value guard (ADR 0010, docs/anonymize.md): no real id or name of the Recordings in a
tracked file.

    python3 tools/anonymize/real_values.py scan
    python3 tools/anonymize/real_values.py refresh local/recordings/*.map.json [--extra FILE]...
    python3 tools/anonymize/real_values.py key | gh secret set ATH_GUARD_KEY

The guard's list (real-values.txt, beside this script) holds no real value: for each real id and
name, the first 16 hex digits of HMAC-SHA256 under a secret guard key, over its kind and the
value. Without the key nothing in the list can be checked against a guess. The key is derived
from the anonymizer's key (HMAC(anonymize.key, "real-values-guard")), read from `ATH_GUARD_KEY`
(hex) or, without it, derived from the main checkout's `local/anonymize.key`; `key` prints it, for
a pipe into `gh secret set` only. Without either, `scan` cannot run and the test skips.

`scan` hashes every id-like number and every run of words in the tracked files the same way and
names the place of each match (never the value: the file has it). test_real_values.py runs it.

`refresh` adds the real values of the anonymizer's maps (`--map-out`) and any `--extra` file
(`{"ids": [...], "people": [...], "names": [...]}`), all private (in `local/`), to the list,
keeping what it has and its `allow` lines; `--rebuild` makes it again from only the files given
(give every map and extra file then).

What is guarded:

- ids of six or more digits (athletes, results, meets, lists, most schools and teams), written
  plain or in groups of three (`1_234_567`), standing alone: not inside a word (`#17793f`) and not
  as the value of a JSON key the anonymizer knows holds no id (a mark's `SortInt`, a count);
- the whole names of people (two or more words, first-last and last-first) and of schools,
  meets and venues (three or more: two, such as `Valley Christian`, are as often plain words),
  matched by their words folded (accents and apostrophes dropped, any case, any punctuation:
  `Jane O'Doe`, `jane-odoe`, `Doe, Jane`), and in JSON a `FirstName` and `LastName` together;
  but for those that are public names too: a place or division name of the real public sources
  (`areas/*.json`, the recorded division trees in `local/recordings/`) is not guarded.
  A first or last name alone names nobody, nor does a city.

A match that is no real value there (a mark equal to an id, a public meet in the docs) is allowed
by path, with its reason: an `allow` line.

Python 3.9+, standard library only.
"""

import argparse
import hashlib
import hmac
import json
import os
import re
import subprocess
import sys
import unicodedata
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent))

import report  # noqa: E402  (tools/report.py: local_dir, git_env)
from anonymize import PUBLIC_INT_KEYS, SORT_KEYS, area_files, public_divisions  # noqa: E402

LIST = HERE / "real-values.txt"
ROOT = HERE.parents[1]
KEY_ENV = "ATH_GUARD_KEY"
KEY_LABEL = b"real-values-guard"
# What the list's `check` line is the keyed hash of: a key that does not give it is the wrong one.
CHECK_LABEL = b"ath real-values check v1"
DERIVE = "python3 tools/anonymize/real_values.py key | gh secret set %s (from the main checkout)" % "ATH_GUARD_KEY"
NAME_KINDS = {"person", "school", "meet", "venue"}
MAX_WORDS = 8
MIN_ID_DIGITS = 6
DIGITS = re.compile(r"(?<![0-9A-Za-z_])(?:[0-9]{%d,}|[0-9]{1,3}(?:_[0-9]{3})+)(?![0-9A-Za-z_])" % MIN_ID_DIGITS)
# A JSON number under a key that holds no id (marks, counts, event ids, seasons).
NOT_AN_ID = re.compile(r'"(%s)"\s*:\s*$' % "|".join(sorted(PUBLIC_INT_KEYS | SORT_KEYS)))
WORD = re.compile(r"[a-z0-9]+")
APOSTROPHES = re.compile(r"['’ʼ`]")
FIRST_KEYS, LAST_KEYS = ("FirstName", "firstName"), ("LastName", "lastName")


class NoKey(Exception):
    pass


def derive(anonymize_key):
    """The guard key from the anonymizer's key (its 64 hex digits)."""
    return hmac.new(bytes.fromhex(anonymize_key.strip()), KEY_LABEL, hashlib.sha256).digest()


def guard_key():
    """The guard key: `ATH_GUARD_KEY` (hex), else derived from `anonymize.key` in your local/
    folder (`ATH_LOCAL`, else the main checkout's `local/`)."""
    env = os.environ.get(KEY_ENV, "").strip()
    if env:
        return bytes.fromhex(env)
    path = report.local_dir() / "anonymize.key"
    if path.is_file():
        return derive(path.read_text())
    raise NoKey("no guard key: set %s, or keep local/anonymize.key in the main checkout" % KEY_ENV)


def fold(text):
    """Text as names are compared: accents dropped (NFKD), apostrophes dropped, lower case."""
    text = unicodedata.normalize("NFKD", text)
    text = "".join(c for c in text if not unicodedata.combining(c))
    return APOSTROPHES.sub("", text).lower()


def words(text):
    """A name as the scan reads it: its folded words."""
    return WORD.findall(fold(text))


def windows(text):
    """Every run of 2 to MAX_WORDS words in `text`, as `words` spells a name."""
    ws = words(text)
    return {" ".join(ws[i:i + n]) for i in range(len(ws)) for n in range(2, MAX_WORDS + 1) if i + n <= len(ws)}


def check_of(key):
    return hmac.new(key, CHECK_LABEL, hashlib.sha256).hexdigest()[:16]


class Guard:
    def __init__(self, key, ids=(), names=(), firsts=(), allow=(), check=None):
        self.key = key
        self.check = check
        self.ids, self.names, self.firsts = set(ids), set(names), set(firsts)
        self.allow = list(allow)  # (kind, hash, [paths], reason)
        self._cache = {}

    @classmethod
    def load(cls, path=LIST, key=None):
        ids, names, firsts, allow, check = [], [], [], [], None
        for line in Path(path).read_text().splitlines():
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            bits = line.split()
            if bits[0] == "check":
                check = bits[1]
            elif bits[0] == "id":
                ids.append(bits[1])
            elif bits[0] == "name":
                names.append(bits[1])
            elif bits[0] == "first":
                firsts.append(bits[1])
            elif bits[0] == "allow":
                head, _, reason = line.partition("#")
                allow.append((bits[1], bits[2], head.split()[3:], reason.strip()))
            else:
                raise ValueError("%s: an unknown line: %s" % (path, bits[0]))
        return cls(key if key is not None else guard_key(), ids, names, firsts, allow, check)

    def verifies(self):
        """Whether this key is the list's: its `check` line is the key's hash of CHECK_LABEL."""
        return self.check is not None and hmac.compare_digest(self.check, check_of(self.key))

    def h(self, kind, value):
        k = (kind, value)
        if k not in self._cache:
            msg = ("%s\x1f%s" % (kind, value)).encode()
            self._cache[k] = hmac.new(self.key, msg, hashlib.sha256).hexdigest()[:16]
        return self._cache[k]

    def allowed(self, kind, hashed, path):
        return any(k == kind and h == hashed and path in paths for k, h, paths, _ in self.allow)

    def name_at(self, ws):
        """Whether a guarded name starts the words `ws`."""
        if not ws or self.h("first", ws[0]) not in self.firsts:
            return None
        for n in range(2, min(MAX_WORDS, len(ws)) + 1):
            hashed = self.h("name", " ".join(ws[:n]))
            if hashed in self.names:
                return hashed
        return None

    def scan_text(self, text, path=""):
        """(line, column, kind) of every real id or name in `text` (read from `path`)."""
        hits = []
        json_file = path.endswith(".json")
        for m in DIGITS.finditer(text):
            if json_file and NOT_AN_ID.search(text, max(0, m.start() - 40), m.start()):
                continue
            hashed = self.h("id", m.group(0).replace("_", ""))
            if hashed in self.ids and not self.allowed("id", hashed, path):
                hits.append((m.start(), "a real id"))
        # Words with their places in the text (folding keeps them: accents and apostrophes
        # only ever drop characters, never move the words).
        tokens = []
        for m in re.finditer(r"[^\W_]+(?:['’ʼ`][^\W_]+)*", text):
            for w in words(m.group(0)):
                tokens.append((m.start(), w))
        for i, (start, _) in enumerate(tokens):
            hashed = self.name_at([w for _, w in tokens[i:i + MAX_WORDS]])
            if hashed and not self.allowed("name", hashed, path):
                hits.append((start, "a real name"))
        if json_file:
            hits += self.scan_pairs(text, path)
            hits += self.scan_escaped(text, path)
        out = []
        for offset, kind in sorted(set(hits)):
            line = text.count("\n", 0, offset) + 1
            column = offset - (text.rfind("\n", 0, offset) + 1) + 1
            out.append((line, column, kind))
        return out

    def scan_escaped(self, text, path):
        """A JSON string with escapes (`\\u00eb`, `\\u0041`), read decoded: an escape cannot
        hide a name or an id."""
        hits = []
        for m in re.finditer(r'"((?:[^"\\\n]|\\.)*)"', text):
            if "\\" not in m.group(1):
                continue
            try:
                value = json.loads(m.group(0))
            except ValueError:
                continue
            for d in DIGITS.finditer(value):
                hashed = self.h("id", d.group(0).replace("_", ""))
                if hashed in self.ids and not self.allowed("id", hashed, path):
                    hits.append((m.start(), "a real id"))
            ws = words(value)
            for i in range(len(ws)):
                hashed = self.name_at(ws[i:i + MAX_WORDS])
                if hashed and not self.allowed("name", hashed, path):
                    hits.append((m.start(), "a real name"))
        return hits

    def scan_pairs(self, text, path):
        """A JSON object's `FirstName` and `LastName` together (each alone names nobody)."""
        try:
            doc = json.loads(text)
        except ValueError:
            return []
        hits = []

        def walk(v):
            if isinstance(v, dict):
                first = next((v[k] for k in FIRST_KEYS if isinstance(v.get(k), str)), None)
                last = next((v[k] for k in LAST_KEYS if isinstance(v.get(k), str)), None)
                if first and last:
                    hashed = self.name_at(words(first + " " + last))
                    if hashed and not self.allowed("name", hashed, path):
                        at = text.find(json.dumps(last, ensure_ascii=False))
                        hits.append((max(at, 0), "a real name"))
                for x in v.values():
                    walk(x)
            elif isinstance(v, list):
                for x in v:
                    walk(x)

        walk(doc)
        return hits


def tracked(root):
    """The tracked files of the git work tree at `root` (relative paths)."""
    done = subprocess.run(["git", "ls-files", "-z"], cwd=str(root), env=report.git_env(), capture_output=True,
                          check=True)
    return [p for p in done.stdout.decode().split("\0") if p]


def scan(root, guard, skip=LIST):
    """(path, line, column, kind) of every real id or name in the tracked text files."""
    hits = []
    for rel in tracked(root):
        path = Path(root) / rel
        if not path.is_file() or path.resolve() == Path(skip).resolve():
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (UnicodeDecodeError, OSError):
            continue  # not text
        hits += [(rel,) + hit for hit in guard.scan_text(text, rel)]
    return hits


def public_names(areas=None, trees=None):
    """Names the real public sources have (never a file the guard checks but the Area files):
    every run of words in the Area files, and the public division names (`anonymize.
    public_divisions`: States, Areas, levels) of the Area files and of the real division trees
    recorded in your local/ folder (`recordings/*/navinfo/`)."""
    files = area_files(areas or ROOT / "areas")
    out = set()
    for _, text, _ in files:
        out |= windows(text)
    pattern = trees or str(report.local_dir() / "recordings" / "*" / "navinfo" / "*.json")
    loaded = [json.loads(p.read_text(encoding="utf-8")) for p in sorted(Path("/").glob(pattern.lstrip("/")))]
    _, names = public_divisions(files, loaded)
    out |= {" ".join(words(n)) for n in names}
    return out


def real_values(map_files, extra_files, public=frozenset()):
    """The ids and names to guard, from the anonymizer's maps and extra files: ids as digits,
    names as their folded words, a person's name in both orders."""
    ids, people, others = set(), set(), set()
    for path in map_files:
        m = json.loads(Path(path).read_text())
        ids.update(k for k in m.get("ids", {}))
        for value, kinds in m.get("value_kinds", {}).items():
            if "person" in kinds:
                people.add(value)
            elif set(kinds) & NAME_KINDS:
                others.add(value)
    for path in extra_files:
        x = json.loads(Path(path).read_text())
        ids.update(str(i) for i in x.get("ids", []))
        people.update(x.get("people", []))
        others.update(x.get("names", []))
    names = set()
    for person in people:
        ws = words(person)
        if len(ws) >= 2:
            names.add(" ".join(ws))
            names.add(" ".join(ws[-1:] + ws[:-1]))  # Last, First
    # A school's, meet's or venue's of three or more words (two, such as `Valley Christian` or
    # `Time Trial`, are as often plain words as a name).
    names |= {n for n in (" ".join(words(v)) for v in others) if len(n.split()) >= 3}
    return ({i for i in ids if len(i) >= MIN_ID_DIGITS},
            {n for n in names if len(n.split()) <= MAX_WORDS and n not in public})


HEADER = """\
# The real-value guard's list (tools/anonymize/real_values.py, docs/anonymize.md): no real value,
# only the first 16 hex digits of HMAC-SHA256(guard key, kind, value) of each real id (`id`),
# whole name (`name`) and a name's first word (`first`, to find where a name may start).
# The guard key is secret (ATH_GUARD_KEY, derived from local/anonymize.key); `check` is its
# keyed hash of a fixed label, telling a wrong key from the right one.
# Add: python3 tools/anonymize/real_values.py refresh local/recordings/*.map.json [--extra FILE]
# `allow <kind> <hash> <path>... # reason`: a match there is no real value (kept by refresh)."""


def write(path, guard):
    lines = [HEADER, "check %s" % check_of(guard.key)]
    lines += ["allow %s %s %s  # %s" % (kind, hashed, " ".join(paths), reason)
              for kind, hashed, paths, reason in guard.allow]
    # Sorted by hash, so the list says nothing of the values' order.
    lines += ["id %s" % h for h in sorted(guard.ids)]
    lines += ["first %s" % h for h in sorted(guard.firsts)]
    lines += ["name %s" % h for h in sorted(guard.names)]
    Path(path).write_text("\n".join(lines) + "\n")


def main(argv=None):
    parser = argparse.ArgumentParser(description="The real-value guard (docs/anonymize.md).")
    sub = parser.add_subparsers(dest="command", required=True)
    s = sub.add_parser("scan", help="scan the tracked files of this work tree for real values")
    s.add_argument("--list", default=str(LIST))
    sub.add_parser("key", help="print the guard key, for a pipe into `gh secret set %s` only" % KEY_ENV)
    r = sub.add_parser("refresh", help="add the real values of the anonymizer's maps to the list")
    r.add_argument("maps", nargs="+", help="--map-out files (private)")
    r.add_argument("--extra", action="append", default=[],
                   help='more real values: {"ids": [...], "people": [...], "names": [...]}')
    r.add_argument("--allow", action="append", default=[], metavar="KIND:VALUE:PATH[,PATH...]:REASON",
                   help="a value allowed in those files (kept with the list's other allow lines)")
    r.add_argument("--rebuild", action="store_true",
                   help="make the list from only the files given (give every map and extra file), not add to it")
    r.add_argument("--areas", help="the Area files (default: the repo's areas/)")
    r.add_argument("--trees", help="the real division trees, a glob (default: local/recordings/*/navinfo/*.json)")
    r.add_argument("--list", default=str(LIST))
    args = parser.parse_args(argv)
    try:
        key = guard_key()
    except NoKey as e:
        print("real_values: %s" % e, file=sys.stderr)
        return 2
    if args.command == "key":
        if sys.stdout.isatty():
            print("real_values: the key goes into a pipe (| gh secret set %s), never a terminal" % KEY_ENV,
                  file=sys.stderr)
            return 2
        sys.stdout.write(key.hex())
        return 0
    if args.command == "scan":
        guard = Guard.load(args.list, key)
        if not guard.verifies():
            print("real_values: the guard key is not the list's (a wrong %s or anonymize.key); derive it with: %s"
                  % (KEY_ENV, DERIVE), file=sys.stderr)
            return 2
        hits = scan(ROOT, guard, args.list)
        for path, line, column, kind in hits:
            print("%s:%d:%d: %s" % (path, line, column, kind))
        print("%d real values in the tracked files" % len(hits))
        return 1 if hits else 0
    old = Guard.load(args.list, key) if Path(args.list).exists() else Guard(key, check=check_of(key))
    if old.check is not None and not old.verifies():  # (a list from before the check line takes it)
        print("real_values: the guard key is not the list's; adding to it would mix keys (derive it with: %s)"
              % DERIVE, file=sys.stderr)
        return 2
    guard = Guard(key, allow=old.allow) if args.rebuild else old
    for spec in args.allow:
        kind, value, paths, reason = spec.split(":", 3)
        value = value.replace("_", "") if kind == "id" else " ".join(words(value))
        guard.allow.append((kind, guard.h(kind, value), paths.split(","), reason))
    ids, names = real_values(args.maps, args.extra, public_names(args.areas, args.trees))
    before = (len(guard.ids), len(guard.names))
    guard.ids |= {guard.h("id", i) for i in ids}
    guard.names |= {guard.h("name", n) for n in names}
    guard.firsts |= {guard.h("first", n.split()[0]) for n in names}
    write(args.list, guard)
    print("%d ids and %d names in %s (%+d, %+d)" % (len(guard.ids), len(guard.names), args.list,
                                                   len(guard.ids) - before[0], len(guard.names) - before[1]))
    return 0


if __name__ == "__main__":
    sys.exit(main())
