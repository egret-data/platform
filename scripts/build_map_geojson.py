#!/usr/bin/env python3
"""Download + normalize the map geojson assets for the Map viz (run once; outputs are committed).

Each feature's properties are reduced to {key, name}:
  - US states:        key = 2-letter state abbreviation, name = full state name
  - World countries:  key = ISO-A2 (ISO_A2_EH fixes Natural Earth's -99 gaps), name = country name
  - Vietnam provinces: 34 merged provinces (2025) incl. Hoàng Sa & Trường Sa; key = name = ten_tinh
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
    with urllib.request.urlopen(url, timeout=180) as r:
        return json.loads(r.read().decode())


def round_coords(obj, nd=2):
    """Round coordinates to `nd` decimals and drop consecutive duplicate points within each ring
    (rounding collapses many high-res vertices onto the same point — deduping shrinks the file)."""
    if not isinstance(obj, list):
        return obj
    if obj and isinstance(obj[0], (int, float)):
        return [round(float(x), nd) for x in obj]  # a single [lon, lat] point
    rounded = [round_coords(x, nd) for x in obj]
    if rounded and isinstance(rounded[0], list) and rounded[0] and isinstance(rounded[0][0], (int, float)):
        ded = [rounded[0]]  # a ring (list of points) -> drop consecutive duplicates
        for q in rounded[1:]:
            if q != ded[-1]:
                ded.append(q)
        return ded
    return rounded


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
        json.dump(fc, fh, separators=(",", ":"), ensure_ascii=False)
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

    # Vietnam: the 34 merged provinces (2025 reorganization), including the Hoàng Sa (Paracel) and
    # Trường Sa (Spratly) archipelagos. Key + label are the province name (ten_tinh).
    vn = fetch("https://raw.githubusercontent.com/lamngockhuong/vietnam-3d-map/main/data/vietnam-provinces.geojson")
    write("vietnam-provinces.geo.json", normalize(
        vn["features"],
        key_of=lambda p: p.get("ten_tinh", ""),
        name_of=lambda p: p.get("ten_tinh", ""),
    ))


if __name__ == "__main__":
    main()
