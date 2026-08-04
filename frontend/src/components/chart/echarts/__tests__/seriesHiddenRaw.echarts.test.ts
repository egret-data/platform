import { describe, it, expect } from "vitest";
import * as echarts from "echarts/core";
import { registerEChartsModules } from "../register";
import { buildCartesianOption, type CartesianBuildConfig } from "../options/cartesian";

/**
 * HIDING AND REORDERING A SERIES WHOSE LABEL IS NOT ITS RAW VALUE.
 *
 * THE DEFECT. `applySeriesOrder` matched `hiddenSeries` and ranked `seriesOrder` against
 * `series[].name`, and by the time it ran `decorateSeries` had already REPLACED that name with the
 * display label — the user's rename, the period the bucket covers, or the series column's own
 * number/date format. The settings panel writes RAW values (`cartesianSeriesNames` builds its list
 * from the result, `SeriesListSection` stores the name it was handed), so the two sides stopped
 * sharing a vocabulary the moment a chart was renamed, bucketed or formatted: the Show switch went
 * off, the config stored the value, and the band stayed on screen.
 *
 * THE IRONY, cited because it is the reason `__raw` exists at all: `decorateSeries` already carries
 * a comment saying "Filtering by the display label found nothing the moment a series was renamed —
 * the rename silently broke the click", written when the DRILL had this exact bug. `__raw` was added
 * then. `applySeriesOrder` was simply never updated to read it.
 *
 * WHY THIS FILE RENDERS. Reading `option.series` back proves the builder dropped the band; it does
 * not prove nothing draws it. Every case below therefore goes through real ECharts and asserts on
 * the SVG it emitted — the label of a hidden series must appear NOWHERE in the document, and the
 * order assertions read the legend text in the order ECharts laid it out. The labels are chosen to
 * be strings that occur nowhere else in a chart (no axis tick, no category name), so a bare
 * substring check on the SVG is exact.
 *
 * `option.series` is asserted too, in the same cases, because the two failures are different
 * defects: a band that survives the builder is the bug this file is named for, while a band the
 * builder dropped but ECharts still drew would be a legend/`data` mismatch in `baseOption`.
 */

const COLS = [
  { name: "month", type: "varchar" },
  { name: "cat", type: "varchar" },
  { name: "rev", type: "double" },
  { name: "cnt", type: "double" },
];
const ROWS: unknown[][] = [
  ["2026-01", "Widget", 10, 1],
  ["2026-01", "Gadget", 20, 2],
  ["2026-02", "Widget", 30, 3],
  ["2026-02", "Gadget", 40, 4],
];

/** A breakout whose values are month BUCKETS — the second trigger. The raw value is the first day
 *  of the month, which is what the panel lists and what a saved `hiddenSeries` therefore holds. */
const BUCKET_COLS = [
  { name: "region", type: "varchar" },
  { name: "bucket", type: "timestamp" },
  { name: "rev", type: "double" },
];
const BUCKET_ROWS: unknown[][] = [
  ["East", "2026-01-01", 10],
  ["East", "2026-02-01", 20],
  ["West", "2026-01-01", 30],
  ["West", "2026-02-01", 40],
];

const SIZE = { width: 720, height: 420 };

/** The option, loosely typed for reading. `cfg` stays typed: an `as never` at the call site would
 *  silence a misspelt key for the whole argument, and a fixture that builds a different chart than
 *  its test name says is how this suite has passed over a dead branch before. */
const build = (cols: { name: string; type: string }[], rows: unknown[][], cfg: CartesianBuildConfig): any =>
  buildCartesianOption(cols, rows, cfg, SIZE);

/** The SVG real ECharts drew for this option. */
function renderSVG(option: unknown): string {
  registerEChartsModules();
  const chart = echarts.init(null, null, { renderer: "svg", ssr: true, ...SIZE });
  chart.setOption({ ...(option as object), animation: false });
  const svg = chart.renderToSVGString();
  chart.dispose();
  return svg;
}

/** Every text node in the drawn chart, in emission order. */
const texts = (svg: string) =>
  [...svg.matchAll(/<text\b[^>]*>([^<]*)<\/text>/g)].map((m) => m[1]).filter((t) => t.trim() !== "");

/** The series names the builder produced (their DISPLAY names, which is what `name` holds). */
const names = (opt: { series: { name?: string }[] }) => opt.series.map((s) => s.name);

describe("hiding a series whose label differs from its raw value", () => {
  it("hides a RENAMED metric — the rename must not break its own Show switch", () => {
    const opt = build(COLS, ROWS, {
      kind: "bar", xAxis: "month", yMetrics: ["rev", "cnt"],
      seriesLabels: { rev: "Revenue" },
      // What `SeriesListSection` writes: the name it was handed, which is the RAW metric.
      hiddenSeries: ["rev"],
    });
    expect(names(opt)).toEqual(["cnt"]);
    expect(renderSVG(opt)).not.toContain("Revenue");
  });

  it("hides a RENAMED breakout band", () => {
    const opt = build(COLS, ROWS, {
      kind: "bar", xAxis: "month", yMetrics: ["rev"], series: "cat",
      seriesLabels: { Widget: "Widgets sold" },
      hiddenSeries: ["Widget"],
    });
    expect(names(opt)).toEqual(["Gadget"]);
    expect(renderSVG(opt)).not.toContain("Widgets sold");
  });

  it("hides a band the chart BUCKETED by month, whose label is the period name", () => {
    const opt = build(BUCKET_COLS, BUCKET_ROWS, {
      kind: "bar", xAxis: "region", yMetrics: ["rev"], series: "bucket",
      seriesUnit: "month", seriesType: "timestamp",
      // The raw bucket value, which is what the panel lists.
      hiddenSeries: ["2026-01-01"],
    });
    expect(names(opt)).toEqual(["February 2026"]);
    expect(renderSVG(opt)).not.toContain("January 2026");
  });

  it("hides a band whose series column carries a DATE FORMAT", () => {
    const opt = build(BUCKET_COLS, BUCKET_ROWS, {
      kind: "bar", xAxis: "region", yMetrics: ["rev"], series: "bucket",
      seriesType: "timestamp",
      columnSettings: { bucket: { date_style: "MMMM YYYY" } },
      hiddenSeries: ["2026-01-01"],
    });
    expect(names(opt)).toEqual(["February 2026"]);
    expect(renderSVG(opt)).not.toContain("January 2026");
  });

  it("hides a band whose series column carries a NUMBER FORMAT", () => {
    const cols = [
      { name: "region", type: "varchar" },
      { name: "tier", type: "bigint" },
      { name: "rev", type: "double" },
    ];
    const rows: unknown[][] = [["East", 1000, 10], ["East", 2000, 20], ["West", 1000, 30], ["West", 2000, 40]];
    const opt = build(cols, rows, {
      kind: "bar", xAxis: "region", yMetrics: ["rev"], series: "tier",
      seriesType: "bigint",
      columnSettings: { tier: { prefix: "Tier " } },
      hiddenSeries: ["1000"],
    });
    expect(names(opt)).toEqual(["Tier 2,000"]);
    expect(renderSVG(opt)).not.toContain("Tier 1,000");
  });
});

describe("reordering series whose labels differ from their raw values", () => {
  it("reorders RENAMED metrics by the raw names the panel stored", () => {
    const opt = build(COLS, ROWS, {
      kind: "bar", xAxis: "month", yMetrics: ["rev", "cnt"],
      seriesLabels: { rev: "Revenue", cnt: "Orders" },
      seriesOrder: ["cnt", "rev"],
    });
    expect(names(opt)).toEqual(["Orders", "Revenue"]);
    // The legend is the user-visible half of "order": it is also the stacking and draw order, but
    // the legend is the one ECharts writes down as text.
    const drawn = texts(renderSVG(opt));
    expect(drawn.indexOf("Orders")).toBeGreaterThanOrEqual(0);
    expect(drawn.indexOf("Orders")).toBeLessThan(drawn.indexOf("Revenue"));
  });

  it("reorders BUCKETED bands by their raw values", () => {
    const opt = build(BUCKET_COLS, BUCKET_ROWS, {
      kind: "bar", xAxis: "region", yMetrics: ["rev"], series: "bucket",
      seriesUnit: "month", seriesType: "timestamp",
      seriesOrder: ["2026-02-01", "2026-01-01"],
    });
    expect(names(opt)).toEqual(["February 2026", "January 2026"]);
  });
});

/**
 * The plain, un-renamed cases are NOT restated here. `cartesian.seriesorder.test.ts` already pins
 * them, and — measured — every assertion of that shape written for this file passed against the
 * unfixed builder, which makes it coverage of something else rather than of this defect. The two
 * kept below both FAILED before the fix: each is a downstream consumer that reads the surviving
 * series, so each is a separate way the fix could have been made to look right while the chart
 * stayed wrong.
 */
describe("what the fix must not break", () => {
  it("still keeps a hidden series out of the stack total", () => {
    const opt = build(COLS, ROWS, {
      kind: "bar", xAxis: "month", yMetrics: ["rev", "cnt"],
      seriesLabels: { rev: "Revenue" }, hiddenSeries: ["rev"],
      stacked: true, showStackTotal: true,
    });
    const visible = opt.series.find((s: { name?: string }) => s.name === "cnt");
    const total = opt.series[opt.series.length - 1];
    expect(total.__total).toBe(true);
    expect(total.data).toEqual(visible.data);
  });

  it("still leaves a renamed series' trend line behind when it is hidden", () => {
    const opt = build(COLS, ROWS, {
      kind: "line", xAxis: "month", yMetrics: ["rev", "cnt"],
      seriesLabels: { rev: "Revenue" }, hiddenSeries: ["rev"], trendLine: true,
    });
    expect(names(opt)).toEqual(["cnt", "cnt trend"]);
  });
});
