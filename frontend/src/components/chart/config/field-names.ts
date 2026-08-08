/**
 * How a FIELD IS NAMED, FOUND AND EXPLAINED wherever a chart offers one to pick.
 *
 * WHY THIS EXISTS. egret picks fields through two widgets that sit one screen apart on the same
 * Data tab, and Metabase draws both shapes deliberately — a checkbox list for the query's
 * projection (`FieldPicker.tsx`), drag zones for the pivot's ordered rows/columns/values. #441
 * measured that and kept both, which was right: a table's columns are a SET chosen out of many
 * where the common act is "not that one", a pivot's zones are three short ORDERED lists where the
 * common act is "this one, here".
 *
 * What was NOT right is that the two disagreed about what a field is CALLED. `DatasetColumn` has
 * carried `verbose_name` for a long time and the pivot's zones have read it since #438; the table's
 * list read `column_name` and nothing else. Driven on the kind deployment against the seeded
 * `Demo Sales`, whose `region` is named "Khu vực": the pivot's "Add rows…" popover offered
 * `Khu vực`, and the table's COLUMNS list, one screen up, offered `region`. Two lists on one tab
 * naming the same column two ways is the same class of defect #410 spent a PR closing between the
 * Data and Display tabs — so the naming rule lives in ONE place now rather than in whichever
 * component happened to implement it.
 *
 * The threshold is here for the same reason. #441 gave the table's list a filter past eight fields
 * and spent a section of its own PR body deleting a class constant that had been declared
 * byte-for-byte in five files; re-declaring this one in `FieldZone` would have re-made that bug in
 * the same week.
 *
 * IDENTITY IS ALWAYS THE MACHINE NAME. Everything these helpers touch is what a row SAYS. What a
 * row STORES — `table.columns`, `pivot.rows`, a spec's projection — keys on `column_name`, so
 * renaming a column in the dataset never rewrites a chart's configuration. That is the rule #410
 * pinned for the Display tab's merged rows and it holds identically here.
 */

/** Machine name -> display name, as a dataset's `verbose_name` supplies it. */
export type FieldLabels = Record<string, string>;

/**
 * Above this many options a list stops being scannable by eye and earns a filter box. Metabase's
 * own picker has none below it, and neither do we — under the threshold the filter is more chrome
 * than help.
 */
export const FIELD_FILTER_THRESHOLD = 8;

/**
 * What the field is CALLED on screen. A blank or whitespace-only `verbose_name` is treated as
 * absent rather than drawn, or a column named `"   "` would render as an unclickable-looking gap.
 */
export function displayName(name: string, labels?: FieldLabels): string {
  return labels?.[name]?.trim() || name;
}

/**
 * The `title` a renamed field carries: the friendly name with the real column beside it. The field
 * is chosen by what it MEANS, but a SQL-facing user still needs the name the query will use, and a
 * row that shows only "Khu vực" hides it completely. Unnamed fields get their own name rather than
 * the redundant `region (region)`.
 */
export function fieldTitle(name: string, labels?: FieldLabels): string {
  const shown = displayName(name, labels);
  return shown === name ? name : `${shown} (${name})`;
}

/**
 * Whether a field survives a filter query — matched against BOTH names, deliberately.
 *
 * Filtering only the displayed name would make a renamed column unreachable by the name it actually
 * has, in the very list that decides what the query projects: someone who knows the table types
 * `region`, and "Khu vực" would vanish. Matching only the machine name would break the opposite
 * user just as badly. The query is trimmed and case-folded because neither name is typed carefully.
 */
export function matchesField(name: string, query: string, labels?: FieldLabels): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return name.toLowerCase().includes(q) || displayName(name, labels).toLowerCase().includes(q);
}

/** Build the label map from a dataset's columns. Absent `verbose_name` simply contributes nothing,
 *  so `displayName` falls back to the machine name without needing a second branch here. */
export function columnLabels(columns: { column_name: string; verbose_name?: string }[]): FieldLabels {
  const out: FieldLabels = {};
  for (const c of columns) if (c.verbose_name) out[c.column_name] = c.verbose_name;
  return out;
}

/** The same, for a dataset's metrics. */
export function metricLabels(metrics: { metric_name: string; verbose_name?: string }[]): FieldLabels {
  const out: FieldLabels = {};
  for (const m of metrics) if (m.verbose_name) out[m.metric_name] = m.verbose_name;
  return out;
}
