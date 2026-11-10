"""Render a meet's index page: every event seedsheet went through for the meet, grouped by gender
and division, each linked to its seed sheet page and CSV or marked "not seeded" with why.

Used by seedsheet.py in meet mode (a config with a meet id); see README.md. The event pages are
build.py's; this page only links to them. Standard library only.
"""
import re

import build  # puts tools/ on sys.path for report
from report import THEME, esc, link, pretty_date, styles

# The index's file name, in the meet's output folder beside its event pages.
INDEX = "index.html"

GENDERS = {"f": "Girls", "m": "Boys"}


def gender_word(gender):
    """`f` -> Girls, `m` -> Boys (anything else as given)."""
    return GENDERS.get(gender, str(gender))


def counts(s, athletes):
    """(how many are on a checked sheet, what they are, how many configured Athletes are on it), by
    the sheet's kind (build.is_relay): a relay's counts its teams and the runners on their legs, any
    other its entrants."""
    if build.is_relay(s):
        on = {leg["athlete_id"] for t in s["teams"] for leg in t["legs"]}
        n, noun = len(s["teams"]), "team"
    else:
        on = {e["athlete_id"] for e in s["entrants"]}
        n, noun = len(s["entrants"]), "athlete"
    return n, noun, len(on & set(athletes))


# How ath's reasons name the source site, and what the page says instead, in order: the site as a
# describing word goes (`a paid Athletic.net subscription`, `the Athletic.net tab`), and the site
# as itself becomes "the results site".
SOURCE_PHRASES = (
    (re.compile(r"\b(a paid|an?|the|no)\s+Athletic\.net\s+(?=[a-z])", re.IGNORECASE), r"\1 "),
    (re.compile(r"^Athletic\.net\b", re.IGNORECASE), "The results site"),
    (re.compile(r"(?<=[.;:]\s)Athletic\.net\b", re.IGNORECASE), "The results site"),
    (re.compile(r"\bAthletic\.net\b", re.IGNORECASE), "the results site"),
)


# How ath speaks of itself in its refusals, and what the page says instead.
ATH_PHRASES = (
    (re.compile(r"\bath seed does not seed relays yet"), "relays are not seeded yet"),
    (re.compile(r"\bath seed seeds only relays of four equal legs"), "only relays of four equal legs are seeded"),
    (re.compile(r"\bath seeds running events only"), "only running events are seeded"),
    (re.compile(r"\bath seeds on outdoor Personal records"), "seeding reads outdoor Personal records"),
    (re.compile(r"\bath does not reconstruct a past seeding"), "a past seeding is not reconstructed"),
    (re.compile(r"\bath seed\b"), "seeding"),
)

# A sentence or clause with advice for whoever runs ath: a command in backticks, or an option.
CLI_ADVICE = re.compile(r"`|(?<![\w-])--[a-z]")


def public(text):
    """ath's reason as the page says it, for the people it is shared with: without its advice for
    whoever runs seedsheet (a command to run, an option to pass, `(see --season)`, `(ADR 0001)`),
    without URLs and the "see" that points at them (an event's entries page is its own link),
    speaking of seeding rather than of ath, and never naming the page's source site."""
    text = re.sub(r";\s*`ath [^`]*`[^;(]*", " ", text)  # `; `ath meet ...` shows its teams and legs`
    text = re.sub(r"\s*\((?:see\s+)?(?:ADR \d+|--[\w-]+)\)", "", text)
    text = re.sub(r",\s*see\s+--[\w-]+", "", text)  # `(42, see --from-list)`
    text = re.sub(r"[,;]?\s*\(\s*see\s+https?://[^\s)]*\s*\)|[,;]?\s*\bsee\s+https?://[^\s)]*[^\s).,;:]", "", text)
    text = re.sub(r"\(\s*https?://[^\s)]*\s*\)|https?://[^\s)]*[^\s).,;:]", " ", text)
    parts = re.split(r"(?<=[.;])\s+", text.strip())
    text = " ".join(p for p in parts if not CLI_ADVICE.search(p))
    for pattern, instead in ATH_PHRASES + SOURCE_PHRASES:
        text = pattern.sub(instead, text)
    text = re.sub(r"\s+([;:,.)])", r"\1", re.sub(r"\s+", " ", text)).strip()
    return re.sub(r"[;,:]$", "", text).strip() or "it could not be seeded"


def span(dates):
    """`June 10, 2026`, or `June 9, 2026 to June 10, 2026` when the days differ."""
    days = sorted(set(dates))
    if not days:
        return ""
    if len(days) == 1:
        return pretty_date(days[0])
    return f"{pretty_date(days[0])} to {pretty_date(days[-1])}"


def where(meet):
    """`Riverton Fieldhouse, Riverton, NY`: the meet's place, as much of it as it has."""
    city = ", ".join(p for p in (meet.get("city"), meet.get("state")) if p)
    return ", ".join(p for p in (meet.get("location"), city) if p)


def row(ev, athletes, mine_col):
    """One event's row. ev: {"row_label", "url" (its entries page, or None), and either "page" (the
    event page's file name, no extension) with "sheet" (its checked `ath seed` report), or
    "reason" (why it is not seeded)}."""
    entries = link("Entry list", ev["url"], "x") if ev.get("url") else ""
    if ev.get("sheet") is None:
        mine = '<td class="mine"></td>' if mine_col else ""
        sub = f'<div class="sub">{entries}</div>' if entries else ""
        return (f'<tr class="ns"><td class="ev">{esc(ev["row_label"])}'
                f'<div class="why"><span class="none">Not seeded</span> {esc(public(ev["reason"]))}</div>{sub}</td>'
                f'<td class="n"></td>{mine}<td class="links">{entries}</td></tr>')
    n, noun, yours = counts(ev["sheet"], athletes)
    page = esc(ev["page"])
    mine = ""
    if mine_col:
        badge = f'<span class="hl">{yours}</span>' if yours else ""
        mine = f'<td class="mine">{badge}</td>'
    files = f'<a class="x" href="{page}.csv" download>CSV</a>' + (f" &middot; {entries}" if entries else "")
    return (f'<tr><td class="ev"><a href="{page}.html">{esc(ev["row_label"])}</a><div class="sub">{files}</div></td>'
            f'<td class="n">{esc(build.plural(n, noun))}</td>{mine}'
            f'<td class="links">{files}</td></tr>')


def groups(events):
    """The events in runs of one gender and division, in their order: [(heading, events)]."""
    out = []
    for ev in events:
        head = f"{gender_word(ev['gender'])} &middot; {esc(ev['division'])}"
        if out and out[-1][0] == head:
            out[-1][1].append(ev)
        else:
            out.append((head, [ev]))
    return out


def html(meet, events, athletes, theme=THEME):
    """The index page, in theme. meet: `ath meet`'s report (name, date, season, location, city,
    state, url). events: in page order, grouped by gender then division (see row)."""
    athletes = set(athletes or [])
    seeded = [ev for ev in events if ev.get("sheet") is not None]
    mine_col = bool(athletes)
    cols = 4 if mine_col else 3
    body = "\n".join(
        f'<tbody>\n<tr class="divider"><th colspan="{cols}" scope="rowgroup">{head}</th></tr>\n'
        + "".join(row(ev, athletes, mine_col) + "\n" for ev in evs) + "</tbody>"
        for head, evs in groups(events))
    mine_head = '<th class="mine" scope="col">Yours</th>' if mine_col else ""
    table = (f'<div class="tbl-wrap"><table>\n<thead><tr><th scope="col">Event</th>'
             f'<th class="n" scope="col">Entries</th>{mine_head}<th class="links" scope="col">Files</th></tr></thead>\n'
             f'{body}\n</table></div>') if events else (
        '<p class="empty">No event of this meet has entries yet, so there is nothing to seed.</p>')
    days = [d for d in (meet.get("date"), meet.get("end_date")) if d]
    facts = [span(days), esc(where(meet))]
    lede = " &middot; ".join(f for f in facts if f)
    lede = f'\n    <p class="lede">{lede}</p>' if lede else ""
    status = ""
    if events:  # with none, the page's one paragraph says so
        as_of = span(ev["sheet"]["as_of"] for ev in seeded)
        status = f"{len(seeded)} of {build.plural(len(events), 'event')} seeded"
        if as_of:
            status += f", on Personal records as of {as_of}"
        status += "."
        left = len(events) - len(seeded)
        if left:
            status += f' {left} {"is" if left == 1 else "are"} not seeded, with why.'
        status = f'\n    <p class="status">{status}</p>'
    pulled = span(ev["pulled"] for ev in seeded)
    kicker = "Seed sheets" + (f" &middot; {esc(meet['season'])}" if meet.get("season") else "")
    meet_link = f'\n    <p class="meet-link">{link("Meet page", meet["url"])}</p>' if meet.get("url") else ""
    notes = ""
    if events:
        notes = ("<p>Each seeded event links to its seed sheet (entrants in seed order on their Personal records) "
                 "and its CSV. Entries is how many athletes (or relay teams) are on its sheet; Entry list is who "
                 "entered.</p>")
        if mine_col:
            notes += "\n    <p>Yours is how many of your Athletes are on the sheet.</p>"
        if pulled:
            notes += f"\n    <p>Data pulled {pulled}.</p>"
        notes = f'\n  <div class="notes">\n    {notes}\n  </div>'
    return TEMPLATE.format(
        title=esc(f"{meet['name']} seed sheets"), **vars(styles(theme)), **build.SHARED_CSS, theme=theme,
        kicker=kicker, name=esc(meet["name"]), lede=lede, status=status, meet_link=meet_link,
        table=table, notes=notes)


def write(meet, events, athletes, out_dir, theme=THEME):
    """Write the index page into out_dir (the meet's folder, beside its event pages). Returns its
    path."""
    out_dir.mkdir(parents=True, exist_ok=True)
    path = out_dir / INDEX
    path.write_text(html(meet, events, athletes, theme))
    return path


TEMPLATE = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<meta name="robots" content="noindex, nofollow, noarchive">
<title>{title}</title>
{head}
<style>
/* Layout: the meet's header, then one table of its events, a divided group per gender and
   division. Always light: the page ignores the viewer's dark-mode setting. */
{theme_css}
{base_css}
.wrap {{ max-width: 760px; }}
{header_css}
.status {{ margin: 0; max-width: 64ch; }}
.meet-link {{ margin: 0; font-size: 14px; }}
.meet-link a {{ color: var(--fg); text-decoration: none; border-bottom: 1px solid var(--line); }}
.meet-link a:hover, .meet-link a:focus-visible {{ color: var(--accent); border-color: var(--accent); outline: none; }}
{table_css}
table tbody:last-child tr:last-child td {{ border-bottom: 0; }}
{divider_css}
.ev {{ font-weight: 600; }}
.ev > a {{ color: inherit; text-decoration: none; border-bottom: 2px solid var(--highlight); }}
.ev > a:hover, .ev > a:focus-visible {{ color: var(--accent); border-color: var(--accent); outline: none; }}
.n, .mine {{ text-align: right; white-space: nowrap; font-family: var(--display); font-weight: 800; font-size: 17px; font-variant-numeric: tabular-nums; }}
.mine {{ width: 5em; }}
thead .n, thead .mine, thead .links {{ font-size: 12px; font-weight: 600; }}
.hl {{ display: inline-block; min-width: 1.4em; padding: 0 .3em; text-align: center; background: var(--highlight); border-radius: 3px; }}
.links {{ width: 9em; white-space: nowrap; color: var(--muted); font-size: 14px; }}
.x {{ color: var(--fg); text-decoration: none; border-bottom: 1px solid var(--line); }}
.x:hover, .x:focus-visible {{ color: var(--accent); border-color: var(--accent); outline: none; }}
tr.ns .ev {{ color: var(--muted); }}
.why {{ margin-top: 2px; font-weight: 400; font-size: 13px; color: var(--muted); }}
.none {{ display: inline-block; margin-right: .3em; font-family: var(--display); font-weight: 700; font-size: 12px; text-transform: uppercase; letter-spacing: .06em; color: var(--muted); border: 1px dashed var(--muted); border-radius: 3px; padding: 0 .4em; }}
{empty_css}
{notes_css}
.sub {{ display: none; }}
@media (max-width: 640px) {{
  td, thead th, .divider th {{ padding: 8px 8px; }}
  .n, .mine {{ font-size: 15px; }}
  .mine {{ width: 3.6em; }}
  .links {{ display: none; }}
  .sub {{ display: block; margin-top: 4px; font-weight: 400; font-size: 13px; color: var(--muted); }}
}}
</style>
</head>
<body>
<div class="wrap">
  <header>
    <p class="kicker">{kicker}</p>
    <h1>{name}</h1>{lede}{status}{meet_link}
  </header>
  {table}{notes}
</div>
</body>
</html>
"""
