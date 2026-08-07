import { describe, it, expect } from "vitest";
import { buildMapOption } from "../options/map";
import { MAPS, indexRegionNames, resolveRegion } from "../maps/registry";
import { SERIES_PALETTE, choroplethScale, setLightness, saturateColor, scaleLightness } from "../palette";

const cols = [{ name: "state", type: "varchar" }, { name: "rev", type: "double" }];
const rows = [["California", 100], ["Texas", 40], ["Nevada", 250]];
const base = { echartsName: "us_states", region: "state", metric: "rev", normalize: (v: unknown) => String(v) };

describe("buildMapOption", () => {
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
