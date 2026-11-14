"""Build the published examples into examples/site/: a landing page, the example pages, and the
demo page every example link goes to (ADR 0010).

    python3 examples/build_site.py [--out DIR]

--out builds the site into DIR instead (the tests build into a temporary folder).

Reads only what is committed under examples/ (anonymized data, never a Recording) and needs no
network and no ath binary: each rankmap example is rankmap's own build of its config in example
mode (`"example": true`), from its saved season. See docs/anonymize.md, "The examples".
Python 3.9+, standard library only.
"""
import argparse
import os
import shutil
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
SITE = HERE / "site"
sys.path.insert(0, str(REPO / "tools" / "rankmap"))
import rankmap  # noqa: E402  (puts tools/ on sys.path for report)
from build import ordinal  # noqa: E402  (rankmap's build)
from report import DEMO_PAGE, FOOTER_CSS, THEME, esc, example_footer, styles  # noqa: E402

# Where the project lives, for the pages' links back to it.
REPO_URL = "https://github.com/jimnist/ath"

# The rankmap examples, in landing-page order: (folder under examples/rankmap/, card title, what
# the page shows). Each folder holds rankmap.config.json, its saved season under data/, and its own
# State Meet standards under areas/ (empty for a State without an Area file).
RANKMAPS = [
    ("ca", "California", "The US map and the CIF Sections map, with the Los Angeles City and Oakland Sections drawn "
                         "as their district outlines, and the CIF State Meet places by Section."),
    ("me", "Maine", "The US map and one card per MPA Class, with the athlete's own Class meet: its automatic and "
                    "provisional standards and the seed the Class list suggests."),
    ("or", "Oregon", "A State without an Area file: the US map and the state-by-state list only."),
]

# Each rankmap example's screenshot (docs/images/, shot by screenshots.py, also in the README), copied
# beside the pages.
SHOT = "example-rankmap-{key}.png"

PAGE = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<title>{title}</title>
<meta name="description" content="{description}">
{head}
<style>
{theme_css}
{base_css}
{footer_css}
h1 {{ font-family: var(--display); font-weight: 900; font-size: clamp(32px, 5vw, 48px); line-height: 1; margin: 0; letter-spacing: -.01em; }}
h2 {{ font-family: var(--display); font-weight: 800; font-size: 26px; margin: 8px 0 0; }}
p {{ margin: 0; max-width: 68ch; }}
.lede {{ font-size: 18px; }}
a {{ color: var(--accent); }}
.cards {{ display: grid; grid-template-columns: repeat(auto-fill, minmax(min(100%, 300px), 1fr)); gap: 16px; list-style: none; margin: 0; padding: 0; }}
.cards > li {{ display: grid; }}
.card {{ display: grid; grid-template-rows: auto auto 1fr auto; gap: 8px; background: var(--surface); border: 1px solid var(--line); border-radius: 8px; overflow: hidden; color: var(--fg); text-decoration: none; }}
a.card:hover, a.card:focus-visible {{ border-color: var(--accent); outline: none; box-shadow: 0 0 0 1px var(--accent); }}
.card img {{ display: block; width: 100%; aspect-ratio: 16 / 10; object-fit: cover; object-position: left top; border-bottom: 1px solid var(--line); background: var(--map-bg); }}
.card .soon {{ display: grid; place-items: center; aspect-ratio: 16 / 10; border-bottom: 1px solid var(--line); background: var(--seg); color: var(--muted); font-family: var(--display); font-weight: 800; font-size: 22px; text-transform: uppercase; letter-spacing: .08em; }}
.card h3 {{ font-family: var(--display); font-weight: 800; font-size: 22px; line-height: 1.1; margin: 0; padding: 0 16px; }}
.card p {{ padding: 0 16px; color: var(--muted); font-size: 14px; }}
.who {{ display: grid; gap: 2px; padding: 4px 16px 16px; font-size: 14px; }}
.who b {{ font-family: var(--display); font-weight: 900; font-size: 22px; line-height: 1.1; font-variant-numeric: tabular-nums; }}
.back {{ font-weight: 600; }}
</style>
</head>
<body>
<div class="wrap">
{body}
  {footer}
</div>
</body>
</html>
"""


def page(title, description, body, theme=THEME):
    """A page of the examples site, in theme, ending with the examples' footer."""
    st = styles(theme)
    return PAGE.format(title=esc(title), description=esc(description), head=st.head, theme_css=st.theme_css,
                       base_css=st.base_css, footer_css=FOOTER_CSS, body=body, footer=example_footer())


def build_rankmap(key, site):
    """Build one rankmap example as rankmap does (its file names, in its config's output_dir, a
    folder of SITE, here of site). Returns its card's facts: the page (relative to site) and, from
    the season the page was built from, who, the grade, the season and the mark."""
    config = HERE / "rankmap" / key / "rankmap.config.json"
    cfg = rankmap.load_config(config)
    if not cfg["example"]:
        sys.exit(f"{config}: an example config needs \"example\": true")
    try:
        cfg["output_dir"] = site / cfg["output_dir"].relative_to(SITE)
    except ValueError:
        sys.exit(f"{config}: output_dir must be {SITE} or a folder in it")
    have, stale = rankmap.load_data(cfg)
    if stale or not have:
        sys.exit(f"{config}: saved seasons that do not build: {stale or 'none saved'}")
    html, built = rankmap.render(cfg, have)
    season = built.default_season
    grade = built.seasons[season]["grade"]
    # The page's grade groups come from the config's graduation year (rankmap.grade_in): the two
    # must agree, or the card and the page would disagree.
    if grade != rankmap.grade_in(cfg, int(season)):
        sys.exit(f"{config}: the saved {season} season is a grade {grade} mark, but graduation_year "
                 f"{cfg['graduation_year']} makes {season} grade {rankmap.grade_in(cfg, int(season))}")
    return {"href": html.relative_to(site).as_posix(), "name": built.meta["name"], "event": built.meta["event"],
            "gender": "girls" if built.meta["gender"] == "f" else "boys",
            "grade": f"{ordinal(grade)} grade" if isinstance(grade, int) else None,
            "season": season, "mark": built.seasons[season]["pr"]}


def card(key, title, about, facts):
    who = ", ".join(x for x in (facts["name"], facts["grade"], facts["season"]) if x)
    return (f'  <li><a class="card" href="{esc(facts["href"])}">'
            f'<img src="images/{SHOT.format(key=key)}" alt="The {esc(title)} rankmap example" loading="lazy">'
            f'<h3>{esc(title)}: {facts["gender"]} {esc(facts["event"])}</h3>'
            f'<p>{esc(about)}</p>'
            f'<span class="who"><span>{esc(who)}</span><b>{esc(facts["mark"])}</b></span></a></li>')


def index(cards):
    body = f"""  <header>
    <h1>ath examples</h1>
  </header>
  <p class="lede"><b>ath</b> reads Athletic.net track and field data for high school coaches and families, and builds pages from it.
  These examples are built with it from anonymized data, so you can see what the pages do before you use it.</p>
  <h2>rankmap</h2>
  <p>Where an athlete&rsquo;s season-best mark would rank on every US state&rsquo;s high school list, by grade group, and in each Section or Class of their home State. Pick a season, a grade group or the State map; click a Section or a Class.</p>
  <ul class="cards">
{chr(10).join(cards)}
  </ul>
  <h2>Seed sheet</h2>
  <ul class="cards">
  <li><div class="card"><div class="soon">Coming soon</div><h3>A meet&rsquo;s seed sheets</h3><p>Every entrant of a meet ranked on their Personal record, one page per event, relays included, made for a phone at the track.</p><span class="who"></span></div></li>
  </ul>
  <h2>Using ath</h2>
  <p>ath is a command-line app with Python reports; the <a href="{REPO_URL}#readme">README</a> says how to install it and build these pages from your own data.
  The links on these examples go to <a href="{DEMO_PAGE}">a page about links</a>: on a page built from real data they open Athletic.net.</p>"""
    return page("ath examples", "Example rankmap pages built with ath from anonymized Athletic.net data.", body)


def demo():
    # "Back" goes back to the example the reader came from (a page of this site: same origin, under
    # this page's folder); without script, or from anywhere else, it is a link to the examples.
    body = f"""  <header>
    <h1>Links in the examples</h1>
  </header>
  <p class="lede">You followed a link on an example page, and every link on the examples comes here.</p>
  <p>On a page built from real data, each athlete, team, list and meet links to its page on Athletic.net.
  The examples are built from anonymized data (invented names, schools and meets, shifted seasons and marks), so there is no real page to link to.</p>
  <p>Real use needs a paid Athletic.net subscription: ath reads Athletic.net through your own session, signed in where it reads (the browser tab of its relay, or the session a cookie comes from), and sees exactly what you see.
  rankmap reads whole State lists, which Athletic.net shows only to a paid subscription.</p>
  <p class="back"><a id="back" href="index.html">Back</a> &middot; <a href="index.html">All examples</a> &middot; <a href="{REPO_URL}#readme">The README</a></p>
<script>
document.getElementById("back").addEventListener("click", e => {{
  const here = location.pathname.slice(0, location.pathname.lastIndexOf("/") + 1);
  let ours = false;
  try {{
    const from = new URL(document.referrer);
    ours = from.origin === location.origin && from.pathname.startsWith(here);
  }} catch (err) {{}}
  if (ours && history.length > 1) {{ e.preventDefault(); history.back(); }}
}});
</script>"""
    return page("Links in the examples", "Why the links on the ath examples come to this page.", body)


def main(out=SITE):
    """Build the site into out (examples/site/ by default), replacing what is there."""
    out = Path(out).resolve()
    if out.exists():
        # Only ever replace a site: examples/site, or a folder an earlier build made.
        if out != SITE and any(out.iterdir()) and not (out / DEMO_PAGE).exists():
            sys.exit(f"{out}: not empty and not an earlier examples site; give an empty or new folder")
        shutil.rmtree(out)
    (out / "images").mkdir(parents=True)
    cards = []
    for key, title, about in RANKMAPS:
        facts = build_rankmap(key, out)
        shutil.copy(REPO / "docs" / "images" / SHOT.format(key=key), out / "images")
        cards.append(card(key, title, about, facts))
    (out / "index.html").write_text(index(cards))
    (out / DEMO_PAGE).write_text(demo())
    # The page relative to here when it is under here (examples/site/index.html), else in full.
    shown = os.path.relpath(out / "index.html")
    if shown.startswith(".."):
        shown = (out / "index.html").resolve()
    print(f"Wrote {shown} and {len(cards)} rankmap examples")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--out", type=Path, default=SITE, help="the folder to build into (default: examples/site)")
    main(parser.parse_args().out)
