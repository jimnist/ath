"""Cut one State's counties out of us-atlas, for a report's State map.

    python3 -I tools/maps/extract_counties.py <counties-albers-10m.json> <state FIPS> <ST>

Reads us-atlas@3.0.1 `counties-albers-10m.json` (pre-projected Albers USA, the same projection
as tools/us-states-albers.topo.json; download it from
https://cdn.jsdelivr.net/npm/us-atlas@3.0.1/counties-albers-10m.json) and writes
tools/maps/<ST>-counties.json, moved and scaled into the State's own frame, coordinates to one
decimal. It keeps the topology: `arcs` is every boundary piece as points, and each county is
polygons of rings of arc indices (`~i` is arc i reversed, as in TopoJSON), so a map can tell a
border two counties share from the State's outline. California is 06. Standard library only.
"""
import json
import sys
from pathlib import Path

WIDTH = 600  # the State map's width in SVG units; the height follows the State's shape
MARGIN = 6


def main():
    src, fips, state = sys.argv[1], sys.argv[2], sys.argv[3]
    topo = json.loads(Path(src).read_text())
    sx, sy = topo["transform"]["scale"]
    tx, ty = topo["transform"]["translate"]
    geoms = [g for g in topo["objects"]["counties"]["geometries"] if str(g["id"]).zfill(5).startswith(fips)]
    if not geoms:
        sys.exit(f"no counties with FIPS prefix {fips} in {src}")

    used = sorted({i if i >= 0 else ~i for g in geoms for poly in (g["arcs"] if g["type"] == "MultiPolygon"
                   else [g["arcs"]]) for ring in poly for i in ring})
    index = {old: new for new, old in enumerate(used)}

    def decode(arc):
        x = y = 0
        pts = []
        for dx, dy in arc:
            x += dx
            y += dy
            pts.append((x * sx + tx, y * sy + ty))
        return pts

    arcs = [decode(topo["arcs"][i]) for i in used]
    pts = [p for a in arcs for p in a]
    x0, x1 = min(p[0] for p in pts), max(p[0] for p in pts)
    y0, y1 = min(p[1] for p in pts), max(p[1] for p in pts)
    k = (WIDTH - 2 * MARGIN) / (x1 - x0)
    height = round((y1 - y0) * k + 2 * MARGIN)

    def renumber(i):
        return index[i] if i >= 0 else ~index[~i]

    counties = {}
    for g in geoms:
        polys = g["arcs"] if g["type"] == "MultiPolygon" else [g["arcs"]]
        counties[g["properties"]["name"]] = [[[renumber(i) for i in ring] for ring in poly] for poly in polys]
    data = {
        "source": "us-atlas@3.0.1 counties-albers-10m, rescaled (tools/maps/extract_counties.py)",
        "viewBox": f"0 0 {WIDTH} {height}",
        # How us-atlas's Albers USA coordinates map here (x * k + dx, y * k + dy), for overlays
        # projected the same way (tools/maps/extract_districts.py).
        "frame": {"k": k, "dx": MARGIN - x0 * k, "dy": MARGIN - y0 * k},
        "arcs": [[[round((x - x0) * k + MARGIN, 1), round((y - y0) * k + MARGIN, 1)] for x, y in a] for a in arcs],
        "counties": dict(sorted(counties.items())),
    }
    out = Path(__file__).parent / f"{state}-counties.json"
    out.write_text(json.dumps(data, separators=(",", ":")) + "\n")
    print(f"{out}: {len(counties)} counties, {len(arcs)} arcs, viewBox {data['viewBox']}, "
          f"{out.stat().st_size} bytes")


if __name__ == "__main__":
    main()
