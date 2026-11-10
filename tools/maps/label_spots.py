"""Work out where each rank label goes on the maps, and how big, for tools/maps/label-spots.json.

    python3 -I tools/maps/label_spots.py

For every state on the US map (tools/us-states-albers.topo.json) and every Area with counties in
an Area file that has a `map` (areas/<ST>.json), and for every label shape (one to four digits,
with or without a tie's t), finds the biggest box with the label's proportions that fits inside
the shape, pulled a little toward its center of area (report.fit_label), and records the SVG
text anchor and font size. Pages read the file instead of searching on every build, which takes
a few minutes. Rerun it when the map files, an Area file's counties, or the fitting change; a
test checks the recorded `inputs` hash. Standard library only.
"""
import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from report import (  # noqa: E402
    AREAS_DIR, LABEL_SHAPES, REPO, SPOTS, area_regions, label_spot, spots_inputs, us_regions,
)


def spots_for(regions):
    out = {}
    for name, (rings, segs) in sorted(regions.items()):
        t = time.monotonic()
        out[name] = {shape: label_spot(rings, segs, shape) for shape in LABEL_SHAPES}
        print(f"  {name}: {time.monotonic() - t:.1f}s", file=sys.stderr)
    return out


def main():
    print("US states", file=sys.stderr)
    data = {"inputs": spots_inputs(), "us": spots_for(us_regions()), "areas": {}}
    for path in sorted(AREAS_DIR.glob("*.json")):
        areas = json.loads(path.read_text())
        if areas.get("map"):
            print(f"{path.stem} {areas['labels']}", file=sys.stderr)
            data["areas"][path.stem] = spots_for(area_regions(areas))
    SPOTS.write_text(json.dumps(data, indent=1, sort_keys=True) + "\n")
    print(f"wrote {SPOTS.relative_to(REPO)}", file=sys.stderr)


if __name__ == "__main__":
    main()
