"""Downloads the place lists the program is built with: Tzofar's public cities.json and polygons.json (alert places,
areas, shelter times, shapes, names in several languages). They are Tzofar's files, so they are not kept in this
repository; run this once before the first build. The running program keeps them up to date by itself.

    python tools/fetch_lists.py
"""
import json
import os
import sys
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "..", "assets", "lists")
BASE = "https://www.tzevaadom.co.il/static/"
# the program's own name: what it is, not what it is not
HEADERS = {"User-Agent": "SirenDesk/1.0 (personal desktop alert display)"}


def fetch(name):
    with urllib.request.urlopen(urllib.request.Request(BASE + name, headers=HEADERS), timeout=30) as r:
        body = r.read()
    json.loads(body.decode("utf-8"))  # (a broken download is not kept)
    return body


def main():
    os.makedirs(OUT, exist_ok=True)
    cities = fetch("cities.json")
    polygons = fetch("polygons.json")
    if len(json.loads(cities)["cities"]) < 1000:
        sys.exit("cities.json looks cut short; nothing written")
    for name, body in (("cities.json", cities), ("polygons.json", polygons)):
        path = os.path.join(OUT, name)
        with open(path + ".tmp", "wb") as f:
            f.write(body)
        os.replace(path + ".tmp", path)
        print(f"{name}: {len(body)} bytes -> {os.path.normpath(path)}")


if __name__ == "__main__":
    main()
