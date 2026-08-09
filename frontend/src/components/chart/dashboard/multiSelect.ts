import type { DashboardParameter, ParamOperator, ParamType } from "@/api/dashboards";

/**
 * Which (type, operator) pairs let the author choose between "Multiple values" and "A single
 * value". Ported verbatim from Metabase's SINGLE_OR_MULTI_SELECTABLE_TYPES
 * (metabase-lib/v1/parameters/constants.ts:243-259), which keys on its own sectionId and subtype.
 * The two axes are egret's ParamType and ParamOperator under different spellings:
 *
 *   Metabase "string"              → egret "text",     "=" / "!=" → "is" / "is-not"
 *   Metabase "category" / "id"     → "any": every operator qualifies
 *   Metabase "location" ["=","!="] → egret ["is", "is-not"]
 *   Metabase "number"   ["=","!="] → identical spelling here
 *
 * `date`, `boolean` and `temporal_unit` are absent from Metabase's table and stay absent: they are
 * never single-or-multi selectable.
 */
export const SINGLE_OR_MULTI_SELECTABLE: Partial<Record<ParamType, "any" | ParamOperator[]>> = {
  text: ["is", "is-not", "contains", "does-not-contain", "starts-with", "ends-with"],
  category: "any",
  id: "any",
  location: ["is", "is-not"],
  number: ["=", "!="],
};

/** Metabase's `isSingleOrMultiSelectable` (parameters/utils/parameter-type.ts), unchanged. */
export function isSingleOrMultiSelectable(param: DashboardParameter): boolean {
  const entry = SINGLE_OR_MULTI_SELECTABLE[param.type];
  if (!entry) return false;
  if (entry === "any") return true;
  return entry.includes(param.operator);
}

/**
 * Whether egret has a widget that can actually honour the choice — and so whether the radio group
 * is rendered at all.
 *
 * `text` and `number` are in the ported table above, and they are DELIBERATELY excluded here:
 * egret's text and number filters are single-value `<input>`s (`FilterPill.tsx`), so a "Multiple
 * values" radio beside them would set a flag nothing reads. Metabase can offer it because its
 * widget is a token input; adding one here is a separate piece of work. Do not "fix" the gap by
 * deleting this predicate — that produces a control that lies.
 *
 * The input-box widget is excluded for the same reason: it is a free-text box either way.
 */
export function canRenderMultiValue(param: DashboardParameter): boolean {
  const listType = param.type === "category" || param.type === "location" || param.type === "id";
  return listType && (param.widget ?? "dropdown") !== "input";
}

/** Per-type default when the author has not chosen.
 *
 *  Metabase's `getIsMultiSelect` (parameter-values.js:178-180) defaults everything to multi. egret
 *  cannot: an `id` filter has always written a scalar and `paramToFilters`'s id case has always
 *  emitted `eq`, so a blanket multi default would change how every saved dashboard's id filter
 *  behaves the moment it loads. Multi for id is opt-in per parameter. */
const DEFAULT_MULTI_VALUE: Partial<Record<ParamType, boolean>> = {
  category: true,
  location: true,
  id: false,
};

export function isMultiValue(param: DashboardParameter): boolean {
  return param.isMultiSelect ?? DEFAULT_MULTI_VALUE[param.type] ?? false;
}
