#!/usr/bin/env python3
"""Download + normalize the map geojson assets for the Map viz (run once; outputs are committed).

Each feature's properties are reduced to {key, name}:
  - US states:        key = 2-letter state abbreviation, name = full state name
  - World countries:  key = ISO-A2 (ISO_A2_EH fixes Natural Earth's -99 gaps), name = country name
  - Vietnam provinces: key = province name (shapeName), name = province name
Coordinates are rounded to 2 decimals to shrink the files. Requires network.
"""
import json
import os
import urllib.request

OUT = os.path.join(os.path.dirname(__file__), "..", "frontend", "src", "components", "chart", "echarts", "maps")

US_ABBR = {
    "Alabama": "AL", "Alaska": "AK", "Arizona": "AZ", "Arkansas": "AR", "California": "CA",
    "Colorado": "CO", "Connecticut": "CT", "Delaware": "DE", "District of Columbia": "DC",
    "Florida": "FL", "Georgia": "GA", "Hawaii": "HI", "Idaho": "ID", "Illinois": "IL",
    "Indiana": "IN", "Iowa": "IA", "Kansas": "KS", "Kentucky": "KY", "Louisiana": "LA",
    "Maine": "ME", "Maryland": "MD", "Massachusetts": "MA", "Michigan": "MI", "Minnesota": "MN",
    "Mississippi": "MS", "Missouri": "MO", "Montana": "MT", "Nebraska": "NE", "Nevada": "NV",
    "New Hampshire": "NH", "New Jersey": "NJ", "New Mexico": "NM", "New York": "NY",
    "North Carolina": "NC", "North Dakota": "ND", "Ohio": "OH", "Oklahoma": "OK", "Oregon": "OR",
    "Pennsylvania": "PA", "Rhode Island": "RI", "South Carolina": "SC", "South Dakota": "SD",
    "Tennessee": "TN", "Texas": "TX", "Utah": "UT", "Vermont": "VT", "Virginia": "VA",
    "Washington": "WA", "West Virginia": "WV", "Wisconsin": "WI", "Wyoming": "WY", "Puerto Rico": "PR",
}


def fetch(url):
    with urllib.request.urlopen(url, timeout=60) as r:
        return json.loads(r.read().decode())


def round_coords(obj, nd=2):
    if isinstance(obj, list):
        if obj and isinstance(obj[0], (int, float)):
            return [round(float(x), nd) for x in obj]
        return [round_coords(x, nd) for x in obj]
    return obj


def normalize(features, key_of, name_of):
    out = []
    for f in features:
        p = f.get("properties", {})
        key = key_of(p)
        name = name_of(p)
        if not key:
            continue
        out.append({
            "type": "Feature",
            "properties": {"key": str(key), "name": str(name)},
            "geometry": {**f["geometry"], "coordinates": round_coords(f["geometry"]["coordinates"])},
        })
    return {"type": "FeatureCollection", "features": out}


def write(name, fc):
    os.makedirs(OUT, exist_ok=True)
    path = os.path.join(OUT, name)
    with open(path, "w") as fh:
        json.dump(fc, fh, separators=(",", ":"))
    print(f"  wrote {name}: {len(fc['features'])} features, {os.path.getsize(path)//1024} KB")


def main():
    us = fetch("https://raw.githubusercontent.com/PublicaMundi/MappingAPI/master/data/geojson/us-states.json")
    write("us-states.geo.json", normalize(
        us["features"],
        key_of=lambda p: US_ABBR.get(p.get("name", "")),
        name_of=lambda p: p.get("name", ""),
    ))

    world = fetch("https://raw.githubusercontent.com/nvkelso/natural-earth-vector/master/geojson/ne_110m_admin_0_countries.geojson")
    write("world-countries.geo.json", normalize(
        world["features"],
        key_of=lambda p: (p.get("ISO_A2_EH") or p.get("ISO_A2") or "").upper() if (p.get("ISO_A2_EH") or p.get("ISO_A2") or "") not in ("-99", "") else None,
        name_of=lambda p: p.get("NAME", ""),
    ))

    vmeta = fetch("https://www.geoboundaries.org/api/current/gbOpen/VNM/ADM1/")
    vurl = vmeta["gjDownloadURL"] if isinstance(vmeta, dict) else vmeta[0]["gjDownloadURL"]
    vn = fetch(vurl)
    write("vietnam-provinces.geo.json", normalize(
        vn["features"],
        key_of=lambda p: p.get("shapeName", ""),
        name_of=lambda p: p.get("shapeName", ""),
    ))


if __name__ == "__main__":
    main()
