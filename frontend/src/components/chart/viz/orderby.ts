import type { AnyChartParams, ChartOrderBy, QuerySpec } from "@/api/charts";
import type { SharedQueryState, VizDescriptor } from "./types";

const same = (a: ChartOrderBy, b: ChartOrderBy) => a.field === b.field && a.desc === b.desc;

/** A field name no dataset can produce, used only to ask a descriptor a yes/no question. */
const PROBE = "__orderby_probe__";

const NO_SHARED: SharedQueryState = { filters: [], rowLimit: 1000, temporalUnits: {}, orderby: [] };

/**
 * The fields a sort may name, derived from the SPEC THE DESCRIPTOR BUILDS rather than from the
 * dataset's column list — `orderby_sql` (crates/egret-bi/src/chartquery.rs) resolves a field as a
 * metric, then as a dimension **of this spec**, then as a raw column, and answers anything else
 * with a 400. Offering the dataset's columns would let a user pick one the query rejects.
 *
 * Aggregate and raw-records specs take different arms of that function and are kept apart here:
 *
 * - **Raw records** (`metrics` empty): the spec's `columns`, resolved by the third arm.
 * - **Aggregate**: its `dimensions` and `metrics`, and NOT its `columns`. `chartquery::build`'s
 *   aggregate branch never reads `columns`, so a raw column would resolve through `find_column`
 *   into an `ORDER BY` that appears in no `GROUP BY` — an engine error rather than a clean 400.
 *   No descriptor emits both today (the table's `buildSpec` blanks `columns` when it aggregates),
 *   so this is a guard on the derivation, not a live bug.
 * - **An aggregate with no breakout** is one row. Nothing to order, so nothing is offered and the
 *   control does not render — an inert control is worse than an absent one.
 */
export function sortableFields(spec: QuerySpec): string[] {
  const fields = spec.metrics.length > 0
    ? (spec.dimensions.length === 0 ? [] : [...spec.dimensions, ...spec.metrics])
    : spec.columns;
  return [...new Set(fields)];
}

/**
 * Whether this descriptor honours an authored sort at all — asked OF THE DESCRIPTOR, by building a
 * spec with a sentinel clause and looking for it, rather than kept as a list of viz keys here that
 * a new descriptor would have to remember to join.
 *
 * Smartscalar is the one that answers no, and deliberately: `SmartScalarChart` reads `valid[n - 1]`
 * and `valid[n - 2]` as "this period" and "the one before", so its dimension order is a
 * precondition of the arithmetic rather than a default an author may override. Under an authored
 * `revenue DESC` a trend would still draw a number and a "vs. previous month" caption, both
 * computed from two rows that are neither, with nothing on screen to say so. So the control is not
 * offered there — an absent control beats a silently wrong number.
 */
export function honoursOrderby(descriptor: VizDescriptor<never>, config: never): boolean {
  const probed = descriptor.buildSpec(config, { ...NO_SHARED, orderby: [{ field: PROBE, desc: false }] });
  return probed.orderby.some((o) => o.field === PROBE);
}

/**
 * Recovers the sort the chart's AUTHOR asked for from a stored spec, which is not the same thing as
 * the spec's `orderby`.
 *
 * A stored `orderby` is the sum of two clauses: what the author wrote, and what the descriptor
 * derived (the cartesian family's ascending-by-breakout, from PR #394). Metabase never has to tell
 * them apart — MBQL stores only the author's `order-by` and its query processor adds the implicit
 * breakout clause at run time, every run, discarding it again. egret stores the built spec, so both
 * arrive together and hydrating them wholesale into the editor's shared state would make the
 * derived one editable, permanent, and eventually WRONG:
 *
 *   a new bar chart broken out by `category` saves `orderby: [category ASC]` with nothing authored;
 *   reopen it, change the x-axis to `state`, and the spec becomes `[category ASC, state ASC]` —
 *   `category` is no longer a dimension, and `orderby_sql` answers 400 `unknown order-by field`.
 *
 * So the derived clauses are subtracted back out: build the spec the descriptor would emit with NO
 * authored sort, and drop exactly those clauses. What is left is what the author added. The
 * round-trip is stable — save re-derives them, load strips them again — and a sort that only looks
 * derived (`category ASC` written by hand) is dropped and re-derived identically, which is a
 * difference nothing can observe.
 */
export function authoredOrderby(
  params: AnyChartParams,
  descriptor: VizDescriptor<never> | undefined,
): ChartOrderBy[] {
  const stored = params.spec?.orderby ?? [];
  if (!descriptor || stored.length === 0) return stored;
  const derived = descriptor.buildSpec(descriptor.fromParams(params), {
    filters: params.spec?.filters ?? [],
    rowLimit: params.spec?.row_limit ?? 1000,
    temporalUnits: params.spec?.temporal_units ?? {},
    orderby: [],
  }).orderby;
  return stored.filter((o) => !derived.some((d) => same(d, o)));
}
