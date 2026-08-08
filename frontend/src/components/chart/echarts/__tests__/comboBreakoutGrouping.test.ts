import { describe, it, expect } from "vitest";
import { buildCartesianOption, type CartesianBuildConfig } from "../options/cartesian";

/**
 * A BROKEN-OUT COMBO MUST NOT DRAW ITS BARS ON TOP OF EACH OTHER.
 *
 * `buildCartesianOption`'s breakout arm counted the bands it was about to draw as bars with the
 * series name hard-coded to `""`:
 *
 *     seriesValues.filter((_, si) => seriesDisplayFor(cfg.kind, "", si, cfg.seriesTypes) === "bar")
 *
 * `cfg.seriesTypes[""]` never matches anything, so on a combo every band fell back to the
 * POSITIONAL default (`index === 0 ? "line" : "bar"`) — while the display each band is actually
 * drawn with is resolved fifteen lines later from `groupName(sv)`. The two disagreed, and the one
 * that decides `grouped` was the wrong one.
 *
 * WHAT THAT COSTS is not a count: `grouped` is what tells `decorateSeries` to leave `barWidth`
 * alone so ECharts can auto-size the bars and place them side by side. Computed `false` on a combo
 * whose bands are all bars, every one of them is pinned at `barWidth: "80%"` at the same x, and
 * ECharts draws them stacked on top of one another with only the last one visible.
 *
 * THE ASSERTIONS ARE ON WHAT ECHARTS IS HANDED, never on the `grouped` local — a test that reached
 * inside would pass on a fix that computed the right boolean and then used it wrongly.
 *
 * The un-broken-out arm passes the real metric and was always correct; this is combo-and-breakout
 * only.
 */

const columns = [
  { name: "month", type: "varchar" },
  { name: "product", type: "varchar" },
  { name: "rev", type: "double" },
];

/** Two bands over two categories. `product` is the breakout, so the bands are its values. */
const rows = [
  ["Jan", "Widget", 10], ["Jan", "Gadget", 4],
  ["Feb", "Widget", 8], ["Feb", "Gadget", 6],
];

const base = { kind: "combo" as const, xAxis: "month", yMetrics: ["rev"], series: "product" };

interface BuiltSeries { name: string; type: string; barWidth?: string | number }
const seriesOf = (cfg: CartesianBuildConfig, cols = columns, rs: unknown[][] = rows): BuiltSeries[] =>
  (buildCartesianOption(cols, rs, cfg) as unknown as { series: BuiltSeries[] }).series;

describe("a broken-out combo sizes its bars from the bands it actually draws", () => {
  /**
   * THE MEASURED DEFECT. Both bands are set to `bar` by NAME, which is the only way a combo's
   * breakout bands can be set at all — `seriesTypes` is keyed by the series' name, and for a
   * breakout that name is the band's value.
   *
   * `barWidth` absent is the whole assertion: it is the difference between "ECharts, place these
   * side by side" and "each of you take 80% of the category slot", and it is what a reader sees.
   */
  it("leaves barWidth to ECharts when two bands are both drawn as bars", () => {
    const series = seriesOf({ ...base, seriesTypes: { Widget: "bar", Gadget: "bar" } });
    expect(series.map((s) => s.type)).toEqual(["bar", "bar"]);
    expect(series.map((s) => s.barWidth)).toEqual([undefined, undefined]);
  });

  /**
   * The control: with only ONE band drawn as a bar there is nothing to sit beside, so that band
   * keeps the pinned width. Without this, a fix that simply never wrote `barWidth` would pass the
   * test above while quietly changing every single-bar chart in the product.
   */
  it("still pins the width when only one band is a bar", () => {
    const series = seriesOf({ ...base, seriesTypes: { Widget: "bar", Gadget: "line" } });
    const bar = series.find((s) => s.type === "bar")!;
    expect(bar.barWidth).toBeDefined();
  });

  /**
   * A STACK IS NOT A GROUP. Stacked bars share an x on purpose, so `grouped` stays false and the
   * pinned width is what makes the column read as one column. Pinning `resolveStacked` here too,
   * because the fix touches the expression that feeds it.
   */
  it("does not group a stacked combo, however many bands are bars", () => {
    const series = seriesOf({ ...base, stacked: true, seriesTypes: { Widget: "bar", Gadget: "bar" } });
    expect(series.map((s) => s.barWidth)).not.toEqual([undefined, undefined]);
  });

  /**
   * THE `(empty)` BAND, which is where the two halves of this can still drift apart.
   *
   * A null or empty breakout value is drawn and labelled `(empty)` (Metabase parity), and
   * `seriesTypes` is therefore keyed by `(empty)` too — that is the name the settings panel writes,
   * because it is the name it displays. So the bar count has to look the value up through the SAME
   * `groupName` mapping `decorateSeries` and `colorsForKeys` use, not through a bare
   * `String(sv)`: `String(null)` is `"null"`, which matches no key the panel can produce.
   *
   * THE NULL BAND IS FIRST ON PURPOSE. A failed lookup falls back to the positional default, which
   * is `line` at index 0 and `bar` everywhere else — so a miss on any LATER band lands on `bar`
   * anyway and the count comes out right by accident. Only a miss at index 0 changes the answer,
   * which makes this the one arrangement that can see the bug.
   *
   * `preserveSeriesOrder` IS WHAT KEEPS IT FIRST, and it is the reason this test carries a flag its
   * siblings do not. The builder sorts the bands by their raw value now (`sortBands`), and a raw
   * `null` stringifies to `"null"`, which sorts AFTER `"Gadget"` — so the fix for the band-order
   * defect would have quietly moved the null band to index 1 and re-blinded this test, leaving it
   * green against the very bug it is named for. The flag asks for arrival order, which is exactly
   * the arrangement the paragraph above requires; nothing else about the assertion changes.
   */
  it("counts an (empty) band through the same name the panel and the palette use", () => {
    const emptyRows = [
      ["Jan", null, 10], ["Jan", "Gadget", 4],
      ["Feb", null, 8], ["Feb", "Gadget", 6],
    ];
    const series = seriesOf(
      { ...base, seriesTypes: { "(empty)": "bar", Gadget: "bar" }, preserveSeriesOrder: true },
      columns,
      emptyRows,
    );
    expect(series.map((s) => s.name)).toEqual(["(empty)", "Gadget"]);
    expect(series.map((s) => s.type)).toEqual(["bar", "bar"]);
    expect(series.map((s) => s.barWidth)).toEqual([undefined, undefined]);
  });
});
