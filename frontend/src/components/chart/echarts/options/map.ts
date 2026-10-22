import type { EChartsCoreOption } from "echarts/core";
import type { ResultColumn } from "@/components/chart/ChartTable";
import type { MapType } from "@/api/charts";
import type { RegionExtent } from "../maps/registry";
import { ariaDescription, baseOption, itemTooltip, tooltipSurface } from "../base";
import {
  COLOR_FALLBACK, SERIES_PALETTE, choroplethScale, mixColor, resolveColor, scaleLightness,
} from "../palette";

export interface MapBuildConfig {
  echartsName: string;
  /** "region" (the choropleth) or "pin" (a marker per row). Undefined is "region" — see `MapConfig.type`. */
  type?: MapType;
  region?: string;
  metric?: string;
  normalize: (v: unknown) => string;
  /** Formats the metric for the tooltip; defaults to a plain locale number. */
  format?: (v: unknown) => string;
  /** Fixed text labels at geo coordinates [lon, lat] (e.g. archipelagos). */
  annotations?: { name: string; coord: [number, number] }[];
  /** The ONE accent the whole ramp derives from — Metabase's `map.colors`. Also the pin colour. */
  color?: string;
  /** Pin maps only: the numeric columns holding each row's coordinate. */
  latitude?: string;
  longitude?: string;
  /** Pin maps only: the column a pin is named by. */
  label?: string;
  /** Pin maps only: the loaded geojson's bounding box, once it has been indexed. */
  extent?: RegionExtent;
  /**
   * Region maps only: the geojson's region keys, once it has been indexed. A row whose region is
   * not one of them is drawn nowhere, so it is left out of the data and out of the legend's range
   * too (KI-046) and counted by `regionCoverage`. Undefined is "no opinion" (not indexed yet).
   */
  regions?: ReadonlySet<string>;
  /**
   * Pin maps only: extra result columns to print in a pin's tooltip — Metabase's "Additional
   * tooltip columns" (`graph.tooltip_columns`), ported for real rather than by analogy.
   *
   * WHY THIS ONE CHART FAMILY CAN HAVE THE ACTUAL FEATURE. #433 judged additional tooltip COLUMNS
   * impossible for the cartesian charts and was right: those queries GROUP BY the dimension, so a
   * raw column has no single value per drawn point and `any_value()` over a group would print one
   * of N rows at random. That objection is about aggregation, and a pin map has none — it is
   * `chartquery.rs`'s raw-records branch, one result row per marker — so a column's value on a pin
   * is that pin's own row's cell, unambiguously. #433 had to substitute dataset METRICS to have
   * something well-defined to offer; here Metabase's original feature ports as itself.
   */
  tooltipColumns?: string[];
  /** Formats one extra column's cell for the tooltip, through the dataset's own column format.
   *  Defaults to `String` for callers (tests, plain option objects) that have no formatter. */
  formatColumn?: (col: string, v: unknown) => string;
  /** Display names for the extra columns — a dataset column's `verbose_name`. Absent falls back to
   *  the machine name, the same rule `FieldZone` uses in the panel that chose them. */
  columnLabels?: Record<string, string>;
}

/** The geo layout the choropleth and its annotation scatter share, so the labels land on the map. */
const GEO_LAYOUT = { layoutCenter: ["50%", "50%"], layoutSize: "92%" };

/**
 * One cell as a coordinate, or NaN if it is not one.
 *
 * `Number()` ALONE IS NOT ENOUGH, and this is the bug the test above caught in the first draft of
 * this file rather than a hypothetical: `Number(null)` is `0`, `Number("")` is `0`, and
 * `Number([])` is `0` — all of them finite. A LEFT JOIN that produced no coordinate therefore
 * sailed through a `Number.isFinite` filter and plotted at (0, 0), which is a real location in the
 * Gulf of Guinea. On a world map every incomplete row piles up there as a confident cluster off
 * the coast of Ghana, which looks like data. Empty is checked BEFORE coercion so it cannot.
 */
function coordinate(cell: unknown): number {
  if (cell == null || cell === "") return NaN;
  return Number(cell);
}

/** Latitude runs to the poles and longitude to the antimeridian. Past those a number is not a
 *  place, whatever the projection is willing to do with it. */
const MAX_LAT = 90;
const MAX_LON = 180;

/** Which of the four things a row can be. Exactly one per row — see `pinCoverage`. */
type PinBucket = "plotted" | "offMap" | "outOfRange" | "missing";

/**
 * WHAT HAPPENS TO A PIN OUTSIDE THE MAP, MEASURED — the fact this whole classification exists for,
 * written down because it is the opposite of the natural assumption (including this file's).
 *
 * ECharts does not drop it and does not clip it. It projects the coordinate with the same linear
 * transform as every other and emits the symbol at the resulting pixel, which for something outside
 * the extent is simply off the canvas. SSR-rendering the real pin option against the real us-states
 * geojson at 540x561 and reading the transforms out of the SVG:
 *
 *     San Francisco (-122.4194,  37.7749) ->  289.5, 317.4    on canvas
 *     Paris         (   2.3522,  48.8566) ->  792.4, 257.8    252px past the right edge
 *     Tokyo         ( 139.6917,  35.6895) -> 1345.9, 328.6    806px past
 *     Cape Town     (  18.4241, -33.9249) ->  857.2, 702.7    past right AND bottom
 *
 * All of them were in the SVG. `ScatterView` does ask for a clip rect (`createCoordSysClipAreaSimply`)
 * but it is gated on `coordSys.shouldClip()`, and a `geo` built by `geoCreator` returns undefined
 * there unless the option sets `geo.clip` — so `SymbolDraw.symbolNeedsDraw` rejects nothing and the
 * pin vanishes by being painted where nobody is looking. ECharts logs nothing about it.
 *
 * THAT MEASUREMENT DECIDES THE TREATMENT, and it splits the two bad cases apart:
 *
 *   * `offMap` is RECOVERABLE. `roam: true` is on, so zooming out or panning genuinely brings a
 *     pin 252px off the edge back into view. It stays in the series and the card says it is out
 *     there. Dropping it would be the silent loss of a row, which is the defect, not the fix.
 *   * `outOfRange` is NOT recoverable. There is no zoom level at which latitude -122.4194 is a
 *     place, so drawing it is "silently draw it in the wrong place" — the other half of the failure
 *     #434 named. It is dropped AND counted, and the count is what stops that being silent.
 *
 * `outOfRange` is also the only check here that can name a FIX rather than a fact. It cannot pick
 * the column — that needs a semantic type, not a heuristic — but a transposed pair puts a longitude
 * in the latitude column, and no longitude outside ±90 can pass for a latitude. Note the asymmetry
 * that makes latitude the load-bearing half: San Francisco's 37.7749 is a perfectly legal
 * longitude, so checking longitude alone would catch nothing.
 */
function bucketOf(lat: number, lon: number, extent?: RegionExtent): PinBucket {
  if (!Number.isFinite(lat) || !Number.isFinite(lon)) return "missing";
  if (Math.abs(lat) > MAX_LAT || Math.abs(lon) > MAX_LON) return "outOfRange";
  if (extent && (lon < extent.minLon || lon > extent.maxLon || lat < extent.minLat || lat > extent.maxLat)) {
    return "offMap";
  }
  return "plotted";
}

/** One extra column's value on one pin — see `MapBuildConfig.tooltipColumns`. */
export interface PinExtra { col: string; value: unknown }

interface ClassifiedPin {
  __row: number;
  name: string;
  value: [number, number];
  bucket: PinBucket;
  extra: PinExtra[];
}

/**
 * Every row, classified — the ONE pass both the drawing and the counting read.
 *
 * SHARED DELIBERATELY. A separate count would be a second implementation of "which rows make it",
 * and the two would drift the first time either changed; a card that draws 20 pins while its own
 * caption says 21 is a worse failure than the silence, because it is confidently wrong.
 *
 * This replaces a `lati < 0 || loni < 0 ? [] : …` fast path that #434 kept as a measured
 * micro-optimisation. It had to go: returning `[]` on an unchosen field reports NO ROWS rather than
 * "these rows have no coordinate", so the count and the drawing would disagree about the one state
 * the count most needs to describe. An unchosen field now yields index -1, `r[-1]` is undefined,
 * and `coordinate` makes it NaN — the same answer the fast path was equivalent to, arrived at by
 * the path that also counts.
 */
function classifyPins(columns: ResultColumn[], rows: unknown[][], cfg: MapBuildConfig): ClassifiedPin[] {
  const idx = (name?: string) => columns.findIndex((c) => c.name === name);
  const lati = idx(cfg.latitude);
  const loni = idx(cfg.longitude);
  const labi = idx(cfg.label);
  const extras = (cfg.tooltipColumns ?? []).map((col) => ({ col, i: idx(col) }));
  return rows.map((r, i) => {
    const lat = lati < 0 ? NaN : coordinate(r[lati]);
    const lon = loni < 0 ? NaN : coordinate(r[loni]);
    return {
      // The row's own index, so a click can drill by that row's RAW cells. Matching a clicked pin
      // back to its row by comparing coordinates would be a float-equality test against a number
      // that has been through the option, the renderer and a click event.
      __row: i,
      name: labi >= 0 ? String(r[labi] ?? "") : "",
      // COORDINATE ORDER IS [LON, LAT] — see `buildPinOption`.
      value: [lon, lat] as [number, number],
      bucket: bucketOf(lat, lon, cfg.extent),
      // A column asked for by name that the result does not carry is skipped rather than shown as
      // empty: the spec and the result can disagree for one render after a field changes.
      extra: extras.filter((e) => e.i >= 0).map((e) => ({ col: e.col, value: r[e.i] })),
    };
  });
}

/** How many of the reader's rows became a pin they can see, and what happened to the rest.
 *  The four buckets PARTITION the rows: `plotted + offMap + outOfRange + missing === total`. */
export interface PinCoverage {
  total: number;
  /** Drawn, and inside the map's extent — visible without touching the chart. */
  plotted: number;
  /** Drawn, outside the extent. Reachable by zooming out; invisible until you do. */
  offMap: number;
  /** Not drawn: a number that is not a latitude or not a longitude. */
  outOfRange: number;
  /** Not drawn: no usable coordinate at all (a LEFT JOIN that produced none, a blank cell). */
  missing: number;
}

/** Counted from the same classification the builder draws from — checked BEFORE building, the way
 *  `sankeyProblem` is, and pure for the same reason: it is decidable from the input alone. */
export function pinCoverage(columns: ResultColumn[], rows: unknown[][], cfg: MapBuildConfig): PinCoverage {
  const out: PinCoverage = { total: rows.length, plotted: 0, offMap: 0, outOfRange: 0, missing: 0 };
  for (const p of classifyPins(columns, rows, cfg)) out[p.bucket] += 1;
  return out;
}

/** "is"/"are" and "has"/"have" for a count. A card that says "1 rows are outside" reads as a bug in
 *  the card rather than a fact about the data, and undermines the number it exists to state. */
const be = (n: number) => (n === 1 ? "is" : "are");
const have = (n: number) => (n === 1 ? "has" : "have");

/** "a", "a and b", "a, b and c" — an Oxford-comma-free list, matching the sankey's plain register. */
function listOut(parts: string[]): string {
  if (parts.length <= 1) return parts[0] ?? "";
  return `${parts.slice(0, -1).join(", ")} and ${parts[parts.length - 1]}`;
}

/** One clause per non-empty bucket, in the order a reader can act on them. */
function reasons(cov: PinCoverage, mapLabel: string): string[] {
  const out: string[] = [];
  if (cov.offMap > 0) out.push(`${cov.offMap} ${be(cov.offMap)} outside the ${mapLabel} map`);
  if (cov.outOfRange > 0) {
    out.push(`${cov.outOfRange} ${have(cov.outOfRange)} a latitude or longitude out of range`);
  }
  if (cov.missing > 0) out.push(`${cov.missing} ${have(cov.missing)} no latitude or longitude`);
  return out;
}

/**
 * WHY THE MAP CANNOT BE DRAWN AT ALL, in words a reader can act on — Metabase's `checkRenderable`,
 * and the same shape as `sankeyProblem`: a string, or undefined when there is no problem.
 *
 * Only when NOT ONE row lands. A map showing some of its rows is still a map, and replacing it with
 * a sentence would throw away the pins that are right in order to complain about the ones that are
 * not; `pinNote` carries that case. The two are mutually exclusive by construction so a card can
 * never say both things.
 */
/**
 * HOW MANY PINS THE BUILDER ACTUALLY DRAWS. `plotted` and `offMap` are BOTH drawn — that is the
 * distinction the two buckets exist to make, and `bucketOf`/#438 chose it deliberately: a pin
 * outside the map's extent is still emitted, because the geo component roams and panning reaches
 * it. Only `outOfRange` and `missing` are refused, and they are refused permanently.
 */
const drawnPins = (cov: PinCoverage) => cov.plotted + cov.offMap;

export function pinProblem(cov: PinCoverage, mapLabel: string): string | undefined {
  // An empty result is the empty state's job, and it already has better words for it.
  if (cov.total === 0) return undefined;
  /**
   * NOTHING DRAWN, not "nothing in view" — and the gap between those two readings is the whole of
   * this fix.
   *
   * This asked `cov.plotted > 0`, which counts only the pins inside the extent, so a result of 10
   * off-map rows and 38 out-of-range ones was replaced outright by "None of these 48 rows can be
   * shown: 10 are outside the US states map and 38 have a latitude or longitude out of range."
   * Every count in that sentence reconciled and its grammar was right, and it was still false of
   * ten of the rows: they were drawable, the builder had deliberately kept them, and roaming would
   * have found them — except that the card had been replaced by the sentence, so there was no map
   * left to roam. The message refuted itself by existing.
   *
   * #438 designed out-of-map as RECOVERABLE and out-of-range as not. Reading `plotted` here
   * collapsed the two into one bucket labelled "cannot be shown"; reading what is DRAWN restores
   * them, and `pinNote` — which now guards on the same number — picks up the case this drops.
   */
  if (drawnPins(cov) > 0) return undefined;
  /**
   * ONE HINT, because after the guard above there can only be one situation left. Nothing is drawn,
   * so `offMap` is zero, so every refused row is out of range or missing — and "Try a map that
   * covers them" (which used to be appended here whenever `offMap > 0`) is now unreachable by
   * construction rather than merely unlikely. It has gone with the branch that produced it. That
   * pairing is what made the card offer two remedies at once, pointing at different problems, one
   * of which was about rows this sentence should never have been describing.
   */
  const swap = cov.outOfRange > 0
    ? " Check whether the Latitude and Longitude columns are the other way round."
    : "";
  return `None of these ${cov.total} rows can be shown: ${listOut(reasons(cov, mapLabel))}.${swap}`;
}

/**
 * THE CARD DRAWS, BUT IT IS NOT SHOWING EVERYTHING — the line under a partial pin map.
 *
 * "N of M rows" is a number the chart could always compute and never said: #434's own live check
 * found 21 rows in and 20 pins out, and nothing on screen accounted for the difference. This is
 * that difference, said out loud, and it is the whole of the fix for the silent case — the pins
 * themselves are unchanged.
 */
export function pinNote(cov: PinCoverage, mapLabel: string): string | undefined {
  /**
   * GUARDED ON WHAT IS DRAWN, which is `pinProblem`'s guard read the other way round, so the two
   * still partition every result between them and a card can still never say both things.
   *
   * The change is which results reach this line at all. `cov.plotted === 0` used to send a map
   * whose every pin was off the extent to `pinProblem` — the case that produced a total refusal
   * over rows that were drawn. Those results land here now, and the sentence they get is true:
   * "Showing 0 of 48 rows" says nothing is IN VIEW, which is exactly the situation, while the map
   * stays on the card and the hint below says how to reach what is on it.
   */
  if (cov.total === 0 || drawnPins(cov) === 0 || cov.plotted === cov.total) return undefined;
  // Only the off-map rows are reachable this way, so the hint is only offered when there are some.
  const reach = cov.offMap > 0 ? " Zoom out or pan the map to reach them." : "";
  return `Showing ${cov.plotted} of ${cov.total} rows: ${listOut(reasons(cov, mapLabel))}.${reach}`;
}

/**
 * A PIN MAP — one marker per row at [longitude, latitude], Metabase's `map.type: "pin"`.
 *
 * WHY THIS IS A SEPARATE BRANCH AND NOT A SERIES ADDED TO THE CHOROPLETH. The two share a `geo`
 * and nothing else. A region map's `geo` exists ONLY to project the archipelago labels — it is
 * `show: false` and the `map` series draws the shapes — whereas a pin map has no `map` series at
 * all, so its `geo` *is* the drawing. And the choropleth's `visualMap` cannot be present here:
 * it re-colours a series by value, and a scatter's "value" is a coordinate pair, so it would tint
 * every pin by its longitude. Composing the two would mean a conditional on nearly every key.
 *
 * COORDINATE ORDER IS [LON, LAT]. ECharts' geo coordinate system takes x-then-y like every other
 * coordinate system it has, which is the opposite order from how the pair is spoken and from how
 * the two field pickers read down the panel. Nothing catches a transposition at runtime — the pins
 * simply land somewhere plausible-looking — so the test fixture uses a negative longitude against
 * a positive latitude, which cannot be swapped without the assertion noticing.
 *
 * THERE IS NO BASEMAP, AND THAT IS THE HONEST LIMIT OF THIS FEATURE. Pins land on the same
 * us-states / world-countries / vietnam-provinces outlines the choropleth uses, so the resolution
 * is whatever those outlines give: measured live on the Vietnam map at 540x561, the geo projection
 * ran at 30.2 px per degree of latitude — about 3.7 km per pixel — which puts an entire city
 * inside three or four pixels. "Revenue by city" across a country is coherent at that scale;
 * street-level addresses are not, and no amount of care in this file changes that. `roam: true` is
 * the mitigation rather than a nicety: zooming scales the projection with it, so a reader who
 * needs a city can get one. A real tile basemap would ship every viewed bounding box to a third
 * party, which is an infrastructure and governance decision, not a chart change.
 */
function buildPinOption(columns: ResultColumn[], rows: unknown[][], cfg: MapBuildConfig): EChartsCoreOption {
  // WHICH BUCKETS ARE DRAWN, and the asymmetry is the point — see `bucketOf` for the measurement
  // behind it. An off-map pin STAYS: it is off the canvas rather than dropped, `roam: true` can
  // reach it, and removing it would be the silent loss of a row. An out-of-range one goes: no zoom
  // level makes a latitude of -122 a place, so drawing it is planting a pin somewhere false.
  // Neither is silent — `pinCoverage` counts both and the card says so.
  const points = classifyPins(columns, rows, cfg)
    .filter((p) => p.bucket === "plotted" || p.bucket === "offMap")
    // `bucket` is stripped rather than passed through: the datum reaches a click handler and an
    // aria pass, and a field there that only the builder understands invites one of them to read it.
    .map(({ __row, name, value, extra }) => ({ __row, name, value, extra }));

  const accent = cfg.color ?? SERIES_PALETTE[0];
  const annotations = cfg.annotations ?? [];
  // The outlines are a BACKDROP, not the data — the pins are. So they take the app's own border
  // token, and the land is that border mixed halfway into the card it is drawn on: a faint wash
  // that stays faint in both themes, and opaque (mixColor flattens) so a translucent `--border`
  // in dark mode cannot let the page show through.
  const border = resolveColor("var(--border)", COLOR_FALLBACK.gridLine);
  const surface = resolveColor("var(--card)", COLOR_FALLBACK.card);
  const land = mixColor(border, surface, 0.5);
  const coord = (v: number) => (Math.round(v * 10000) / 10000).toString();
  const fmtCol = cfg.formatColumn ?? ((_col: string, v: unknown) => String(v ?? ""));

  return {
    ...baseOption(),
    aria: ariaDescription({
      kind: "Pin map",
      metric: cfg.label,
      format: (v) => String(v),
      points: points.map((p) => ({
        name: p.name || "Point",
        value: `${coord(p.value[1])}, ${coord(p.value[0])}`,
      })),
    }),
    tooltip: {
      trigger: "item" as const,
      ...tooltipSurface(),
      // Latitude FIRST here, deliberately unlike the option's own [lon, lat]: this is the string a
      // person reads, and a person writes a coordinate lat-then-lon. A named pin leads with its
      // name and keeps the numbers, which are the only thing distinguishing two pins in one city.
      formatter: (p: { name?: string; value?: unknown; data?: { extra?: PinExtra[] } }) => {
        const v = Array.isArray(p.value) ? (p.value as number[]) : [];
        const where = v.length === 2 ? `${coord(v[1])}, ${coord(v[0])}` : "";
        const head = p.name ? `${p.name}<br/>${where}` : where;
        // The extra columns come LAST, under the coordinate: name and place identify the pin, and
        // the columns are what the reader asked to see about it. Read off the datum rather than
        // looked up by index, so a tooltip cannot bind to a row the option no longer holds.
        const extra = (p.data?.extra ?? [])
          .map((e) => `<br/>${cfg.columnLabels?.[e.col]?.trim() || e.col}: ${fmtCol(e.col, e.value)}`)
          .join("");
        return `${head}${extra}`;
      },
    },
    geo: {
      map: cfg.echartsName,
      nameProperty: "key",
      // The one place roaming is on. See the resolution note above: without it a country's worth
      // of pins can never be told apart, because the outlines are far coarser than the pins are.
      roam: true,
      show: true,
      ...GEO_LAYOUT,
      itemStyle: { areaColor: land, borderColor: border, borderWidth: 0.5 },
      // A backdrop that highlights on hover is claiming to be the data. ECharts' default geo
      // emphasis is a yellow area fill, which would flash a whole state every time the pointer
      // crossed one on its way to a pin.
      emphasis: { disabled: true },
      selectedMode: false,
    },
    series: [
      {
        type: "scatter",
        coordinateSystem: "geo",
        symbol: "circle",
        symbolSize: 8,
        // Slightly transparent so overlapping pins read as denser rather than as one pin — the
        // nearest thing to a density cue available without binning.
        itemStyle: { color: accent, opacity: 0.8, borderColor: surface, borderWidth: 1 },
        data: points,
      },
      // THE ARCHIPELAGO LABELS, ON A PIN MAP TOO. The Vietnam map carries Hoàng Sa and Trường Sa
      // because it is not a correct map of Vietnam without them, and that does not stop being true
      // because the reader switched to plotting their own points on it. They are a separate series
      // so the reader's pins keep their own colour and their own tooltip, `silent` so they never
      // intercept a hover meant for a pin, and drawn in a darkened accent — the same treatment the
      // region branch gives them.
      ...(annotations.length > 0 ? [annotationSeries(annotations, scaleLightness(accent, 0.6))] : []),
    ],
  };
}

/** The fixed lon/lat labels (archipelagos) as a geo-projected scatter. Shared by both map modes so
 *  that they cannot drift apart — they are the same annotation on the same projection. */
function annotationSeries(annotations: { name: string; coord: [number, number] }[], ink: string) {
  return {
    type: "scatter",
    coordinateSystem: "geo",
    symbol: "circle",
    symbolSize: 5,
    itemStyle: { color: ink },
    silent: true,
    label: { show: true, position: "right", formatter: "{b}", fontSize: 11, fontWeight: "bold", color: ink },
    data: annotations.map((a) => ({ name: a.name, value: a.coord })),
  };
}

/** How many of a region map's rows land on a region, and the raw values of the ones that do not. */
export interface RegionCoverage { total: number; matched: number; unmatched: string[] }

/** The rows `buildMapOption` can colour and the ones it cannot, read with the builder's own rule:
 *  a row counts once it has a value, and lands when its region resolves to a key of the map. */
export function regionCoverage(columns: ResultColumn[], rows: unknown[][], cfg: MapBuildConfig): RegionCoverage {
  const ri = columns.findIndex((c) => c.name === cfg.region);
  const mi = columns.findIndex((c) => c.name === cfg.metric);
  if (ri < 0 || mi < 0) return { total: 0, matched: 0, unmatched: [] };
  let total = 0;
  const unmatched: string[] = [];
  for (const r of rows) {
    if (!Number.isFinite(Number(r[mi]))) continue;
    total += 1;
    if (!regionLands(cfg.normalize(r[ri]), cfg.regions)) unmatched.push(String(r[ri] ?? ""));
  }
  return { total, matched: total - unmatched.length, unmatched };
}

const regionLands = (name: string, regions: ReadonlySet<string> | undefined) =>
  name !== "" && (!regions || regions.has(name));

/** How many unmatched values the note names before it stops listing them. */
const NAMED_UNMATCHED = 3;

/**
 * THE REGION MAP'S OWN "N OF M ROWS" — the line a pin map has had since #434 (`pinNote`), for the
 * case a choropleth used to swallow: a row whose region names no shape on the map. Vietnam's keys
 * are names such as `TP. Hồ Chí Minh`, so `Thành phố Hồ Chí Minh` left Ho Chi Minh City unshaded
 * with nothing on screen to say why (KI-046). The first few values are named, quoted as stored, so
 * the reader can find the spelling to fix.
 */
export function regionNote(cov: RegionCoverage, mapLabel: string): string | undefined {
  const n = cov.unmatched.length;
  if (cov.total === 0 || n === 0) return undefined;
  const named = cov.unmatched.slice(0, NAMED_UNMATCHED).map((v) => `"${v}"`).join(", ");
  const more = n > NAMED_UNMATCHED ? ", …" : "";
  const match = n === 1 ? "matches" : "match";
  return `Showing ${cov.matched} of ${cov.total} rows: ${n} ${match} no region on the ${mapLabel} map (${named}${more}).`;
}

/** Build a map option: a choropleth `map` series by default, or a `scatter` of pins when
 *  `cfg.type` is "pin". The choropleth's data names are normalized region keys that match the
 *  registered geojson's `properties.key`; unmatched/NaN rows are dropped. */
export function buildMapOption(columns: ResultColumn[], rows: unknown[][], cfg: MapBuildConfig): EChartsCoreOption {
  if (cfg.type === "pin") return buildPinOption(columns, rows, cfg);
  const idx = (name?: string) => columns.findIndex((c) => c.name === name);
  const ri = idx(cfg.region);
  const mi = idx(cfg.metric);
  const data =
    ri < 0 || mi < 0
      ? []
      : rows
          .map((r) => ({ name: cfg.normalize(r[ri]), value: Number(r[mi]) }))
          // Rows that land on no region are dropped HERE rather than left for ECharts to ignore, so
          // the legend's min and max are the range of what is coloured (KI-046).
          .filter((d) => regionLands(d.name, cfg.regions) && Number.isFinite(d.value));
  const values = data.map((d) => d.value);
  const min = values.length ? Math.min(...values) : 0;
  const max = values.length ? Math.max(...values) : 1;
  const annotations = cfg.annotations ?? [];
  const fmt = cfg.format ?? ((v: unknown) => Number(v).toLocaleString());
  // One accent in, both ends of the ramp out — see `choroplethScale`.
  const accent = cfg.color ?? SERIES_PALETTE[0];
  const ramp = choroplethScale(accent);
  // The annotation dots and their labels belong to the same accent, darkened so they read on top
  // of the ramp's own dark end rather than competing with it. They were a third hard-coded teal.
  const annotationInk = scaleLightness(accent, 0.6);
  // Annotation labels are drawn by a scatter series on a hidden `geo` (which provides the lon/lat→
  // pixel projection). The choropleth `map` series and the geo share an identical layout so the
  // label points land in the right place. The pin branch reuses the same constant, which is what
  // makes the two modes frame the same country identically when you switch between them.
  const layout = GEO_LAYOUT;
  return {
    ...baseOption(),
    aria: ariaDescription({
      kind: "Map", metric: cfg.metric, dimension: cfg.region,
      format: fmt,
      points: data.map((d) => ({ name: d.name, value: d.value })),
    }),
    // The map used to print raw `toLocaleString()` numbers, ignoring the chart's column
    // formats entirely — it is the one builder that never formatted its metric.
    tooltip: itemTooltip(fmt),
    visualMap: {
      min,
      max,
      left: 8,
      bottom: 8,
      calculable: true,
      inRange: { color: ramp },
      // `calculable: true` already prints the range at the slider's ends; `text` would put a
      // SECOND copy outside them (measured live: "250 / 250" and "30 / 30"). Formatting the
      // built-in labels is what states the range once, through the metric's own format —
      // "High"/"Low" said nothing the shading had not already said.
      formatter: (v: number) => fmt(v),
      seriesIndex: 0, // colour the choropleth (map) series only, not the annotation scatter
    },
    ...(annotations.length > 0
      ? { geo: { map: cfg.echartsName, roam: false, show: false, ...layout } }
      : {}),
    series: [
      {
        type: "map",
        map: cfg.echartsName,
        nameProperty: "key",
        roam: false,
        emphasis: { label: { show: false } },
        ...(annotations.length > 0 ? layout : {}),
        data,
      },
      ...(annotations.length > 0 ? [annotationSeries(annotations, annotationInk)] : []),
    ],
  };
}
