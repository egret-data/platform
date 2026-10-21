import { describe, it, expect } from "vitest";
import { CHART_TYPES } from "@/components/chart/chartTypes";
import type { ChartParamsV2, VizType } from "@/api/charts";
import { getDescriptor } from "../registry";
import { authoredOrderby, honoursOrderby, honoursRowLimit, sortableFields } from "../orderby";
import { cartesianDescriptor } from "../cartesian";
import { pivotDescriptor } from "../pivot";
import { tableDescriptor, objectDescriptor } from "../table";
import { mapDescriptor } from "../map";
import { SIMPLE_DESCRIPTORS } from "../simple";
import type { SharedQueryState } from "../types";

/**
 * `orderby` is shared query state, and this file pins BOTH halves of Metabase's precedence rule
 * (`add_implicit_clauses.clj`'s `add-implicit-breakout-order-by`): an explicit sort is honoured,
 * and where there is none the implicit ascending-by-breakout default PR #394 added still applies.
 *
 * Asserting only the first half would let an implementation that ALWAYS emits `shared.orderby`
 * pass — which would silently delete #394's default and put the bar, row and box plot charts back
 * on engine order, the exact bug it fixed.
 */
const NONE: SharedQueryState = { filters: [], rowLimit: 1000, temporalUnits: {}, orderby: [] };
const AUTHORED: SharedQueryState = { ...NONE, orderby: [{ field: "revenue", desc: true }] };

const simple = (key: VizType) => SIMPLE_DESCRIPTORS.find((d) => d.key === key)!;

describe("an authored sort reaches every descriptor's spec", () => {
  // Every viz in the app, not a hand-kept list: a new one that drops `shared.orderby` on the floor
  // has to fail here rather than wait for someone to remember this file.
  const every = CHART_TYPES.map((t) => t.key);

  it("covers all nineteen viz types", () => {
    expect(every).toHaveLength(19);
  });

  for (const viz of CHART_TYPES.map((t) => t.key)) {
    // Smartscalar is the deliberate exception; its own case is below.
    if (viz === "smartscalar") continue;
    it(`${viz} emits the shared orderby`, () => {
      const d = getDescriptor(viz)!;
      expect(d.buildSpec(d.defaultConfig as never, AUTHORED).orderby).toEqual([
        { field: "revenue", desc: true },
      ]);
    });
  }

  /**
   * THE EXCEPTION, and why. `trend-comparison.ts` searches BACKWARDS from the latest row for the
   * one whose date is a whole period earlier, and stops as soon as a candidate is older than the
   * target — sound only because the rows arrive in ascending dimension order. (It used to be a
   * simpler claim, "the last two rows", which the comparisons work replaced; the precondition is
   * the same one and is now explicit rather than incidental.) An authored `revenue DESC` would
   * still draw a number and a period caption, both wrong, with nothing on screen saying so.
   */
  it("smartscalar keeps its own dimension order, because its arithmetic depends on it", () => {
    const d = simple("smartscalar");
    expect(d.buildSpec({ dimension: "order_month", metric: "revenue" } as never, AUTHORED).orderby)
      .toEqual([{ field: "order_month", desc: false }]);
  });
});

describe("with no authored sort, PR #394's implicit default is untouched", () => {
  it("a cartesian chart still orders ascending by its x-axis", () => {
    for (const kind of ["bar", "row", "line", "area", "combo", "scatter"] as const) {
      expect(cartesianDescriptor(kind).buildSpec({ xAxis: "category", yMetrics: ["revenue"] }, NONE).orderby, kind)
        .toEqual([{ field: "category", desc: false }]);
    }
  });

  it("a cartesian chart with no x-axis still asks for nothing", () => {
    expect(cartesianDescriptor("bar").buildSpec({ yMetrics: ["revenue"] }, NONE).orderby).toEqual([]);
  });

  it("smartscalar still derives its own", () => {
    expect(simple("smartscalar").buildSpec({ dimension: "order_month", metric: "revenue" } as never, NONE).orderby)
      .toEqual([{ field: "order_month", desc: false }]);
  });

  // The non-goal PR #394 pinned and this one keeps: the charts that sort themselves client-side
  // still ask the engine for nothing, so which rows survive `row_limit` does not move under them.
  it("every other descriptor still asks for nothing", () => {
    expect(pivotDescriptor().buildSpec({ rows: ["category"], cols: [], metrics: ["revenue"] }, NONE).orderby).toEqual([]);
    expect(tableDescriptor().buildSpec({ columns: ["category"], metrics: ["revenue"] }, NONE).orderby).toEqual([]);
    expect(tableDescriptor().buildSpec({ columns: ["category"], metrics: [] }, NONE).orderby).toEqual([]);
    expect(objectDescriptor().buildSpec({ columns: ["category"] }, NONE).orderby).toEqual([]);
    expect(mapDescriptor().buildSpec({ mapKey: "us_states", region: "state", metric: "revenue" } as never, NONE).orderby).toEqual([]);
    expect(simple("pie").buildSpec({ dimensions: ["category"], metric: "revenue" } as never, NONE).orderby).toEqual([]);
    expect(simple("boxplot").buildSpec({ category: "category", value: "price" } as never, NONE).orderby).toEqual([]);
    expect(simple("scalar").buildSpec({ metric: "revenue" } as never, NONE).orderby).toEqual([]);
  });
});

/**
 * The precedence rule itself, ported rather than invented. `add-implicit-breakout-order-by` adds an
 * ascending clause for each breakout by calling `lib/order-by`, which APPENDS and skips a field
 * already referenced in any direction (`order-by-distinct-key`, order_by.cljc:104). So the two
 * clauses coexist — the author's sort decides the drawn order, the breakout's decides ties — and a
 * sort ON the breakout replaces the implicit one instead of duplicating it.
 */
describe("the breakout's ascending default and an authored sort compose, as Metabase composes them", () => {
  const bar = cartesianDescriptor("bar");

  it("puts the authored sort first and keeps the breakout as the tie-break", () => {
    expect(bar.buildSpec({ xAxis: "category", yMetrics: ["revenue"] }, AUTHORED).orderby)
      .toEqual([{ field: "revenue", desc: true }, { field: "category", desc: false }]);
  });

  it("does not add a second clause for a breakout the author already sorted", () => {
    const shared: SharedQueryState = { ...NONE, orderby: [{ field: "category", desc: true }] };
    expect(bar.buildSpec({ xAxis: "category", yMetrics: ["revenue"] }, shared).orderby)
      .toEqual([{ field: "category", desc: true }]);
  });
});

/**
 * Loading a saved chart back into the editor. The stored `orderby` is authored + derived; only the
 * authored half is editable state, and hydrating both is what would turn PR #394's derived clause
 * into a permanent sort on a field the chart may not group by any more.
 */
describe("authoredOrderby separates what the author asked for from what the descriptor derived", () => {
  const barParams = (orderby: ChartParamsV2["spec"]["orderby"]): ChartParamsV2 => ({
    version: 2,
    spec: {
      metrics: ["revenue"], dimensions: ["category"], columns: [], filters: [],
      orderby, row_limit: 1000, rollup_groups: [],
    },
    settings: { "cartesian.xAxis": "category", "cartesian.yMetrics": ["revenue"] },
  });

  it("recovers nothing from a chart saved with only the implicit breakout clause", () => {
    const p = barParams([{ field: "category", desc: false }]);
    expect(authoredOrderby(p, getDescriptor("bar"))).toEqual([]);
  });

  it("recovers the author's clause and drops the derived one", () => {
    const p = barParams([{ field: "revenue", desc: true }, { field: "category", desc: false }]);
    expect(authoredOrderby(p, getDescriptor("bar"))).toEqual([{ field: "revenue", desc: true }]);
  });

  // Same field, other direction: the author overrode the implicit clause rather than repeating it,
  // so it is theirs and must survive.
  it("keeps a descending sort on the breakout itself", () => {
    const p = barParams([{ field: "category", desc: true }]);
    expect(authoredOrderby(p, getDescriptor("bar"))).toEqual([{ field: "category", desc: true }]);
  });

  it("round-trips: what it recovers rebuilds the spec it came from", () => {
    const stored = [{ field: "revenue", desc: true }, { field: "category", desc: false }];
    const p = barParams(stored);
    const recovered = authoredOrderby(p, getDescriptor("bar"));
    expect(getDescriptor("bar")!.buildSpec(
      { xAxis: "category", yMetrics: ["revenue"] } as never,
      { ...NONE, orderby: recovered },
    ).orderby).toEqual(stored);
  });

  it("passes a descriptor that derives nothing straight through", () => {
    const p: ChartParamsV2 = {
      version: 2,
      spec: {
        metrics: ["revenue"], dimensions: ["category"], columns: [], filters: [],
        orderby: [{ field: "revenue", desc: true }], row_limit: 1000, rollup_groups: [],
      },
      settings: { "pie.dimensions": ["category"], "pie.metric": "revenue" },
    };
    expect(authoredOrderby(p, getDescriptor("pie"))).toEqual([{ field: "revenue", desc: true }]);
  });
});

/**
 * What the sort control is allowed to offer. `orderby_sql` (crates/egret-bi/src/chartquery.rs)
 * resolves a field as a METRIC, then as a DIMENSION of the spec, then as a raw COLUMN, and answers
 * anything else with a 400 — so the options have to come from the spec the descriptor builds, never
 * from the dataset's column list.
 */
describe("sortableFields", () => {
  it("offers an aggregate spec's breakout and its aggregation", () => {
    const spec = cartesianDescriptor("bar").buildSpec(
      { xAxis: "category", yMetrics: ["revenue"] }, NONE,
    );
    expect(sortableFields(spec)).toEqual(["category", "revenue"]);
  });

  it("offers a raw-records spec's columns — the third arm of orderby_sql", () => {
    const spec = simple("boxplot").buildSpec({ category: "category", value: "score" }, NONE);
    expect(spec.metrics).toEqual([]);
    expect(sortableFields(spec)).toEqual(["category", "score"]);
  });

  it("offers nothing for a query that returns one row", () => {
    // An aggregate with no breakout is a single row; ordering it is inert. It is also the only
    // shape where `spec.columns` could be offered and then IGNORED — `build`'s aggregate branch
    // never reads them, so ORDER BY one would not be in the GROUP BY.
    expect(sortableFields(simple("scalar").buildSpec({ metric: "revenue" }, NONE))).toEqual([]);
  });

  it("offers each field once even when the spec names it twice", () => {
    const spec = simple("boxplot").buildSpec({ category: "category", value: "category" }, NONE);
    expect(sortableFields(spec)).toEqual(["category"]);
  });
});

/**
 * Whether a descriptor honours an authored sort AT ALL, asked of the descriptor rather than of a
 * hardcoded list — a viz that drops `shared.orderby` must lose the control automatically.
 */
describe("honoursOrderby", () => {
  it("is true for the descriptors that pass an authored sort through", () => {
    expect(honoursOrderby(getDescriptor("bar")!, { xAxis: "category", yMetrics: ["revenue"] } as never)).toBe(true);
    expect(honoursOrderby(getDescriptor("boxplot")!, { category: "category", value: "score" } as never)).toBe(true);
    expect(honoursOrderby(getDescriptor("table")!, { columns: ["category"], metrics: [] } as never)).toBe(true);
  });

  it("is false for the trend, whose dimension order is a precondition of its arithmetic", () => {
    // `trend-comparison.ts` searches backwards for the row one period earlier and stops at the
    // first candidate older than the target, which needs ascending order; an authored sort would
    // draw a number and a caption computed from rows that are neither.
    expect(honoursOrderby(getDescriptor("smartscalar")!, { dimension: "day", metric: "revenue" } as never)).toBe(false);
    // Also with nothing configured yet — the control must not flicker into existence.
    expect(honoursOrderby(getDescriptor("smartscalar")!, {} as never)).toBe(false);
  });
});

// KI-051: the editor showed Row limit on charts whose query is one aggregate row regardless.
describe("honoursRowLimit", () => {
  const ask = (key: VizType) => {
    const d = getDescriptor(key)!;
    return honoursRowLimit(d, d.defaultConfig as never);
  };
  it("is false for the one-row charts: Big number, Progress and Gauge", () => {
    expect(ask("scalar")).toBe(false);
    expect(ask("progress")).toBe(false);
    expect(ask("gauge")).toBe(false);
  });
  it("is true for a chart whose rows the limit decides", () => {
    for (const key of ["bar", "row", "table", "pie", "map", "boxplot", "funnel"] as VizType[]) {
      expect(ask(key), key).toBe(true);
    }
  });
});
