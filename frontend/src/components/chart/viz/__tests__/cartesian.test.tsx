import { describe, it, expect } from "vitest";
import { cartesianDescriptor } from "../cartesian";
import { EMPTY_SPEC } from "@/api/charts";

const SHARED = { filters: [], rowLimit: 1000, temporalUnits: { order_date: "month" }, orderby: [] };

describe("cartesianDescriptor", () => {
  it("names the missing fields of an empty config", () => {
    expect(cartesianDescriptor("bar").requiredFields({ yMetrics: [] })).toEqual(["X-axis", "a metric"]);
  });

  it("reports nothing missing once an axis and a metric are set", () => {
    expect(cartesianDescriptor("bar").requiredFields({ xAxis: "region", yMetrics: ["total_revenue"] })).toEqual([]);
  });

  it("builds a spec with the x-axis and series as dimensions", () => {
    const spec = cartesianDescriptor("bar").buildSpec(
      { xAxis: "region", yMetrics: ["total_revenue"], series: "category" },
      SHARED,
    );
    expect(spec.dimensions).toEqual(["region", "category"]);
    expect(spec.metrics).toEqual(["total_revenue"]);
    expect(spec.row_limit).toBe(1000);
  });

  it("prunes temporal units down to the dimensions actually grouped", () => {
    const spec = cartesianDescriptor("line").buildSpec(
      { xAxis: "order_date", yMetrics: ["total_revenue"] },
      SHARED,
    );
    expect(spec.temporal_units).toEqual({ order_date: "month" });
    const other = cartesianDescriptor("line").buildSpec({ xAxis: "region", yMetrics: ["m"] }, SHARED);
    expect(other.temporal_units).toEqual({});
  });

  it("puts scatter's x-metric first in the metric list", () => {
    const spec = cartesianDescriptor("scatter").buildSpec(
      { xAxis: "region", xMetric: "order_count", yMetrics: ["total_revenue"] },
      SHARED,
    );
    expect(spec.metrics).toEqual(["order_count", "total_revenue"]);
  });

  it("ignores an x-metric for a non-scatter kind", () => {
    const spec = cartesianDescriptor("bar").buildSpec(
      { xAxis: "region", xMetric: "order_count", yMetrics: ["total_revenue"] },
      SHARED,
    );
    expect(spec.metrics).toEqual(["total_revenue"]);
  });

  it("round-trips a v1 params object into settings the descriptor reads back", () => {
    const d = cartesianDescriptor("bar");
    const cfg = d.fromParams({ spec: EMPTY_SPEC, cartesian: { xAxis: "region", yMetrics: ["m"], stacked: true } });
    const back = d.fromParams({ version: 2, spec: EMPTY_SPEC, settings: d.toSettings(cfg) });
    expect(back).toEqual(cfg);
  });

  it("exposes data controls before display controls", () => {
    const sections = cartesianDescriptor("bar").controls.map((c) => c.section);
    expect(sections.indexOf("data")).toBeLessThan(sections.indexOf("display"));
  });

  it("gives scatter an extra x-metric control the other kinds do not have", () => {
    const ids = (k: "bar" | "scatter") => cartesianDescriptor(k).controls.map((c) => c.id);
    expect(ids("scatter")).toContain("xMetric");
    expect(ids("bar")).not.toContain("xMetric");
  });
});

/**
 * A cartesian query asks for the order it draws in.
 *
 * THE ORDER IS ASCENDING BY THE DIMENSION, and Metabase's own default decided it — not preference.
 * A fresh "summarise by category" question carries NO `order-by` in its MBQL; the query processor
 * adds one implicitly, `:asc`, on every breakout column before the query runs:
 *
 *   `add-implicit-breakout-order-by` — "Fields specified in `breakout` should add an implicit
 *   ascending `order-by` subclause unless that Field is already explicitly referenced in
 *   `order-by`" (metabase src/metabase/query_processor/middleware/add_implicit_clauses.clj:71,
 *   proven by `add-order-bys-for-breakouts-test`, which asserts `:order-by [[:asc price]]`).
 *
 * So the default Metabase executes is ascending by the dimension, NOT descending by the
 * aggregation — that only appears when a question's author asks for it, as the harness's
 * `ORDER BY revenue DESC` does. It is also what egret chose twice already: `pivot/engine.ts`
 * (`sortTree`, `cmpValues` ascending) and smartscalar (`simple.tsx`, `desc: false`).
 */
describe("a cartesian spec carries the order it draws in", () => {
  it("orders a bar's dimension ascending", () => {
    const spec = cartesianDescriptor("bar").buildSpec({ xAxis: "category", yMetrics: ["revenue"] }, SHARED);
    expect(spec.orderby).toEqual([{ field: "category", desc: false }]);
  });

  // D7 / KI-045: a row chart ranks, so its default is by value (largest first), the dimension only
  // breaking ties. See orderby.test.ts.
  it("orders a row chart by its metric, descending, with the dimension as tie-breaker", () => {
    const spec = cartesianDescriptor("row").buildSpec({ xAxis: "category", yMetrics: ["revenue"] }, SHARED);
    expect(spec.orderby).toEqual([{ field: "revenue", desc: true }, { field: "category", desc: false }]);
  });

  /**
   * A temporal or numeric x-axis KEEPS THE ORDER IT ALREADY DRAWS. `sortCategories` sorts those two
   * itself — temporal chronologically, numeric ascending — and ascending-by-dimension agrees with
   * both, so nothing about the drawn axis moves; only the rows now arrive in it. CHANGING what a
   * temporal or numeric axis draws is out of scope: those two branches are Metabase's own
   * (`model/dataset.ts` `sortDataset`) and are not the defect.
   */
  it("asks a temporal x-axis for the chronological order it already draws", () => {
    const spec = cartesianDescriptor("line").buildSpec({ xAxis: "order_date", yMetrics: ["m"] }, SHARED);
    expect(spec.orderby).toEqual([{ field: "order_date", desc: false }]);
  });

  /**
   * BY EVERY BREAKOUT, X AXIS FIRST — where this used to assert "never by the breakout series" and
   * pin the x-axis alone.
   *
   * That assertion contradicted the Metabase source quoted at the top of this describe. The rule is
   * "Fields specified in `breakout`", plural, and `add-implicit-breakout-order-by` REDUCES
   * `lib/order-by` over all of them; `add-order-bys-for-breakouts-test` pins two breakouts
   * producing two clauses. Emitting one of the two left the rows deterministic in the CATEGORY and
   * arbitrary within it, which is precisely how the same card came to draw
   * `Doohickey / Gadget / Gizmo / Widget` on one page load and `Gizmo / Widget / Gadget / Doohickey`
   * on the next.
   *
   * The x-axis stays FIRST because the array is the sort precedence: the drawn axis still decides
   * the row order and the breakout only breaks its ties.
   */
  it("orders by every breakout, the x-axis first and the series after it", () => {
    const spec = cartesianDescriptor("bar").buildSpec(
      { xAxis: "category", series: "state", yMetrics: ["revenue"] }, SHARED,
    );
    expect(spec.orderby).toEqual([
      { field: "category", desc: false },
      { field: "state", desc: false },
    ]);
    expect(spec.dimensions).toEqual(["category", "state"]);
  });

  /**
   * An author's clause REPLACES the implicit one on the same field rather than being appended to,
   * in either direction — `lib/order-by` compares on the expression alone, so a DESC on the
   * breakout is not defeated by an implicit ASC. This is the input `ChartRenderer` turns into
   * `preserveSeriesOrder`, so the two halves have to agree about what a stored spec can say.
   */
  it("lets an author's sort on the breakout stand instead of adding a second clause", () => {
    const spec = cartesianDescriptor("bar").buildSpec(
      { xAxis: "category", series: "state", yMetrics: ["revenue"] },
      { ...SHARED, orderby: [{ field: "state", desc: true }] },
    );
    expect(spec.orderby).toEqual([
      { field: "state", desc: true },
      { field: "category", desc: false },
    ]);
  });

  it("leaves a chart with no x-axis unordered", () => {
    expect(cartesianDescriptor("bar").buildSpec({ yMetrics: ["revenue"] }, SHARED).orderby).toEqual([]);
    expect(cartesianDescriptor("bar").buildSpec({ xAxis: "", yMetrics: ["revenue"] }, SHARED).orderby).toEqual([]);
  });
});
