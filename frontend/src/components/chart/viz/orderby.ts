import type { AnyChartParams, ChartOrderBy } from "@/api/charts";
import type { VizDescriptor } from "./types";

const same = (a: ChartOrderBy, b: ChartOrderBy) => a.field === b.field && a.desc === b.desc;

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
