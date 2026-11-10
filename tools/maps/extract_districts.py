"""Cut school district outlines out of a Census shapefile, for Areas that are one district's
schools (California's Los Angeles City and Oakland Sections), onto a State's county map.

    python3 -I tools/maps/extract_districts.py <shapefile without extension> <ST> "<district name>=<Area name>" ...

Reads a Census cartographic boundary file of unified school districts (for California
`cb_2024_06_unsd_500k`, from https://www2.census.gov/geo/tiger/GENZ2024/shp/cb_2024_06_unsd_500k.zip),
projects each named district the way us-atlas projects its maps (d3's Albers USA: scale 1300,
translate 487.5, 305), moves it into tools/maps/<ST>-counties.json's frame, and writes
tools/maps/<ST>-districts.json: {"districts": {Area name: [polygon: [ring: [[x, y], ...]]]}}.
Standard library only.
"""
import json
import math
import struct
import sys
from pathlib import Path

HERE = Path(__file__).parent


def read_dbf(path):
    """The records of a dBase file, as dicts of stripped strings."""
    data = path.read_bytes()
    count, header_len, record_len = struct.unpack("<IHH", data[4:12])
    fields, pos = [], 32
    while data[pos] != 0x0D:
        name = data[pos:pos + 11].split(b"\0")[0].decode("ascii")
        fields.append((name, data[pos + 16]))
        pos += 32
    records = []
    for i in range(count):
        rec = data[header_len + i * record_len: header_len + (i + 1) * record_len]
        out, off = {}, 1  # byte 0 is the deletion flag
        for name, length in fields:
            out[name] = rec[off:off + length].decode("latin-1").strip()
            off += length
        records.append(out)
    return records


def read_shp(path):
    """Each record's rings, as [(lon, lat), ...] (polygon shapes only)."""
    data = path.read_bytes()
    shapes, pos = [], 100
    while pos < len(data):
        _, length = struct.unpack(">ii", data[pos:pos + 8])
        content = data[pos + 8: pos + 8 + length * 2]
        pos += 8 + length * 2
        (kind,) = struct.unpack("<i", content[:4])
        if kind != 5:
            shapes.append([])
            continue
        parts_n, points_n = struct.unpack("<ii", content[36:44])
        parts = list(struct.unpack(f"<{parts_n}i", content[44:44 + 4 * parts_n]))
        at = 44 + 4 * parts_n
        pts = list(struct.iter_unpack("<dd", content[at:at + 16 * points_n]))
        shapes.append([pts[s:e] for s, e in zip(parts, parts[1:] + [points_n])])
    return shapes


def albers_usa(lon, lat):
    """d3.geoAlbersUsa().scale(1300).translate([487.5, 305]) for the lower 48: conic equal-area,
    parallels 29.5 and 45.5, rotated 96 degrees, centered on (-0.6, 38.7)."""
    phi0, phi1 = math.radians(29.5), math.radians(45.5)
    n = (math.sin(phi0) + math.sin(phi1)) / 2
    c = 1 + math.sin(phi0) * (2 * n - math.sin(phi0))
    r0 = math.sqrt(c) / n

    def raw(lam, phi):
        r = math.sqrt(c - 2 * n * math.sin(phi)) / n
        return r * math.sin(lam * n), r0 - r * math.cos(lam * n)

    k, tx, ty = 1300, 487.5, 305
    cx, cy = raw(math.radians(-0.6), math.radians(38.7))
    lam = math.radians(lon + 96)
    lam = (lam + math.pi) % (2 * math.pi) - math.pi
    px, py = raw(lam, math.radians(lat))
    return tx - k * cx + k * px, ty + k * cy - k * py


def main():
    base, state, pairs = Path(sys.argv[1]), sys.argv[2], sys.argv[3:]
    wanted = dict(p.split("=", 1) for p in pairs)
    frame = json.loads((HERE / f"{state}-counties.json").read_text())["frame"]
    records = read_dbf(base.with_suffix(".dbf"))
    shapes = read_shp(base.with_suffix(".shp"))
    out = {}
    for rec, rings in zip(records, shapes):
        area = wanted.get(rec.get("NAME", ""))
        if not area:
            continue
        polys = []
        for ring in rings:
            pts = []
            for lon, lat in ring:
                x, y = albers_usa(lon, lat)
                p = [round(x * frame["k"] + frame["dx"], 1), round(y * frame["k"] + frame["dy"], 1)]
                # Keep a point only when it moved: the 1:500k outline is finer than the map.
                if not pts or math.hypot(p[0] - pts[-1][0], p[1] - pts[-1][1]) >= 0.6:
                    pts.append(p)
            if len(pts) >= 3:
                polys.append([pts])
        out[area] = polys
    missing = set(wanted.values()) - set(out)
    if missing:
        sys.exit(f"no district for {sorted(missing)} in {base}")
    data = {"source": f"Census {base.name}, projected like us-atlas (tools/maps/extract_districts.py)",
            "districts": out}
    dest = HERE / f"{state}-districts.json"
    dest.write_text(json.dumps(data, separators=(",", ":")) + "\n")
    print(f"{dest}: {', '.join(f'{a} ({sum(len(r) for p in ps for r in p)} points)' for a, ps in out.items())}")


if __name__ == "__main__":
    main()
