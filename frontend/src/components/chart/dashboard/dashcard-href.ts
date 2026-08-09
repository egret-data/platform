import type { DashboardCard } from "@/api/dashboards";

/**
 * The chart a dashboard card was built from, as a route — or `undefined` when the card must not
 * offer one. This is Metabase's `getDashcardHref` (`dashboard/selectors.ts:228-248`), which returns
 * `undefined` for anything that is not a question card.
 *
 * THE ROUTE SHAPE IS READ FROM `App.tsx:94`, not assumed. `/explore/:workspace/charts/:id`.
 *
 * THREE SUPPRESSIONS:
 *  - while EDITING, because the card body is already a click target that opens the card settings
 *    panel, and a link nested inside it would navigate out of an unsaved draft;
 *  - when the title override is exactly `""`, which is how a reader hides the header — the link and
 *    the "Edit question" affordance both hang off the header, so hiding it hides them. `undefined`
 *    (no override at all) and any non-empty override keep them;
 *  - for the virtual card kinds and for a chart card whose `chart_id` is null, which have no chart
 *    to open.
 */
export function dashcardHref(
  ws: string,
  card: Pick<DashboardCard, "card_type" | "chart_id" | "options">,
  editing: boolean | undefined,
): string | undefined {
  if (editing) return undefined;
  if (card.card_type !== "chart" || !card.chart_id) return undefined;
  if (card.options.title === "") return undefined;
  return `/explore/${ws}/charts/${card.chart_id}`;
}
