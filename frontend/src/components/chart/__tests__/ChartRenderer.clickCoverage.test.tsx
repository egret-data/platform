// src/components/chart/__tests__/ChartRenderer.clickCoverage.test.tsx
import { describe, it, expect, vi } from "vitest";
import { render, waitFor } from "@testing-library/react";
import { ChartRenderer } from "../ChartRenderer";
import type { ChartParams, QuerySpec, VizType } from "@/api/charts";

/**
 * EVERY VIZ TYPE EITHER RESPONDS TO A CLICK OR IS ON AN EXPLICIT REFUSAL LIST. Nothing else passes.
 *
 * WHY THIS FILE EXISTS. `ChartRenderer` hands each branch's click adapter down as a prop, and a
 * branch that simply omits it draws a perfectly correct chart that does nothing when you click it.
 * That defect has now shipped TEN times in this programme — waterfall, box plot, sankey, map and
 * object in one round; funnel, gauge, progress, smartscalar and object again in the next — and the
 * reason is structural rather than ten separate oversights: **the chart-level tests call
 * `buildXOption` (or the component) directly and never cross this boundary**, so a green suite of
 * ~3,000 assertions was never evidence about it. Only a `ChartRenderer`-seam test or a live click
 * can see it, and until this file there was no seam test that ran over ALL of them.
 *
 * A new viz type fails here on the day it is added, and the only two ways to make it pass are to
 * wire a click or to write down, in `REFUSES_CLICK` below, why it does not have one. Both are
 * decisions; neither is an omission.
 *
 * WHAT IS ASSERTED, and what deliberately is not. This file checks that the handler REACHES the
 * chart — that rendering with `onValueClick` produces more click affordance than rendering without
 * it. It does NOT check what the resulting `DrillContext` says; that is per-chart and lives in
 * `ChartRenderer.test.tsx`, where each chart's filter is asserted against the column and raw value
 * it should carry. Splitting them that way is what keeps this file a single generic assertion that
 * cannot rot into nineteen hand-maintained ones — the exact failure mode the config whitelists in
 * `ChartRenderer` were.
 */

/** The affordances a drillable chart grows. Counted rather than merely detected, so a chart that
 *  already shows one for an unrelated reason (a sortable table header's `cursor-pointer`) cannot
 *  make the "without" render look drillable. */
const MARKERS = ['data-has-click="true"', "cursor-pointer", "Show records"];
const affordances = (html: string) =>
  MARKERS.reduce((n, m) => n + html.split(m).length - 1, 0);

/**
 * The mocked renderer reports, in the DOM, whether the branch above it handed over a click
 * handler. Without this the ECharts-based charts are invisible to the assertion: their handler goes
 * to `EChartsRenderer` as a prop and changes nothing about what is drawn, which is precisely why a
 * missing one was undetectable from the outside.
 */
vi.mock("@/components/chart/echarts/EChartsRenderer", () => ({
  EChartsRenderer: ({ onPointClick }: { onPointClick?: unknown }) => (
    <div data-testid="echarts" data-has-click={onPointClick ? "true" : "false"} />
  ),
}));

const spec = (s: Partial<QuerySpec> = {}): QuerySpec => ({
  metrics: [], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 1000,
  rollup_groups: [], ...s,
});

interface Fixture { params: ChartParams; columns: { name: string; type: string }[]; rows: unknown[][] }

const DIM_COLS = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }];
/** Three rows everywhere it makes sense — a one-row fixture hides an index bug and a two-row one
 *  survives a swap. */
const DIM_ROWS: unknown[][] = [["EU", 10], ["US", 7], ["APAC", 3]];
const GROUPED = spec({ metrics: ["rev"], dimensions: ["region"] });

const cartesianFixture: Fixture = {
  params: { spec: GROUPED, cartesian: { xAxis: "region", yMetrics: ["rev"] } },
  columns: DIM_COLS, rows: DIM_ROWS,
};

/**
 * One drawable fixture per viz type. Every one of these must produce a real chart rather than an
 * empty state — a branch that renders "No data" for both halves of the comparison would pass the
 * assertion below while proving nothing, so each fixture is chosen to draw.
 */
const FIXTURES: Record<VizType, Fixture> = {
  table: { params: { spec: GROUPED }, columns: DIM_COLS, rows: DIM_ROWS },
  pivot_table: {
    // The `__grp_0` marker column is not decoration: a pivot is laid out from the `GROUPING(x)`
    // markers the backend emits, and without them `pivotProblem` refuses to lay the result out at
    // all and renders its "missing the grouping columns" message instead. The vacuity guard below
    // is what caught that while this file was being written.
    params: {
      spec: spec({ metrics: ["rev"], dimensions: ["region"], rollup_groups: [["region"]] }),
      rows: ["region"], cols: [], metrics: ["rev"],
    },
    columns: [...DIM_COLS, { name: "__grp_0", type: "int" }],
    rows: DIM_ROWS.map((r) => [...r, 0]),
  },
  bar: cartesianFixture,
  row: cartesianFixture,
  line: cartesianFixture,
  area: cartesianFixture,
  scatter: cartesianFixture,
  combo: cartesianFixture,
  pie: {
    params: { spec: GROUPED, pie: { dimensions: ["region"], metric: "rev" } },
    columns: DIM_COLS, rows: DIM_ROWS,
  },
  scalar: { params: { spec: spec({ metrics: ["rev"] }), scalar: { metric: "rev" } }, columns: DIM_COLS, rows: DIM_ROWS },
  progress: {
    params: { spec: spec({ metrics: ["rev"] }), progress: { metric: "rev", goal: 100 } },
    columns: DIM_COLS, rows: DIM_ROWS,
  },
  gauge: { params: { spec: spec({ metrics: ["rev"] }), gauge: { metric: "rev" } }, columns: DIM_COLS, rows: DIM_ROWS },
  funnel: { params: { spec: GROUPED, funnel: { dimension: "region", metric: "rev" } }, columns: DIM_COLS, rows: DIM_ROWS },
  waterfall: { params: { spec: GROUPED, waterfall: { xAxis: "region", metric: "rev" } }, columns: DIM_COLS, rows: DIM_ROWS },
  smartscalar: {
    params: { spec: GROUPED, smartscalar: { dimension: "region", metric: "rev" } },
    columns: DIM_COLS, rows: DIM_ROWS,
  },
  boxplot: {
    params: { spec: spec({ columns: ["region", "rev"] }), boxplot: { category: "region", value: "rev" } },
    columns: DIM_COLS, rows: [["EU", 10], ["EU", 12], ["US", 7], ["US", 9], ["APAC", 3], ["APAC", 5]],
  },
  sankey: {
    params: { spec: GROUPED, sankey: { source: "s", target: "t", metric: "rev" } },
    columns: [{ name: "s", type: "varchar" }, { name: "t", type: "varchar" }, { name: "rev", type: "double" }],
    rows: [["A", "B", 3], ["B", "C", 2], ["A", "C", 1]],
  },
  object: { params: { spec: spec({ columns: ["region", "rev"] }) }, columns: DIM_COLS, rows: DIM_ROWS },
  map: {
    params: { spec: GROUPED, map: { mapKey: "us_states", region: "region", metric: "rev" } },
    columns: [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }],
    rows: [["California", 10], ["Texas", 7], ["Nevada", 3]],
  },
};

/**
 * Viz types that deliberately have no click, each with the reason it does not.
 *
 * DELIBERATELY EMPTY TODAY: as of this unit all nineteen respond to a click. It is not dead weight
 * — it is the escape hatch that makes the assertion above a decision point rather than an
 * obligation, so that a future chart with genuinely nothing to filter by can be recorded here in
 * one line instead of tempting someone to delete the test. Anything listed here is checked BOTH
 * ways: it must name a reason, and it must actually render no click affordance, so the list cannot
 * be used to park a half-wired chart.
 *
 * Note what does NOT belong here: a chart that refuses SOME of its elements. The waterfall's Total
 * bar, the cartesian fold's "Other" and the pie's folded slice are all per-element refusals inside
 * a chart that is otherwise fully clickable, and they are asserted where they live.
 */
const REFUSES_CLICK: Partial<Record<VizType, string>> = {};

const ALL_VIZ = Object.keys(FIXTURES) as VizType[];

/** Renders `viz` with and without a drill handler and reports how much click affordance each grew.
 *  The map loads its geojson asynchronously, so this waits for the chart to replace its own
 *  "Loading map…" placeholder — for every other type the wait is already satisfied on first paint. */
async function affordanceCounts(viz: VizType) {
  const f = FIXTURES[viz];
  const out: number[] = [];
  for (const onValueClick of [undefined, vi.fn()]) {
    const { container, unmount } = render(
      <ChartRenderer vizType={viz} params={f.params} columns={f.columns} rows={f.rows}
        onValueClick={onValueClick} />,
    );
    await waitFor(() => expect(container.textContent).not.toContain("Loading map"));
    // A fixture that draws nothing would make the comparison vacuous.
    expect(container.textContent).not.toContain("No data");
    out.push(affordances(container.innerHTML));
    unmount();
  }
  return { without: out[0], with: out[1] };
}

describe("every viz type answers a click, or says why it does not", () => {
  // The enumeration itself. `VizType` is a union, so this is the one place a new member can be
  // noticed at all — a missing fixture is a TypeScript error on `FIXTURES`, and a fixture with no
  // wiring is a failure below.
  it("covers every viz type the app can render", () => {
    expect(ALL_VIZ).toHaveLength(19);
    expect(new Set(ALL_VIZ).size).toBe(ALL_VIZ.length);
  });

  it.each(ALL_VIZ.filter((v) => !(v in REFUSES_CLICK)))(
    "%s grows a click affordance when the shell passes onValueClick",
    async (viz) => {
      const n = await affordanceCounts(viz);
      // Stated separately from the comparison so a fixture that draws a placeholder rather than a
      // chart fails as "this fixture is wrong" rather than as "this chart is unwired" — the pivot's
      // did exactly that while this file was being written, needing `rollup_groups` to lay out.
      expect(n.with).toBeGreaterThan(0);
      expect(n.with).toBeGreaterThan(n.without);
    },
  );

  it("records a reason for every viz type that refuses the click", () => {
    for (const [viz, reason] of Object.entries(REFUSES_CLICK)) {
      expect(ALL_VIZ).toContain(viz as VizType);
      expect(String(reason).length).toBeGreaterThan(20);
    }
  });

  it.each(Object.keys(REFUSES_CLICK) as VizType[])(
    "%s is on the refusal list and really does refuse",
    async (viz) => {
      const n = await affordanceCounts(viz);
      expect(n.with).toBe(n.without);
    },
  );
});
