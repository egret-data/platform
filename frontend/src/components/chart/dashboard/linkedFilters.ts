import type { DashboardParameter } from "@/api/dashboards";

export type LinkedFilterState =
  | { kind: "list"; usable: DashboardParameter[] }
  | { kind: "no-usable" }
  | { kind: "custom-source" }
  | { kind: "input-widget" };

/** Metabase's `usableAsLinkedFilter` (parameters/utils/linked-filters.ts) admits every
 *  FIELD_FILTER_PARAMETER_TYPES entry — date, string, number, boolean, id, category, location.
 *  egret's set is the same minus `temporal_unit`, which has no counterpart there: it re-grids a
 *  column rather than holding a value, so it can never narrow another filter's option list. */
function usableAsLinkedFilter(param: DashboardParameter): boolean {
  return param.type !== "temporal_unit";
}

/**
 * Which of the four things the linked-filters section should show, in Metabase's order
 * (`ParameterLinkedFilters.tsx:92-103`). The order is load-bearing: a parameter can be both
 * custom-sourced and an input box, and Metabase names the source first.
 */
export function linkedFilterState(
  param: DashboardParameter,
  others: DashboardParameter[],
): LinkedFilterState {
  const usable = others.filter(usableAsLinkedFilter);
  if (usable.length === 0) return { kind: "no-usable" };
  const source = param.valuesSource?.type;
  if (source === "static" || source === "card") return { kind: "custom-source" };
  if ((param.widget ?? "dropdown") === "input") return { kind: "input-widget" };
  return { kind: "list", usable };
}
