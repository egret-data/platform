import { describe, it, expect } from "vitest";
import { buildMapOption, pinCoverage, pinProblem, pinNote, regionCoverage, regionNote } from "../options/map";
import { MAPS, indexRegionNames, indexRegionExtent, regionExtent, resolveRegion } from "../maps/registry";
import { SERIES_PALETTE, choroplethScale, setLightness, saturateColor, scaleLightness } from "../palette";

const cols = [{ name: "state", type: "varchar" }, { name: "rev", type: "double" }];
const rows = [["California", 100], ["Texas", 40], ["Nevada", 250]];
const base = { echartsName: "us_states", region: "state", metric: "rev", normalize: (v: unknown) => String(v) };

describe("buildMapOption", () => {
  // KI-046: a row naming no region was ignored by ECharts but still stretched the legend's range.
  it("leaves rows that match no region out of the data and out of the legend's range", () => {
    const regions = new Set(["California", "Texas"]);
    const opt = buildMapOption(cols, rows, { ...base, regions }) as any;
    expect(opt.series[0].data.map((d: { name: string }) => d.name)).toEqual(["California", "Texas"]);
    expect([opt.visualMap.min, opt.visualMap.max]).toEqual([40, 100]);
  });

  it("maps rows to normalised region keys, dropping what does not resolve", () => {
    const opt = buildMapOption(cols, [...rows, ["", 5], ["Ohio", NaN]], base) as any;
    expect(opt.series[0].data).toEqual([
      { name: "California", value: 100 },
      { name: "Texas", value: 40 },
      { name: "Nevada", value: 250 },
    ]);
  });

  /**
   * Both ends of the ramp derive from ONE accent — Metabase's `getColorplethColorScale`. egret
   * hard-coded `["#e6f4f1", "#0f9b8e"]`, two teals belonging to no palette: they agreed with each
   * other and with nothing else, so the map drew in a colour the design system had never heard of
   * and a configurable accent could not have worked at all.
   */
  it("derives the whole ramp from one accent", () => {
    const opt = buildMapOption(cols, rows, { ...base, color: "#3366CC" }) as any;
    expect(opt.visualMap.inRange.color).toEqual(choroplethScale("#3366CC"));
    // The light end is genuinely light and the dark end genuinely dark — the ramp has a direction.
    const [light, dark] = opt.visualMap.inRange.color;
    expect(light).not.toBe(dark);
    expect(light.toLowerCase()).not.toBe("#e6f4f1");
  });

  it("falls back to the first palette accent", () => {
    const opt = buildMapOption(cols, rows, base) as any;
    expect(opt.visualMap.inRange.color).toEqual(choroplethScale(SERIES_PALETTE[0]));
  });

  // "High"/"Low" told a reader nothing the shading had not already said. The ends now state the
  // actual range, through the metric's own format.
  /**
   * The legend states the range ONCE, through the metric's format. `calculable: true` already
   * prints the ends, so adding `text` renders a second copy beside them — measured live as
   * "250 / 250" and "30 / 30" before this was corrected. Formatting the built-in labels is the fix.
   */
  it("formats the legend's own range labels, without adding a second pair", () => {
    const opt = buildMapOption(cols, rows, { ...base, format: (v) => `$${v}` }) as any;
    expect(opt.visualMap.min).toBe(40);
    expect(opt.visualMap.max).toBe(250);
    expect(opt.visualMap.formatter(250)).toBe("$250");
    expect(opt.visualMap.text).toBeUndefined();
  });

  it("colours the annotation marks from the same accent", () => {
    const opt = buildMapOption(cols, rows, {
      ...base, color: "#3366CC", annotations: [{ name: "Isle", coord: [1, 2] }],
    }) as any;
    const scatter = opt.series.find((s: any) => s.type === "scatter");
    expect(scatter.itemStyle.color).toBe(scatter.label.color);
    expect(scatter.itemStyle.color).not.toBe("#0f766e");
  });
});

/**
 * THE PIN MAP — individual markers at latitude/longitude, Metabase's `map.type: "pin"`.
 *
 * These assertions were written before the branch existed and watched to fail. What they are
 * really pinning is the ONE thing about this feature that is easy to get backwards and impossible
 * to notice from a screenshot of a symmetric test fixture: an ECharts `geo` coordinate system
 * takes `[longitude, latitude]`, which is the opposite order from how every human names a
 * coordinate pair and the opposite order from how the field pickers are read top to bottom. A
 * fixture whose lat and lon happen to be similar numbers would pass either way, so the rows below
 * use deliberately un-swappable values (a latitude that is not a legal longitude and vice versa).
 */
const pinCols = [
  { name: "city", type: "varchar" },
  { name: "lat", type: "double" },
  { name: "lon", type: "double" },
];
// San Francisco and Denver. `lon` is negative and `lat` is not, so a transposed pair is visible.
const pinRows = [
  ["San Francisco", 37.7749, -122.4194],
  ["Denver", 39.7392, -104.9903],
];
const pinBase = {
  echartsName: "us_states",
  type: "pin" as const,
  latitude: "lat",
  longitude: "lon",
  normalize: (v: unknown) => String(v),
};

describe("buildMapOption — pin map", () => {
  it("plots one scatter point per row, as [lon, lat] and not [lat, lon]", () => {
    const opt = buildMapOption(pinCols, pinRows, pinBase) as any;
    const scatter = opt.series.find((s: any) => s.type === "scatter");
    expect(scatter.coordinateSystem).toBe("geo");
    expect(scatter.data.map((d: any) => d.value)).toEqual([
      [-122.4194, 37.7749],
      [-104.9903, 39.7392],
    ]);
  });

  /** The choropleth's `geo` exists only to project the archipelago labels, so it is `show: false`
   *  and un-roamable — the `map` series draws the shapes. A pin map has no `map` series at all, so
   *  its `geo` IS the drawing, and it has to be roamable or a whole country's worth of pins can
   *  never be told apart (see the resolution note in options/map.ts). */
  it("shows the geo component and lets the reader roam it", () => {
    const opt = buildMapOption(pinCols, pinRows, pinBase) as any;
    expect(opt.geo.show).toBe(true);
    expect(opt.geo.roam).toBe(true);
    expect(opt.geo.map).toBe("us_states");
  });

  /**
   * NO `map` SERIES AND NO `visualMap`. Both would be actively wrong rather than merely spare: a
   * second `map` series would paint region shapes on top of the geo the pins are projected onto,
   * and a `visualMap` re-colours series by value — pointed at a scatter whose "value" is a
   * coordinate pair it would tint every pin by its longitude.
   */
  it("draws no choropleth series and no value ramp", () => {
    const opt = buildMapOption(pinCols, pinRows, pinBase) as any;
    expect(opt.series.some((s: any) => s.type === "map")).toBe(false);
    expect(opt.visualMap).toBeUndefined();
  });

  /**
   * THE (0, 0) TRAP, which the first draft of the pin branch fell into. `Number(null)` is `0` and
   * `Number("")` is `0` — both finite — so a `Number.isFinite` filter alone passes a row that has
   * no coordinate at all and plots it in the Gulf of Guinea, where on a world map every incomplete
   * row piles up as a confident-looking cluster off Ghana. Each empty shape is listed separately
   * so that a fix covering only `null` still fails here.
   */
  it("drops rows whose coordinates are not finite, rather than plotting them at zero", () => {
    const rowsWithHoles = [
      ...pinRows,
      ["null lat", null, -100],
      ["undefined lon", 40, undefined],
      ["empty string", "", -100],
      ["not a number", 40, "n/a"],
    ];
    const opt = buildMapOption(pinCols, rowsWithHoles, pinBase) as any;
    const scatter = opt.series.find((s: any) => s.type === "scatter");
    expect(scatter.data.map((d: any) => d.__row)).toEqual([0, 1]);
  });

  /** A genuine zero coordinate is a real place (the Prime Meridian crosses the equator, and it
   *  crosses plenty of land), so the guard above must reject EMPTY rather than reject zero. */
  it("still plots a real zero coordinate", () => {
    const opt = buildMapOption(pinCols, [["Null Island", 0, 0]], pinBase) as any;
    const scatter = opt.series.find((s: any) => s.type === "scatter");
    expect(scatter.data.map((d: any) => d.value)).toEqual([[0, 0]]);
  });

  /**
   * The datum carries its SOURCE ROW INDEX so a click can drill by the row's own raw cells.
   * Matching a clicked pin back to a row by comparing coordinates would be a float equality test
   * against a number that has been through the option, the renderer and a click event.
   */
  it("carries each point's source row index for the drill", () => {
    const opt = buildMapOption(pinCols, [...pinRows], pinBase) as any;
    const scatter = opt.series.find((s: any) => s.type === "scatter");
    expect(scatter.data.map((d: any) => d.__row)).toEqual([0, 1]);
  });

  it("names each pin from the label column when one is chosen", () => {
    const opt = buildMapOption(pinCols, pinRows, { ...pinBase, label: "city" }) as any;
    const scatter = opt.series.find((s: any) => s.type === "scatter");
    expect(scatter.data.map((d: any) => d.name)).toEqual(["San Francisco", "Denver"]);
  });

  /**
   * THE ARCHIPELAGOS SURVIVE PIN MODE. This was implemented the other way first — suppressing the
   * annotations on a pin map, on the presentation argument that two labelled dots among a reader's
   * own pins read as data — and corrected: egret's Vietnam map carries Hoàng Sa and Trường Sa
   * because the map is not correct without them, and a reader switching to pin mode is not a
   * reason for a Vietnam basemap to lose two archipelagos. Asserted so it cannot be re-argued
   * silently.
   */
  it("keeps the archipelago labels on a Vietnam pin map, beside the reader's own pins", () => {
    const opt = buildMapOption(pinCols, pinRows, {
      ...pinBase,
      echartsName: "vietnam",
      annotations: [{ name: "Hoàng Sa", coord: [112.4, 16.5] }, { name: "Trường Sa", coord: [114.2, 9.5] }],
    }) as any;
    const scatters = opt.series.filter((s: any) => s.type === "scatter");
    expect(scatters).toHaveLength(2);
    // The reader's pins are series 0 and are NOT silent (they have a tooltip and a drill);
    // the annotations are silent, so they never intercept a hover meant for a pin.
    expect(scatters[0].silent).toBeUndefined();
    expect(scatters[1].silent).toBe(true);
    expect(scatters[1].data.map((d: any) => d.name)).toEqual(["Hoàng Sa", "Trường Sa"]);
  });

  it("plots nothing at all when either coordinate field is unchosen", () => {
    const opt = buildMapOption(pinCols, pinRows, { ...pinBase, longitude: undefined }) as any;
    const scatter = opt.series.find((s: any) => s.type === "scatter");
    expect(scatter.data).toEqual([]);
  });
});

/**
 * THE VIETNAM ANNOTATIONS ARE THE PIN BRANCH'S BLAST RADIUS. Hoàng Sa and Trường Sa are drawn by
 * the SAME geo-projected scatter the pin map now shares, so a pin branch that reorganised this
 * builder could take both archipelagos off egret's Vietnam map — which is not a cosmetic
 * regression. Asserted here as a whole rather than as a colour (which the test above already
 * covers) so that the region branch keeps its hidden geo, its layout and both labels.
 */
describe("buildMapOption — the region map keeps its annotations", () => {
  const annotations = [
    { name: "Hoàng Sa", coord: [112.4, 16.5] as [number, number] },
    { name: "Trường Sa", coord: [114.2, 9.5] as [number, number] },
  ];

  it("still draws both archipelagos on a hidden, un-roamable geo", () => {
    const opt = buildMapOption(cols, rows, { ...base, echartsName: "vietnam", annotations }) as any;
    expect(opt.geo.show).toBe(false);
    expect(opt.geo.roam).toBe(false);
    const scatter = opt.series.find((s: any) => s.type === "scatter");
    expect(scatter.coordinateSystem).toBe("geo");
    expect(scatter.data).toEqual([
      { name: "Hoàng Sa", value: [112.4, 16.5] },
      { name: "Trường Sa", value: [114.2, 9.5] },
    ]);
    // …and the choropleth is still there beside them, sharing the geo's layout.
    expect(opt.series.some((s: any) => s.type === "map")).toBe(true);
  });
});

describe("choroplethScale", () => {
  /**
   * Metabase's rule, stated independently of the implementation: the light end is the accent taken
   * to 92% lightness, the dark end is it darkened by 0.2, and both gain 0.1 saturation. Asserted
   * against the pieces rather than by re-running the same expression, which would pass for any
   * formula.
   */
  it("is 92%-lightness to darkened-0.2, both saturated", () => {
    const [light, dark] = choroplethScale("#509EE3");
    expect(light).toBe(saturateColor(setLightness("#509EE3", 0.92), 1.1));
    // `Color.darken(0.2)` is an HSL lightness multiply by 0.8.
    expect(dark).toBe(saturateColor(scaleLightness("#509EE3", 0.8), 1.1));
  });

  it("puts the ends the right way round for any accent", () => {
    for (const accent of SERIES_PALETTE) {
      const [light, dark] = choroplethScale(accent);
      expect(luminance(light)).toBeGreaterThan(luminance(dark));
    }
  });
});

/** Rough relative brightness, enough to order two ends of a ramp. */
function luminance(hex: string): number {
  const m = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(hex);
  if (!m) return 0;
  const [r, g, b] = m.slice(1).map((h) => parseInt(h, 16) / 255);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

describe("resolveRegion", () => {
  /**
   * A region column may hold the geojson's KEY or its NAME, and egret only ever matched the key.
   * `us_states` is keyed by two-letter code, so a perfectly ordinary column of "California" matched
   * nothing: the map drew every state in the palest shade, with no message. Seen live before this
   * was fixed. Metabase keeps hand-written name→code tables for exactly this; deriving them from
   * the geojson keeps them from drifting off the shapes they have to match.
   */
  const geo = {
    features: [
      { properties: { key: "CA", name: "California" } },
      { properties: { key: "TX", name: "Texas" } },
      { properties: { key: "NY", name: "New York" } },
    ],
  };

  it("accepts the key or the full name, in any case", () => {
    indexRegionNames("us_states", geo);
    const def = MAPS.us_states;
    expect(resolveRegion(def, "CA")).toBe("CA");
    expect(resolveRegion(def, "California")).toBe("CA");
    expect(resolveRegion(def, "  new york ")).toBe("NY");
    expect(resolveRegion(def, "tx")).toBe("TX");
  });

  it("leaves an unknown region to the map's own normaliser rather than inventing a match", () => {
    indexRegionNames("us_states", geo);
    expect(resolveRegion(MAPS.us_states, "Atlantis")).toBe("ATLANTIS");
  });

  // A chart renders before the geojson finishes loading; that first pass must not throw.
  it("falls back cleanly when the map has not been indexed", () => {
    expect(resolveRegion(MAPS.vietnam, "An Giang")).toBe("An Giang");
  });
});

/**
 * WHAT ACTUALLY HAPPENS TO A PIN OUTSIDE THE MAP'S EXTENT, MEASURED — the fact this whole block
 * exists to surface, recorded here because it is the opposite of what a reader (and this file's
 * first draft) assumes.
 *
 * ECharts does NOT drop it and does NOT clip it: it emits the symbol at its projected pixel, which
 * for an out-of-extent coordinate is simply off the canvas. Measured by SSR-rendering the real pin
 * option against the real us-states geojson at 540x561 (the size #434 measured live) and reading
 * the `<path>` transforms straight out of the SVG:
 *
 *     San Francisco (-122.4194,  37.7749) -> matrix(...,  289.5, 317.4)   on canvas
 *     New York      ( -74.0060,  40.7128) -> matrix(...,  484.6, 301.6)   on canvas
 *     Paris         (   2.3522,  48.8566) -> matrix(...,  792.4, 257.8)   252px past the right edge
 *     Tokyo         ( 139.6917,  35.6895) -> matrix(..., 1345.9, 328.6)   806px past
 *     Cape Town     (  18.4241, -33.9249) -> matrix(...,  857.2, 702.7)   past right AND bottom
 *
 * All seven elements were in the SVG; none carried any marker of being out of bounds, and ECharts
 * logged nothing. `ScatterView` DOES ask for a clip rect — `createCoordSysClipAreaSimply` — but it
 * is gated on `coordSys.shouldClip()`, and a `geo` built by `geoCreator` returns `undefined` there
 * unless the option sets `geo.clip`, so `SymbolDraw`'s `symbolNeedsDraw` never rejects anything.
 * A pin therefore vanishes by being painted somewhere nobody is looking.
 *
 * TWO CONSEQUENCES THAT DECIDE THIS DESIGN. First, the pin is not lost: `roam: true` means zooming
 * out or panning genuinely brings it back, so the honest thing is to keep drawing it and SAY it is
 * out there — dropping it would be the silent loss this exists to fix. Second, `ariaDescription`
 * already lists every point, so before this change a screen reader heard seven pins on a map that
 * showed five, and nothing reconciled the two.
 *
 * THE EXTENT IS THE RIGHT TEST, and that is also measured rather than assumed: the geo's own view
 * rect came back as x in [21.5, 518.5], y in [136.9, 424.1] — the geojson's bounding box projected
 * into `layoutSize: "92%"` — so "inside the bounding box" and "inside the drawn map area" are the
 * same predicate, and the bounding box can be computed with no layout at all. The us-states box
 * used below is that geojson's real one, lon [-188.900, -65.630] lat [17.930, 71.350]; it reaches
 * to -188.9 because the Aleutians wrap past the antimeridian, which is why Vancouver and Mexico
 * City are INSIDE it and do draw, in the blank margin beyond the outlines.
 */
describe("pinCoverage — every row lands in exactly one bucket", () => {
  const US = { minLon: -188.9, maxLon: -65.63, minLat: 17.93, maxLat: 71.35 };
  const cov = (rows: unknown[][], extra: Record<string, unknown> = {}) =>
    pinCoverage(pinCols, rows, { ...pinBase, extent: US, ...extra });

  it("counts a coordinate outside the map's extent as off-map, not as plotted", () => {
    // Paris on a map of the United States.
    const rows = [...pinRows, ["Paris", 48.8566, 2.3522]];
    expect(cov(rows)).toStrictEqual({ total: 3, plotted: 2, offMap: 1, outOfRange: 0, missing: 0 });
  });

  it("counts a row with no usable coordinate as missing rather than off-map", () => {
    const rows = [...pinRows, ["no fix", null, ""], ["not a number", 40, "n/a"]];
    expect(cov(rows)).toStrictEqual({ total: 4, plotted: 2, offMap: 0, outOfRange: 0, missing: 2 });
  });

  /**
   * THE FAILURE #434 WAS MOST WORRIED ABOUT, and the one a range check can actually catch. A
   * transposed pair puts a longitude in the latitude column, and -122.4194 is not a latitude on
   * any planet — so this needs no guess about which column is which to say the pair is wrong.
   * Note the asymmetry that makes latitude the load-bearing half: the matching 37.7749 IS a legal
   * longitude, so checking longitude alone would have caught nothing here.
   */
  it("counts a transposed pair as out of range, not as off-map", () => {
    const rows = pinRows.map(([city, lat, lon]) => [city, lon, lat]);
    expect(cov(rows)).toStrictEqual({ total: 2, plotted: 0, offMap: 0, outOfRange: 2, missing: 0 });
  });

  /** BOTH LIMITS, AND BOTH ARE BOUNDARIES. A mutation run found `MAX_LAT = 90 -> 91` surviving:
   *  every fixture was either legal or wildly illegal, so nothing pinned where the edge actually
   *  is. 90.5 and 180.5 are the values that can only pass if the limit is the right one. */
  it("counts a latitude past the pole as out of range", () => {
    expect(cov([["bad lat", 90.5, -100]])).toStrictEqual(
      { total: 1, plotted: 0, offMap: 0, outOfRange: 1, missing: 0 },
    );
    expect(cov([["bad lat", -90.5, -100]])).toStrictEqual(
      { total: 1, plotted: 0, offMap: 0, outOfRange: 1, missing: 0 },
    );
  });

  it("counts a longitude past the antimeridian as out of range", () => {
    expect(cov([["bad lon", 40, 180.5]])).toStrictEqual(
      { total: 1, plotted: 0, offMap: 0, outOfRange: 1, missing: 0 },
    );
    expect(cov([["bad lon", 40, -180.5]])).toStrictEqual(
      { total: 1, plotted: 0, offMap: 0, outOfRange: 1, missing: 0 },
    );
  });

  /**
   * OFF-MAP BY LATITUDE ALONE. Another mutation survivor: every off-map fixture here was Paris,
   * Tokyo or Cape Town, all of them outside by LONGITUDE, so dropping the two latitude clauses from
   * the extent test changed nothing that was asserted. This row sits at a thoroughly American
   * longitude and a Central American latitude — inside the box on one axis, outside on the other.
   */
  it("counts a row outside the extent's latitude band, at an on-map longitude", () => {
    expect(cov([["Panama", 8.98, -79.52]])).toStrictEqual(
      { total: 1, plotted: 0, offMap: 1, outOfRange: 0, missing: 0 },
    );
    // And past the northern edge too, so a fix covering only one direction still fails here.
    expect(cov([["North Pole-ish", 80, -100]])).toStrictEqual(
      { total: 1, plotted: 0, offMap: 1, outOfRange: 0, missing: 0 },
    );
  });

  /** The buckets PARTITION the rows — no row counted twice, none uncounted. Asserted as an
   *  identity so that a future bucket cannot be added without keeping the sum honest. */
  it("partitions the rows, so the counts always sum to the total", () => {
    const rows = [
      ...pinRows, ["Paris", 48.8566, 2.3522], ["no fix", null, null],
      ["backwards", -122.4194, 37.7749], ["Null Island", 0, 0],
    ];
    const c = cov(rows);
    expect(c.plotted + c.offMap + c.outOfRange + c.missing).toBe(c.total);
    expect(c.total).toBe(rows.length);
  });

  /** The geojson is loaded asynchronously, so the first render has no extent to judge against.
   *  Nothing may be called off-map on the strength of an extent nobody supplied. */
  it("calls nothing off-map before the geojson has been indexed", () => {
    const rows = [...pinRows, ["Paris", 48.8566, 2.3522]];
    expect(pinCoverage(pinCols, rows, pinBase)).toStrictEqual(
      { total: 3, plotted: 3, offMap: 0, outOfRange: 0, missing: 0 },
    );
  });

  it("counts nothing at all when a coordinate field is unchosen", () => {
    expect(cov(pinRows, { longitude: undefined })).toStrictEqual(
      { total: 2, plotted: 0, offMap: 0, outOfRange: 0, missing: 2 },
    );
  });
});

describe("buildMapOption — pin map, what is drawn and what is not", () => {
  const US = { minLon: -188.9, maxLon: -65.63, minLat: 17.93, maxLat: 71.35 };

  /**
   * AN OFF-MAP PIN IS STILL DRAWN. It is reachable — `roam: true`, and the measurement above shows
   * it lands 252px past the right edge of a 540px canvas, which zooming out brings back. Dropping
   * it would be the silent loss of a row, which is the defect, not the fix. The message says it is
   * out there; the series keeps it.
   */
  it("keeps an off-map pin in the series, because zooming out can reach it", () => {
    const rows = [...pinRows, ["Paris", 48.8566, 2.3522]];
    const opt = buildMapOption(pinCols, rows, { ...pinBase, extent: US }) as any;
    const scatter = opt.series.find((s: any) => s.type === "scatter");
    expect(scatter.data.map((d: any) => d.__row)).toEqual([0, 1, 2]);
  });

  /**
   * A COORDINATE THAT IS NOT A COORDINATE IS NOT DRAWN, unlike an off-map one. There is nowhere
   * on the globe a latitude of -122.4194 could be reached by roaming, so plotting it is the
   * "silently draw it in the wrong place" half of the failure rather than the "silently drop it"
   * half. It is dropped AND counted — `pinCoverage.outOfRange` is what stops that being silent.
   */
  it("drops a latitude that is not a latitude, rather than projecting it anyway", () => {
    const rows = [...pinRows, ["backwards", -122.4194, 37.7749]];
    const opt = buildMapOption(pinCols, rows, { ...pinBase, extent: US }) as any;
    const scatter = opt.series.find((s: any) => s.type === "scatter");
    expect(scatter.data.map((d: any) => d.__row)).toEqual([0, 1]);
  });

  /** The poles and the antimeridian are real places; the guard rejects what is BEYOND them. */
  it("still plots the extremes of the legal range", () => {
    const opt = buildMapOption(pinCols, [["pole", 90, -180], ["anti", -90, 180]], pinBase) as any;
    const scatter = opt.series.find((s: any) => s.type === "scatter");
    expect(scatter.data.map((d: any) => d.value)).toEqual([[-180, 90], [180, -90]]);
  });
});

/**
 * THE WORDS, and where they go. Two presentations because there are two situations, and the split
 * follows what `MapChart` and `sankeyProblem` already do rather than inventing a third thing:
 * a chart that cannot be drawn at all is REPLACED by a sentence (`MapChart` already does this for
 * "Couldn't load the map" and "No data"), and a chart that draws but is not showing everything
 * keeps its pins and gains a line underneath.
 */
describe("pinProblem and pinNote", () => {
  const all = (n: number) => ({ total: n, plotted: n, offMap: 0, outOfRange: 0, missing: 0 });

  it("says nothing when every row is plotted", () => {
    expect(pinProblem(all(21), "US states")).toBeUndefined();
    expect(pinNote(all(21), "US states")).toBeUndefined();
  });

  it("says nothing at all about an empty result, which is the empty state's job", () => {
    const none = { total: 0, plotted: 0, offMap: 0, outOfRange: 0, missing: 0 };
    expect(pinProblem(none, "US states")).toBeUndefined();
    expect(pinNote(none, "US states")).toBeUndefined();
  });

  /** #434's own live check found 21 rows in and 20 pins out. This is that number, said out loud. */
  it("counts the plotted rows against the total, and names the reason", () => {
    const note = pinNote({ total: 21, plotted: 20, offMap: 0, outOfRange: 0, missing: 1 }, "US states");
    expect(note).toContain("20 of 21");
    expect(note).toMatch(/latitude/i);
    // A partial map is still a map — it is not replaced.
    expect(pinProblem({ total: 21, plotted: 20, offMap: 0, outOfRange: 0, missing: 1 }, "US states"))
      .toBeUndefined();
  });

  it("names the map a row fell outside of, and how to reach it", () => {
    const note = pinNote({ total: 21, plotted: 10, offMap: 11, outOfRange: 0, missing: 0 }, "US states");
    expect(note).toContain("10 of 21");
    expect(note).toContain("US states");
    expect(note).toMatch(/zoom out/i);
  });

  it("names every reason at once when there is more than one", () => {
    const note = pinNote({ total: 10, plotted: 4, offMap: 3, outOfRange: 2, missing: 1 }, "World countries");
    // THE WHOLE SENTENCE, not three separate substrings. Asserting the clauses individually let a
    // mutation that joined them with ", " instead of "… and …" survive — every clause was present
    // and the sentence was still not English. Singular/plural agreement is pinned here too, so a
    // card can never say "1 rows have no…" and read as a bug in the card rather than a fact.
    expect(note).toBe(
      "Showing 4 of 10 rows: 3 are outside the World countries map, "
      + "2 have a latitude or longitude out of range and 1 has no latitude or longitude."
      + " Zoom out or pan the map to reach them.",
    );
  });

  /**
   * MOVED, and the fixture is exactly why: 21 rows, ALL of them off-map, NONE of them refused.
   *
   * This asserted that such a result replaces the chart with "None of these 21 rows can be shown".
   * Every one of those 21 pins is DRAWN — `bucketOf` puts a coordinate outside the extent in
   * `offMap` rather than refusing it precisely because the geo component roams and panning reaches
   * it (#438), and `PinCoverage`'s own field comment says so: "Drawn, outside the extent. Reachable
   * by zooming out; invisible until you do." So the old sentence was false of every row it counted,
   * and it removed the map that made the claim recoverable.
   *
   * The refusal itself has not gone soft — the case below, where nothing is drawn at all, still
   * gets it. What moved is which results reach it.
   */
  it("keeps the chart when every pin is drawn but none is in view, and says so", () => {
    const cov = { total: 21, plotted: 0, offMap: 21, outOfRange: 0, missing: 0 };
    expect(pinProblem(cov, "US states")).toBeUndefined();
    expect(pinNote(cov, "US states")).toBe(
      "Showing 0 of 21 rows: 21 are outside the US states map."
      + " Zoom out or pan the map to reach them.",
    );
  });

  it("replaces the chart when not one row can be plotted", () => {
    // Nothing DRAWN, which after the change above means nothing off-map either: these 21 are out of
    // range, and out of range is the bucket that is not recoverable by any amount of roaming.
    const cov = { total: 21, plotted: 0, offMap: 0, outOfRange: 21, missing: 0 };
    const problem = pinProblem(cov, "US states");
    expect(problem).toMatch(/none/i);
    expect(problem).toContain("21");
    /**
     * AND IT NO LONGER NAMES THE MAP, which is a consequence worth stating rather than an omission.
     * The map's label appears in exactly one clause — "N are outside the <map> map" — and a refusal
     * can no longer carry that clause, because a refusal now means nothing was drawn and an off-map
     * pin IS drawn. Naming "US states" over a set of rows whose coordinates are not coordinates
     * would point the reader at the map when the map is not the problem; that misdirection is
     * precisely what the old wording did to the ten off-map rows on the live card.
     */
    expect(problem).not.toContain("US states");
    // and the note stands down, so the card never says both things.
    expect(pinNote(cov, "US states")).toBeUndefined();
  });

  /** The one message that can name a FIX rather than a fact, which is the whole value of a range
   *  check: it cannot pick the column, but it can say the pair is the wrong way round. */
  it("suggests the transposition when the coordinates are out of range", () => {
    const problem = pinProblem({ total: 21, plotted: 0, offMap: 0, outOfRange: 21, missing: 0 }, "US states");
    expect(problem).toMatch(/latitude/i);
    expect(problem).toMatch(/longitude/i);
    expect(problem).toMatch(/other way round|swapped|reversed/i);
  });
});

describe("regionExtent", () => {
  /** A ring, a polygon-with-hole and a multipolygon — the three shapes the three bundled geojsons
   *  actually use, so the walk cannot be right for one nesting depth only. */
  const geo = {
    features: [
      { properties: { key: "A" }, geometry: { type: "Polygon", coordinates: [[[-10, 5], [-8, 5], [-8, 7], [-10, 7]]] } },
      { properties: { key: "B" }, geometry: { type: "MultiPolygon", coordinates: [[[[20, -30], [22, -30], [22, -28]]]] } },
    ],
  };

  it("derives the bounding box from the geometry, at any nesting depth", () => {
    indexRegionExtent("test_map", geo);
    expect(regionExtent("test_map")).toStrictEqual({ minLon: -10, maxLon: 22, minLat: -30, maxLat: 7 });
  });

  /**
   * A POSITION IS TWO NUMBERS, and the walk checks BOTH before treating an array as one. Found by a
   * surviving mutant: dropping the second check changed nothing that was asserted, because no
   * fixture contained a malformed position. A geojson is a checked-in file that a human can edit,
   * and a stray one-number entry read as a position would widen the box on one axis only — which
   * would then report perfectly ordinary rows as off-map, the confident-wrong-message failure this
   * whole change exists to avoid.
   */
  it("ignores an array that is not a position", () => {
    indexRegionExtent("ragged_map", {
      features: [
        { geometry: { type: "Polygon", coordinates: [[[-10, 5], [-8, 7]]] } },
        // A single number where a pair belongs, far outside the real box on both axes.
        { geometry: { type: "Polygon", coordinates: [[[-999], [888]]] } },
      ],
    });
    expect(regionExtent("ragged_map")).toStrictEqual({ minLon: -10, maxLon: -8, minLat: 5, maxLat: 7 });
  });

  it("is undefined for a map that has not been indexed", () => {
    expect(regionExtent("never_loaded")).toBeUndefined();
  });

  /** A geojson with no usable geometry must yield NO extent rather than an inside-out one made of
   *  Infinities, which would call every row on earth off-map. */
  it("is undefined when the geojson carries no coordinates", () => {
    indexRegionExtent("empty_map", { features: [] });
    expect(regionExtent("empty_map")).toBeUndefined();
  });
});

/**
 * ADDITIONAL TOOLTIP COLUMNS on the pin map — Metabase's `graph.tooltip_columns`. See
 * `MapConfig.tooltipColumns`: this is the one chart family in egret where the feature is
 * well-posed, because a raw-records query has no GROUP BY for a column's value to be ambiguous
 * under. The tooltip is a string, so it is asserted as one.
 */
describe("buildMapOption — pin map tooltip columns", () => {
  const cols = [
    { name: "city", type: "varchar" }, { name: "lat", type: "double" },
    { name: "lon", type: "double" }, { name: "rev", type: "double" },
  ];
  const rows = [["San Francisco", 37.7749, -122.4194, 1234.5]];
  const cfg = { ...pinBase, label: "city", tooltipColumns: ["rev"] };
  const tip = (c: Record<string, unknown>, r = rows) => {
    const opt = buildMapOption(cols, r, { ...cfg, ...c }) as any;
    const scatter = opt.series.find((s: any) => s.type === "scatter");
    const d = scatter.data[0];
    // ECharts hands an item tooltip the datum on `params.data`, alongside `name` and `value`. The
    // extras are read from there rather than from a closure over the rows, so that a tooltip can
    // never bind to a row the option no longer holds — which is what this shape is asserting.
    return opt.tooltip.formatter({ name: d.name, value: d.value, data: d });
  };

  it("prints each extra column under the name and the coordinate", () => {
    // Name first, then where, then what the reader asked about it — the coordinate identifies the
    // pin and the columns describe it, so reversing the order would bury the identity.
    expect(tip({})).toBe("San Francisco<br/>37.7749, -122.4194<br/>rev: 1234.5");
  });

  it("formats an extra column through the dataset's own column format", () => {
    expect(tip({ formatColumn: (col: string, v: unknown) => `$${Number(v).toFixed(2)} (${col})` }))
      .toContain("rev: $1234.50 (rev)");
  });

  it("uses the column's display title when the shell has one", () => {
    expect(tip({ columnLabels: { rev: "Revenue" } })).toContain("Revenue: 1234.5");
    // A blank title is not a title — it falls back to the machine name rather than printing ": ".
    expect(tip({ columnLabels: { rev: "  " } })).toContain("rev: 1234.5");
  });

  /** The spec and the result can disagree for one render after a field changes. A name the result
   *  does not carry is skipped, not printed as an empty row that looks like a null in the data. */
  it("skips a column the result does not carry", () => {
    expect(tip({ tooltipColumns: ["rev", "not_selected"] })).toBe(
      "San Francisco<br/>37.7749, -122.4194<br/>rev: 1234.5",
    );
  });

  it("is unchanged when no extra column is asked for", () => {
    expect(tip({ tooltipColumns: undefined })).toBe("San Francisco<br/>37.7749, -122.4194");
  });
});

describe("regionCoverage and regionNote (KI-046)", () => {
  const regions = new Set(["California", "Texas"]);
  it("counts the rows that land and names the ones that do not", () => {
    const cov = regionCoverage(cols, rows, { ...base, regions });
    expect(cov).toEqual({ total: 3, matched: 2, unmatched: ["Nevada"] });
    expect(regionNote(cov, "US states")).toBe('Showing 2 of 3 rows: 1 matches no region on the US states map ("Nevada").');
  });
  it("names three values and says there are more", () => {
    const many = [["A", 1], ["B", 2], ["C", 3], ["D", 4], ["Texas", 5]];
    const note = regionNote(regionCoverage(cols, many, { ...base, regions }), "US states");
    expect(note).toBe('Showing 1 of 5 rows: 4 match no region on the US states map ("A", "B", "C", …).');
  });
  it("is silent when every row lands, and when the map is not indexed yet", () => {
    expect(regionNote(regionCoverage(cols, [["Texas", 1]], { ...base, regions }), "US states")).toBeUndefined();
    expect(regionNote(regionCoverage(cols, rows, base), "US states")).toBeUndefined();
  });
});
