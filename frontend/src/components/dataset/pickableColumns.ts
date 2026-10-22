import type { DatasetColumn } from "@/api/datasets";

/** Every string anywhere in `v` (the names a viz config or a spec refers to). */
function collectStrings(v: unknown, out: Set<string>): Set<string> {
  if (typeof v === "string") out.add(v);
  else if (Array.isArray(v)) v.forEach((x) => collectStrings(x, out));
  else if (v && typeof v === "object") Object.values(v).forEach((x) => collectStrings(x, out));
  return out;
}

/**
 * The dataset columns the chart builder's field pickers offer (KI-096): those with **Dimension**
 * on. The switch used to be stored and read by nothing, so a column the dataset's author marked
 * "not a dimension" was still offered as an X axis.
 *
 * Enforced HERE, in the pickers, and not by refusing the chart in the backend: a saved chart that
 * already groups by such a column must keep rendering. For the same reason a column the chart
 * already uses (anywhere in `inUse`, its config or spec) stays offered, so the picker can still
 * show the chart's own pick.
 *
 * A records view lists columns rather than grouping by them — the table with no metric, and the
 * object detail — so it keeps every column, as Superset's raw-records mode does.
 */
export function pickableColumns(columns: DatasetColumn[], opts: { recordsView: boolean; inUse: unknown[] }): DatasetColumn[] {
  if (opts.recordsView) return columns;
  const used = new Set<string>();
  opts.inUse.forEach((x) => collectStrings(x, used));
  return columns.filter((c) => c.groupby !== false || used.has(c.column_name));
}

/**
 * The columns a dashboard filter can be mapped to on one card (KI-096): those whose **Filter**
 * switch is on in the card's dataset. `filterOff` is the set of column names with the switch off;
 * `current` is the column the filter is mapped to now, which stays offered so an existing mapping
 * still shows (and keeps working).
 */
export function filterableColumns(columns: string[], filterOff: ReadonlySet<string> | undefined, current?: string | null): string[] {
  if (!filterOff || filterOff.size === 0) return columns;
  return columns.filter((c) => !filterOff.has(c) || c === current);
}
