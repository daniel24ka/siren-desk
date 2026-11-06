"""Bakes assets/basemap.json from Natural Earth's 1:10m admin-1 shapefile (public domain): the land
around Israel, its coast and the borders between countries, cut to the map's area. Run once by hand;
the program embeds the result, so the map needs no internet and no tile server.

    python -I -X utf8 tools/bake_basemap.py <ne_10m_admin_1_states_provinces dir> assets/basemap.json

The file's shared edges use identical vertices, so an edge seen once is a coast and an edge between two
countries is a border. The two lakes are hand-drawn, ~1 km accurate: Natural Earth's admin-1 keeps them land.
"""
import json
import os
import struct
import sys

LON0, LON1, LAT0, LAT1 = 31.5, 38.5, 27.5, 35.5

LAKES = [
    # the Sea of Galilee
    [(35.5900, 32.8840), (35.6130, 32.8880), (35.6340, 32.8700), (35.6490, 32.8330), (35.6480, 32.8040), (35.6400, 32.7780),
     (35.6300, 32.7440), (35.6020, 32.7140), (35.5820, 32.7060), (35.5680, 32.7190), (35.5550, 32.7480), (35.5430, 32.7800),
     (35.5300, 32.8010), (35.5170, 32.8270), (35.5240, 32.8500), (35.5500, 32.8720), (35.5760, 32.8810)],
    # the Dead Sea, north basin
    [(35.5550, 31.7640), (35.4800, 31.7450), (35.4600, 31.7000), (35.4580, 31.6200), (35.4200, 31.5400), (35.3950, 31.4600),
     (35.3950, 31.3800), (35.4020, 31.3000), (35.4300, 31.2300), (35.4750, 31.2500), (35.5200, 31.3000), (35.5600, 31.4000),
     (35.5800, 31.5000), (35.5820, 31.6200), (35.5800, 31.7200)],
    # the Dead Sea, south basin (evaporation ponds)
    [(35.4000, 31.1900), (35.4700, 31.2000), (35.5200, 31.1000), (35.4600, 31.0300), (35.3900, 31.0800), (35.3800, 31.1500)],
]


def read_dbf(path, field):
    """the values of one text column of a .dbf"""
    with open(path, "rb") as f:
        head = f.read(32)
        count, head_len, rec_len = struct.unpack("<IHH", head[4:12])
        fields, off = [], 1
        while True:
            d = f.read(32)
            if d[0] == 0x0D:
                break
            name = d[:11].split(b"\0")[0].decode("ascii")
            fields.append((name, off, d[16]))
            off += d[16]
        col = next(x for x in fields if x[0] == field)
        out = []
        f.seek(head_len)
        for _ in range(count):
            rec = f.read(rec_len)
            out.append(rec[col[1]:col[1] + col[2]].decode("utf-8", "replace").replace(chr(0), "").strip())
    return out


def read_shp(path):
    """[(rings of (lon, lat))] per record, of a polygon .shp"""
    out = []
    with open(path, "rb") as f:
        f.seek(100)
        while True:
            h = f.read(8)
            if len(h) < 8:
                break
            size = struct.unpack(">i", h[4:8])[0] * 2
            rec = f.read(size)
            kind = struct.unpack("<i", rec[:4])[0]
            if kind != 5:
                out.append([])
                continue
            parts, points = struct.unpack("<ii", rec[36:44])
            idx = struct.unpack("<%di" % parts, rec[44:44 + 4 * parts]) + (points,)
            xy = struct.unpack("<%dd" % (points * 2), rec[44 + 4 * parts:44 + 4 * parts + 16 * points])
            out.append([[(xy[2 * j], xy[2 * j + 1]) for j in range(idx[i], idx[i + 1])] for i in range(parts)])
    return out


def clip_ring(ring):
    """Sutherland-Hodgman against the map's box; a concave ring keeps thin edges on the box, which fill nothing"""
    def clip(pts, inside, cross):
        out = []
        for i, p in enumerate(pts):
            q = pts[(i + 1) % len(pts)]
            a, b = inside(p), inside(q)
            if a:
                out.append(p)
            if a != b:
                out.append(cross(p, q))
        return out

    def x_at(lon):
        return lambda p, q: (lon, p[1] + (q[1] - p[1]) * (lon - p[0]) / (q[0] - p[0]))

    def y_at(lat):
        return lambda p, q: (p[0] + (q[0] - p[0]) * (lat - p[1]) / (q[1] - p[1]), lat)

    pts = ring
    for inside, cross in ((lambda p: p[0] >= LON0, x_at(LON0)), (lambda p: p[0] <= LON1, x_at(LON1)),
                          (lambda p: p[1] >= LAT0, y_at(LAT0)), (lambda p: p[1] <= LAT1, y_at(LAT1))):
        if pts:
            pts = clip(pts, inside, cross)
    return pts


def in_box(p):
    return LON0 <= p[0] <= LON1 and LAT0 <= p[1] <= LAT1


def chain(edges):
    """undirected edges -> polylines"""
    near = {}
    for a, b in edges:
        near.setdefault(a, []).append(b)
        near.setdefault(b, []).append(a)
    used, lines = set(), []
    # ends and branch points first, so a line is not cut in its middle
    starts = [p for p, n in near.items() if len(n) != 2] + list(near)
    for s in starts:
        for n in list(near[s]):
            if (s, n) in used:
                continue
            line = [s, n]
            used.add((s, n))
            used.add((n, s))
            while len(near[line[-1]]) == 2:
                nxt = [x for x in near[line[-1]] if (line[-1], x) not in used]
                if not nxt:
                    break
                used.add((line[-1], nxt[0]))
                used.add((nxt[0], line[-1]))
                line.append(nxt[0])
            lines.append(line)
    return lines


def flat(pts):
    return [v for p in pts for v in (round(p[0], 4), round(p[1], 4))]


def main():
    src, dst = sys.argv[1], sys.argv[2]
    base = os.path.join(src, "ne_10m_admin_1_states_provinces")
    admins = read_dbf(base + ".dbf", "admin")
    shapes = read_shp(base + ".shp")
    land, home, owners = [], [], {}
    wanted = set()
    for admin, rings in zip(admins, shapes):
        for ring in rings:
            if not any(in_box(p) for p in ring):
                continue
            wanted.add(admin)
            for a, b in zip(ring, ring[1:]):
                if in_box(a) or in_box(b):
                    key = (a, b) if a <= b else (b, a)
                    owners.setdefault(key, []).append(admin)
            cut = clip_ring(ring[:-1] if ring[0] == ring[-1] else ring)
            if len(cut) >= 3:
                land.append(flat(cut))
                if admin in ("Israel", "West Bank", "Gaza Strip"):
                    home.append(flat(cut))
    coast = [e for e, who in owners.items() if len(who) == 1]
    borders = [e for e, who in owners.items() if len(who) == 2 and who[0] != who[1]]
    out = {
        "source": "Natural Earth 1:10m admin-1 (public domain)",
        "land": land,
        "home": home,
        "coast": [flat(x) for x in chain(coast)],
        "borders": [flat(x) for x in chain(borders)],
        "lakes": [flat(x) for x in LAKES],
    }
    with open(dst, "w", encoding="utf-8") as f:
        json.dump(out, f, separators=(",", ":"))
    print("countries:", sorted(wanted))
    print("land rings", len(land), "points", sum(len(x) for x in land) // 2)
    print("coast lines", len(out["coast"]), "points", sum(len(x) for x in out["coast"]) // 2)
    print("border lines", len(out["borders"]), "points", sum(len(x) for x in out["borders"]) // 2)
    print(os.path.getsize(dst), "bytes ->", dst)


if __name__ == "__main__":
    main()
