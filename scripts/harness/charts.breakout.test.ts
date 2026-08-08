/**
 * DOES THE HARNESS'S BREAKOUT CARD ACTUALLY SEE THE DEFECT IT WAS ADDED FOR?
 *
 * The "ZZ Chart Parity" dashboard carried 19 cards for thirty-one PRs and every one of them left
 * `cartesian.series` unset. There was therefore NO BROKEN-OUT CHART anywhere in the instrument, and
 * that is why PR #436's defect — the same card drawing `Doohickey / Gadget / Gizmo / Widget` on one
 * page load and `Gizmo / Widget / Gadget / Doohickey` on the next — survived thirty PRs of
 * side-by-side comparison. Not because anyone looked and missed it: because there was nothing to
 * look at.
 *
 * A CARD THAT EXISTS BUT CANNOT SEE THE DEFECT IS WORSE THAN NO CARD, because it looks like
 * coverage. So this file does not merely assert that the new entries are shaped like a breakout.
 * It runs the card's OWN CONFIG through the app's own builder twice, with the rows arriving in two
 * different orders, and asserts:
 *
 *   1. under today's builder the two arrival orders draw the SAME bands in the SAME order, and
 *   2. under the PRE-#436 builder they draw DIFFERENT orders — i.e. the card would have shown the
 *      defect on the dashboard, which is the claim the card is being added to make good.
 *
 * HOW (2) SIMULATES THE OLD CODE, stated plainly because "we simulated it" is where this kind of
 * proof usually goes soft. The pre-#436 line was `distinct(xrows.map((r) => r[gi]))` — arrival
 * order, no sort. Today's line is `sortBands(distinct(...), type, cfg.preserveSeriesOrder)`, and
 * `sortBands`' first statement is `if (preserve) return bands`. So `preserveSeriesOrder: true` does
 * not approximate the old builder, it reduces to it: same expression, same value, byte for byte.
 * That is why this is done with the flag rather than by checking out the old file.
 *
 * The rows are the REAL bands, read out of `MB Month x Category`'s own SQL literal rather than
 * retyped, so a dataset edit that changed the categories would land here rather than leaving this
 * test passing against four names that no longer exist.
 *
 * `describe`/`it`/`expect` come from vitest's globals; see `params.test.ts` for why a bare
 * `from "vitest"` cannot resolve from outside `frontend/`.
 */
import { buildCartesianOption, type CartesianBuildConfig } from "@/components/chart/echarts/options/cartesian";
import { getDescriptor } from "@/components/chart/viz/registry";
import type { CartesianConfig } from "@/api/charts";
import { chartByName, chartParams } from "./charts";
import { datasetByName } from "./datasets";

const DATASET = "MB Month x Category";

/**
 * The four bands, READ OUT OF THE DATASET rather than retyped. Every quoted literal in that SQL is
 * either a category name or a date, and a date starts with a digit — so a letters-only match picks
 * out exactly the categories, in the order the literal lists them.
 */
const BANDS = [...new Set(
  [...datasetByName(DATASET).sql.matchAll(/'([A-Za-z]+)'/g)].map((m) => m[1]),
)];

const COLUMNS = datasetByName(DATASET).columns.map((c) => ({ name: c.column_name, type: c.data_type }));

/**
 * Two months of rows with the bands introduced in a chosen order. Two months rather than twelve
 * because the question is which band the builder MEETS FIRST, and the second month only proves the
 * first month's order is not an artefact of a single group.
 */
function rowsWithBandOrder(order: string[]): unknown[][] {
  return ["2026-01-01", "2026-02-01"].flatMap((month) =>
    order.map((band, i) => [month, band, 1000 + i]));
}

/**
 * The `CartesianBuildConfig` the RENDERER would assemble for this card, derived the way
 * `ChartRenderer` derives it: the config comes back through the descriptor's own `fromParams` (the
 * function the editor calls to open a chart), and the two `preserve*` flags are read off the STORED
 * SPEC exactly as `ChartRenderer.tsx` reads them. Retyping either derivation here would let this
 * test agree with itself while the app did something else — which is the whole failure mode the
 * harness exists to avoid.
 */
function rendererConfig(chartName: string): CartesianBuildConfig {
  const entry = chartByName(chartName);
  const params = chartParams(entry);
  const cfg = getDescriptor(entry.viz)!.fromParams(params) as CartesianConfig;
  const typeOf = (n?: string) => COLUMNS.find((c) => c.name === n)?.type;
  return {
    ...cfg,
    kind: entry.viz,
    preserveCategoryOrder: (params.spec.orderby?.length ?? 0) > 0,
    preserveSeriesOrder: !!cfg.series && params.spec.orderby?.[0]?.field === cfg.series,
    xAxisUnit: params.spec.temporal_units?.[cfg.xAxis ?? ""],
    xAxisType: typeOf(cfg.xAxis),
    seriesUnit: cfg.series ? params.spec.temporal_units?.[cfg.series] : undefined,
    seriesType: typeOf(cfg.series),
  } as CartesianBuildConfig;
}

interface BuiltSeries { name: string; stack?: string }

function bandsOf(cfg: CartesianBuildConfig, rows: unknown[][]): string[] {
  const option = buildCartesianOption(COLUMNS, rows, cfg) as unknown as { series: BuiltSeries[] };
  return option.series.map((s) => s.name);
}

describe("the dataset the breakout cards stand on", () => {
  it("offers a small, stable, engine-independent set of bands", () => {
    // The harness's value is that the two products are comparable. A breakout with many categories,
    // or with values an engine could render differently, would defeat that — so the choice of
    // `category` over `state` (40+) or `order_month` (12, and already the x axis) is pinned here
    // rather than left as a comment.
    expect(BANDS).toEqual(["Doohickey", "Gadget", "Gizmo", "Widget"]);
  });
});

describe("MB · Bar Breakout — the case the instrument could not see", () => {
  it("groups by the x axis AND by the band column", () => {
    const spec = chartParams(chartByName("MB · Bar Breakout")).spec;
    expect(spec.dimensions).toEqual(["order_month", "category"]);
    expect(spec.metrics).toEqual(["revenue"]);
  });

  it("stores the breakout under the key the renderer reads", () => {
    // `cartesian.series` and nothing else. The harness has shipped a wrong settings key five times;
    // it is generated through the descriptor now, so this asserts the OUTCOME of that rather than
    // guarding a hand-written string.
    expect(chartParams(chartByName("MB · Bar Breakout")).settings["cartesian.series"]).toBe("category");
  });

  it("asks the engine for both breakouts in order, which is what makes the ROWS deterministic", () => {
    // `add-implicit-breakout-order-by` reduces over every breakout; #436 made `buildSpec` do the
    // same. Without the second clause the rows are deterministic in the month and arbitrary within
    // it, which is the data half of the defect.
    expect(chartParams(chartByName("MB · Bar Breakout")).spec.orderby).toEqual([
      { field: "order_month", desc: false },
      { field: "category", desc: false },
    ]);
  });

  it("draws the same bands in the same order however the rows arrive", () => {
    const cfg = rendererConfig("MB · Bar Breakout");
    expect(bandsOf(cfg, rowsWithBandOrder(BANDS))).toEqual(BANDS);
    expect(bandsOf(cfg, rowsWithBandOrder([...BANDS].reverse()))).toEqual(BANDS);
  });

  it("WOULD HAVE CAUGHT #436: under the pre-fix builder the same card draws two different pictures", () => {
    // The proof the card has eyes. `preserveSeriesOrder: true` short-circuits `sortBands` to
    // `return bands`, which is the pre-#436 expression verbatim — see this file's header.
    const preFix = { ...rendererConfig("MB · Bar Breakout"), preserveSeriesOrder: true };
    const arrivedAscending = bandsOf(preFix, rowsWithBandOrder(BANDS));
    const arrivedDescending = bandsOf(preFix, rowsWithBandOrder([...BANDS].reverse()));

    expect(arrivedAscending).toEqual(BANDS);
    expect(arrivedDescending).toEqual([...BANDS].reverse());
    expect(arrivedDescending).not.toEqual(arrivedAscending);
  });
});

describe("MB · Area Breakout — the same breakout, stacked", () => {
  it("stacks, which makes the band order the STACKING order and not merely the legend's", () => {
    const settings = chartParams(chartByName("MB · Area Breakout")).settings;
    expect(settings["cartesian.stacked"]).toBe(true);
    expect(settings["cartesian.series"]).toBe("category");
  });

  it("puts every band in one stack, bottom-up in band order", () => {
    const cfg = rendererConfig("MB · Area Breakout");
    const option = buildCartesianOption(COLUMNS, rowsWithBandOrder([...BANDS].reverse()), cfg) as unknown as
      { series: BuiltSeries[] };
    expect(option.series.map((s) => s.name)).toEqual(BANDS);
    // One shared stack id, or the "stacking order" this card exists to compare is not a thing.
    expect(new Set(option.series.map((s) => s.stack)).size).toBe(1);
    expect(option.series[0].stack).toBeTruthy();
  });
});

describe("MB · Bar Breakout Desc — the sorted-by-the-breakout variant", () => {
  it("makes the breakout the PRIMARY sort key, which is the only thing preserveSeriesOrder reads", () => {
    // `orderby[0].field === series`, and NOT `.some(...)`: a clause in second place sorts within
    // the first one's ties and does not decide which band the builder meets first.
    expect(chartParams(chartByName("MB · Bar Breakout Desc")).spec.orderby).toEqual([
      { field: "category", desc: true },
      { field: "order_month", desc: false },
    ]);
    expect(rendererConfig("MB · Bar Breakout Desc").preserveSeriesOrder).toBe(true);
  });

  it("keeps the descending band order the author asked for rather than sorting it away", () => {
    // The card that makes the PAIR worth having. Metabase never sorts its bands —
    // `getBreakoutDistinctValues` is `Array.from(new Set(rows.map(...)))`, arrival order — so on the
    // ascending card a builder that ALWAYS sorted would agree with Metabase by coincidence. Only a
    // card whose engine order is NOT the sorted order can tell "egret sorts" apart from "egret
    // honours the query", and this is that card.
    const cfg = rendererConfig("MB · Bar Breakout Desc");
    expect(bandsOf(cfg, rowsWithBandOrder([...BANDS].reverse()))).toEqual([...BANDS].reverse());
  });

  it("differs from MB · Bar Breakout in its ORDERBY and in nothing else", () => {
    // What makes the pair an experiment rather than two cards: same dataset, same viz, same config,
    // so any difference on screen is attributable to the sort alone.
    const asc = chartByName("MB · Bar Breakout");
    const desc = chartByName("MB · Bar Breakout Desc");
    expect(desc.dataset).toBe(asc.dataset);
    expect(desc.viz).toBe(asc.viz);
    expect(desc.config).toStrictEqual(asc.config);
    expect(asc.orderby).toBeUndefined();
    expect(desc.orderby).toStrictEqual([{ field: "category", desc: true }]);
  });
});
