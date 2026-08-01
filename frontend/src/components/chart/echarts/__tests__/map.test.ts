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
