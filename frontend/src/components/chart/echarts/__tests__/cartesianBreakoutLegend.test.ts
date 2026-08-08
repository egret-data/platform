import { describe, it, expect } from "vitest";
import { buildCartesianOption } from "../options/cartesian";
import { CHART_THEME, COLOR_FALLBACK, resolveColor } from "../palette";

/**
 * THE LEGEND, THE STACK'S AXIS AND THE BREAKOUT'S ARIA — the three defects a live sweep of the
 * parity harness found concentrated in the breakout family, plus the one it found on every
 * cartesian legend in the product.
 *
 * WHAT A jsdom TEST CAN PIN HERE AND WHAT IT CANNOT, stated once because three of the four
 * findings behind this file were measured in a browser and only their CAUSE is assertable here.
 * jsdom runs no layout: it has no text metrics, so nothing below can observe that "revenue"
 * overprinted "10,000" by 6.2x15px, that the scroll pager clipped "Gadget" to a nameless chip, or
 * that a legend label sat at 2.4:1 against the dark card. What it CAN pin is the option that
 * produced each of those — the axis formatter's compact flag, the presence of the `legend` key,
 * the colour handed to `textStyle` — which is the whole of what this change alters. The pixels are
 * in the PR body, measured on the deployed bundle in both themes at 209px and 309px.
 */

const cols = [
  { name: "order_month", type: "varchar" },
  { name: "category", type: "varchar" },
  { name: "revenue", type: "double" },
];

/** Four bands over three months, each band peaking near 6K and stacking to ~24K — the shape of the
 *  harness's "MB · Area Breakout", which is the card every measurement below was taken from. */
const rows: unknown[][] = [];
for (const m of ["2026-01", "2026-02", "2026-03"]) {
  for (const [i, g] of ["Doohickey", "Gadget", "Gizmo", "Widget"].entries()) {
    rows.push([m, g, 6000 - i * 200]);
  }
}

type Axis = { id?: string; axisLabel?: { formatter?: (v: number) => string } };
type Legend = { textStyle?: { color?: string }; pageTextStyle?: { color?: string }; pageIconColor?: string };
type Built = {
  legend?: Legend;
  aria?: { label?: { description?: string } };
  yAxis: Axis | Axis[];
  grid?: { top?: number };
};

const build = (extra: Record<string, unknown>, size?: { width: number; height: number }) =>
  buildCartesianOption(
    cols,
    rows,
    { kind: "area", xAxis: "order_month", yMetrics: ["revenue"], series: "category", ...extra } as never,
    size as never,
  ) as never as Built;

const leftAxis = (o: Built) => (Array.isArray(o.yAxis) ? o.yAxis[0] : o.yAxis);
const ticks = (o: Built, vs: number[]) => vs.map((v) => leftAxis(o).axisLabel?.formatter?.(v));

describe("D1 — a cartesian legend is drawn in the theme's own ink", () => {
  /**
   * MEASURED IN THE BROWSER, both themes, on the deployed bundle: every legend label rendered at
   * the literal `#54555a` and the scroll pager at `#6d6e73`, IDENTICAL in light and dark, while
   * every axis label beside them resolved `var(--muted-foreground)` — `oklch(0.49 0.07 270.49)` in
   * light, `oklch(0.71 0.01 286.09)` in dark. Against the dark card (`oklch(0.21 0.01 285.93)`)
   * the constant sits near 2.4:1; the bar for text is 4.5:1.
   *
   * The constant is ECharts' own default, reached because the legend option named no `textStyle`
   * at all — `base.ts` records that the shared legend builders which used to carry the theme were
   * removed when the pie stopped using them, and the cartesian legend never had one of its own.
   *
   * ECHARTS CANNOT READ A CSS CUSTOM PROPERTY. The value has to be resolved and handed in, which
   * is exactly what every axis on this option already does through `axisLabelColor` — so this
   * asserts the legend now carries THAT colour rather than any literal, and the scatter builder's
   * own legend (which has carried it since it was written) is the precedent being followed rather
   * than a second answer being invented.
   */
  it("hands the legend, its pager text and its arrows the axis label's resolved colour", () => {
    const themed = resolveColor(CHART_THEME.axisLabel, COLOR_FALLBACK.axisLabel);
    const opt = build({}, { width: 900, height: 400 });
    expect(opt.legend).toBeDefined();
    expect(opt.legend?.textStyle?.color).toBe(themed);
    expect(opt.legend?.pageTextStyle?.color).toBe(themed);
    expect(opt.legend?.pageIconColor).toBe(themed);
  });

  it("carries no hard-coded grey anywhere in the legend option", () => {
    const opt = build({}, { width: 900, height: 400 });
    // The two literals the browser measured. A regression that reintroduced either would have to
    // do it through this object, since nothing else in the option describes legend ink.
    expect(JSON.stringify(opt.legend ?? {})).not.toMatch(/#54555a|#6d6e73/i);
  });
});

describe("D2 — a stacked axis compacts against the pile, not against one band", () => {
  /**
   * MEASURED at 209px AND at 309px, so this is not a fit decision that a wider card would undo:
   * "MB · Area" drew `0/5K/10K/15K/20K/25K` while "MB · Area Breakout" — same metric, same twelve
   * months, same ~24.7K maximum, same settings shape — drew `0/5,000/…/25,000`, and those wider
   * labels ran under the rotated "revenue" axis title by a measured 6.2x15px on "10,000".
   *
   * THE CAUSE IS A DISAGREEMENT BETWEEN TWO FUNCTIONS THAT DESCRIBE THE SAME AXIS. `axisExtent`
   * sums a stack per category (it has since the row-chart interval fit needed it, and says why);
   * `widestOf`, which the compact threshold reads, took the largest single DATUM. On a grouped
   * chart the two agree, because the axis really does only have to reach the tallest bar. On a
   * stack they cannot: each band peaks near 6K and the axis is drawn to the 24K pile, so the
   * threshold was asked about a number the axis never draws.
   */
  it("compacts a stacked breakout whose bands are each under the threshold", () => {
    const opt = build({ stacked: true }, { width: 900, height: 400 });
    expect(ticks(opt, [0, 10_000, 25_000])).toEqual(["0", "10K", "25K"]);
  });

  it("leaves a GROUPED breakout of the same data uncompacted, because its axis stops at 6K", () => {
    // Not an incidental pass — this is the half of the rule that must NOT move. Unstacked bands sit
    // side by side from one baseline, so the axis reaches the tallest single band and "6,000" is
    // the right label. The harness's "MB · Bar Breakout" is this case and drew `0/2,000/…/8,000`
    // in the browser, matching Metabase.
    const opt = build({ kind: "bar", stacked: false }, { width: 900, height: 400 });
    expect(ticks(opt, [0, 2000, 6000])).toEqual(["0", "2,000", "6,000"]);
  });

  it("still compacts a plain chart whose own data passes the threshold", () => {
    const big = rows.map((r) => [r[0], r[1], 30_000]);
    const opt = buildCartesianOption(
      cols, big,
      { kind: "area", xAxis: "order_month", yMetrics: ["revenue"], series: "category" } as never,
      { width: 900, height: 400 } as never,
    ) as never as Built;
    expect(ticks(opt, [30_000])).toEqual(["30K"]);
  });

  /**
   * NORMALISATION NEEDS NOTHING, and this pins that rather than leaving it to be rediscovered. A
   * normalised stack is 0-100 by construction: `isNormalized` short-circuits `axisExtent` to
   * `{lo: 0, hi: 100}` and `fmtFor` to `${Math.round(v)}%` before either consults `widestOf`, so
   * the summing below cannot reach it. Asserted because "the axis maximum changed" is true of
   * normalisation too, and the tempting fix is to teach `widestOf` about it a second time.
   */
  it("does not disturb a normalised stack, which is 0-100 before widestOf is consulted", () => {
    const opt = build({ stacked: true, normalized: true }, { width: 900, height: 400 });
    expect(ticks(opt, [0, 50, 100])).toEqual(["0%", "50%", "100%"]);
  });
});

describe("D4 — a breakout's aria names the metric, not whichever band sorts first", () => {
  /**
   * MEASURED on all three breakout cards, both sizes, both themes: "MB · Bar Breakout" announced
   * "Bar chart of DOOHICKEY by order_month. 12 values: 4 series; …" and "MB · Bar Breakout Desc",
   * whose bands sort the other way, announced "of Widget". The chart draws revenue broken out by
   * category — the band is not the subject, it is one of four.
   *
   * #400's principle is that a description must describe what is DRAWN, and the value axis of this
   * very chart is already titled "revenue" by `metricTitle` — so the name the description wants is
   * one the option computes two hundred lines above and never asked for.
   *
   * WHY THE TEST FOR THE PLAIN MULTI-SERIES CHART IS HERE TOO. The nineteen non-breakout cards were
   * swept clean, and `first?.name` is RIGHT for them: on a combo of `orders` and `revenue` the
   * first series IS a metric and the enumerated values are its own. The fix therefore has to be
   * narrow enough not to touch that, which is what the second case pins.
   */
  it("names the metric the bands share", () => {
    const opt = build({ kind: "bar" }, { width: 900, height: 400 });
    const d = opt.aria?.label?.description ?? "";
    expect(d).toMatch(/^Bar chart of revenue by order_month\./);
    expect(d).not.toMatch(/of (Doohickey|Gadget|Gizmo|Widget)/);
  });

  it("says which band the numbers it reads out belong to", () => {
    // Naming the metric alone would leave a reader hearing four bands' worth of chart and one
    // band's worth of numbers with nothing to attribute them to — trading a wrong subject for an
    // unlabelled enumeration. The band is stated instead of dropped.
    const opt = build({ kind: "bar" }, { width: 900, height: 400 });
    expect(opt.aria?.label?.description ?? "").toContain("Doohickey");
  });

  it("leaves a plain multi-metric chart naming its first series, as the swept cards do", () => {
    const two = [
      { name: "order_month", type: "varchar" },
      { name: "orders", type: "double" },
      { name: "revenue", type: "double" },
    ];
    const opt = buildCartesianOption(
      two, [["2026-01", 205, 20_000], ["2026-02", 240, 24_000]],
      { kind: "combo", xAxis: "order_month", yMetrics: ["orders", "revenue"] } as never,
      { width: 900, height: 400 } as never,
    ) as never as Built;
    const d = opt.aria?.label?.description ?? "";
    expect(d).toMatch(/^Combo chart of orders by order_month\./);
    // …and it gains NO band clause. The subject and the series being read are the same string here,
    // so `seriesName` must stay unset — a description that said "2 series, showing orders" would be
    // repeating itself, and this is the assertion that stops the new field leaking onto the
    // nineteen cards the sweep found clean.
    expect(d).not.toContain("showing");
  });
});

describe("D5 — a legend that cannot be laid out across a narrow card is not drawn", () => {
  /**
   * MEASURED at 209px on all three breakout cards, both themes: four entries were laid out across
   * ~307px inside a 193px plot, so ECharts' scroll legend clipped mid-item and the reader was left
   * with one named entry, one coloured chip with NO name, two entries gone entirely, and a "1/4"
   * pager offering four pages for four series.
   *
   * THE FIX IS METABASE'S OWN RULE, read out of its `LegendLayout.tsx` rather than invented:
   *
   *     const maxXItems = Math.floor(width / MIN_ITEM_WIDTH);   // MIN_ITEM_WIDTH  = 100
   *     const isNarrow  = width < MIN_LEGEND_WIDTH;             // MIN_LEGEND_WIDTH = 400
   *     const isVertical = maxXItems < items.length;
   *     const isVisible = hasLegend && !(isVertical && isNarrow);
   *
   * A legend needs 100px per entry; when the entries will not fit across AND the card is under
   * 400px, Metabase draws NO legend at all. Verified live at :3033 against the same four-band card
   * at 170, 214, 258 and 345px — its series legend is absent at every one of them, and the only
   * "legend" elements in the DOM are the card's own title caption.
   *
   * That is a better answer than any pager tuning, because the pager was never the defect: two of
   * four series unnamed and two invisible is not a legend, and the 24px band it costs at the top of
   * a 209px card is better spent on the plot.
   */
  /**
   * 193x246, NOT 209x262 — the PLOT, which is what this builder is handed.
   *
   * `EChartsRenderer` observes the ECharts host, and on the parity dashboard's 209px card that host
   * measures 193px across all eight charts checked (a flat 16px of card padding). Sizing these
   * fixtures from the card would test the rule against a number the builder never sees, and the
   * boundary case below is decided inside that 16px — so the fixture is the measured host and
   * `legendFitsAcross` adds the card back explicitly.
   */
  const CARD_209_PLOT = { width: 193, height: 246 };

  it("drops the legend when four bands cannot fit across a 209px card", () => {
    expect(build({ kind: "bar" }, CARD_209_PLOT).legend).toBeUndefined();
  });

  it("gives the 24px the legend would have taken back to the plot", () => {
    const wide = build({ kind: "bar" }, { width: 900, height: 400 });
    const narrow = build({ kind: "bar" }, CARD_209_PLOT);
    expect((wide.grid?.top ?? 0) - (narrow.grid?.top ?? 0)).toBe(24);
  });

  /**
   * THE BOUNDARY, ASSERTED FROM THE SIDE THAT MUST NOT MOVE — and it is the reason this rule is
   * asked about the card rather than the plot.
   *
   * `floor((193 + 16) / 100) = 2`, and two entries is not more than two, so "MB · Combo" keeps its
   * legend on the very card where the four-band breakout loses one. Both halves were measured in a
   * browser at that width: the combo's `orders` and `revenue` occupy ~128px of the 193 available
   * with no pager at all, while four bands are laid out across ~307px and clipped mid-item.
   *
   * Metabase agrees at the same card sizes, measured on this same card: its legend is drawn at a
   * 214px card and absent at a 170px one.
   */
  it("keeps a two-series legend on the same 209px card, which is where two entries do fit", () => {
    const two = [
      { name: "order_month", type: "varchar" },
      { name: "orders", type: "double" },
      { name: "revenue", type: "double" },
    ];
    const opt = buildCartesianOption(
      two, [["2026-01", 205, 20_000], ["2026-02", 240, 24_000]],
      { kind: "combo", xAxis: "order_month", yMetrics: ["orders", "revenue"] } as never,
      CARD_209_PLOT as never,
    ) as never as Built;
    expect(opt.legend).toBeDefined();
  });

  it("drops that same two-series legend on Metabase's own narrowest card, which is 170px", () => {
    // The other side of the same boundary, and the one that proves the rule still runs at all for
    // two entries. Metabase's 170px card gives its legend 168px and it draws none; egret's
    // equivalent plot is 154px, and `floor((154 + 16) / 100) = 1`.
    const two = [
      { name: "order_month", type: "varchar" },
      { name: "orders", type: "double" },
      { name: "revenue", type: "double" },
    ];
    const opt = buildCartesianOption(
      two, [["2026-01", 205, 20_000], ["2026-02", 240, 24_000]],
      { kind: "combo", xAxis: "order_month", yMetrics: ["orders", "revenue"] } as never,
      { width: 154, height: 246 } as never,
    ) as never as Built;
    expect(opt.legend).toBeUndefined();
  });

  it("keeps the four-band legend once the card is wide enough to be past `isNarrow`", () => {
    const opt = build({ kind: "bar" }, { width: 420, height: 400 });
    expect(opt.legend).toBeDefined();
  });

  it("keeps the legend when nothing measured the card, which is every jsdom chart", () => {
    // No `size` means "not laid out yet", never "tiny" — the same reading `useMeasuredSize` and
    // `measurable` already take. Hiding a legend on an unmeasured card would strip it from every
    // chart in the test suite and from the first paint of every chart in the product.
    expect(build({ kind: "bar" }).legend).toBeDefined();
  });
});
