"""Work out where each rank label goes on the maps, and how big, for tools/maps/label-spots.json.

    python3 -I tools/maps/label_spots.py

For every state on the US map (tools/us-states-albers.topo.json) and every Area with counties in
an Area file that has a `map` (areas/<ST>.json), and for every label shape (one to four digits,
with or without a tie's t), finds the biggest box with the label's proportions in the house
theme's display font (report.THEME's metrics) that fits inside the shape, pulled a little toward
its center of area (report.fit_label), and records the SVG text anchor and font size. Pages read
the file instead of searching on every build. Rerun it when the map files, an Area file's
counties, the house display font's metrics or the fitting change; a test checks the recorded
`inputs` hash. A theme file with its own display font gets its spots computed at build time
instead (report.label_spots). Standard library only.
"""
import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from report import REPO, SPOTS, THEME, compute_spots  # noqa: E402


def main():
    start = time.monotonic()
    data = compute_spots(THEME, progress=lambda line: print(f"{line} ({time.monotonic() - start:.1f}s)",
                                                            file=sys.stderr))
    SPOTS.write_text(json.dumps(data, indent=1, sort_keys=True) + "\n")
    print(f"wrote {SPOTS.relative_to(REPO)}", file=sys.stderr)


if __name__ == "__main__":
    main()
