import type { EChartsCoreOption } from "echarts/core";
import type { ResultColumn } from "@/components/chart/ChartTable";
import type { MapType } from "@/api/charts";
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
  const idx = (name?: string) => columns.findIndex((c) => c.name === name);
  const lati = idx(cfg.latitude);
  const loni = idx(cfg.longitude);
  const labi = idx(cfg.label);
  // A FAST PATH, NOT THE GUARANTEE. Mutating this `||` to `&&` survives the suite, and the
  // equivalence was measured rather than argued: with one field unchosen the index is -1, `r[-1]`
  // is `undefined`, and `coordinate` returns NaN, so the filter below empties the series anyway —
  // all five unchosen/misnamed combinations produce byte-identical output either way. It stays
  // because skipping the map over every row is worth one comparison, and it says out loud that an
  // unchosen field means no pins. The REAL defence against a bad coordinate is `coordinate` +
  // the `Number.isFinite` filter; do not weaken those on the strength of this line.
  const points =
    lati < 0 || loni < 0
      ? []
      : rows
          .map((r, i) => ({
            // The row's own index, so a click can drill by that row's RAW cells. Matching a clicked
            // pin back to its row by comparing coordinates would be a float-equality test against a
            // number that has been through the option, the renderer and a click event.
            __row: i,
            name: labi >= 0 ? String(r[labi] ?? "") : "",
            value: [coordinate(r[loni]), coordinate(r[lati])] as [number, number],
          }))
          // Dropped rather than plotted at zero — see `coordinate` for why that needs saying.
          .filter((p) => Number.isFinite(p.value[0]) && Number.isFinite(p.value[1]));

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
      formatter: (p: { name?: string; value?: unknown }) => {
        const v = Array.isArray(p.value) ? (p.value as number[]) : [];
        const where = v.length === 2 ? `${coord(v[1])}, ${coord(v[0])}` : "";
        return p.name ? `${p.name}<br/>${where}` : where;
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
          .filter((d) => d.name !== "" && Number.isFinite(d.value));
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
