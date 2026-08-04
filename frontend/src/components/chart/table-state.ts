import type { TableState } from "@/api/charts";

/**
 * Reconcile a table's stored display state against the columns the query actually returned.
 *
 * WHY THIS EXISTS. egret's table is configured by two lists that never spoke to each other:
 * `Data ▸ Columns` decides what the QUERY projects, `Display ▸ Visible columns` decides which of
 * the RESULT's columns are drawn. The second keys on column name and stores a NEGATIVE list —
 * `hiddenColumns` — so a name in it survives its column leaving the result. Hide `vendor`, untick
 * it in the Data tab, tick it again: the query projects it, the Data tab shows it on, and the table
 * refuses to draw it, with nothing anywhere saying the fix is one tab over. `columnOrder` and
 * `columnWidths` accumulate the same debris; they are only less visible because the render side
 * already ignores names it cannot resolve (`ChartTable.tsx`), which hides the leak rather than
 * fixing it.
 *
 * Pruning on the way OUT is the whole fix, and it has to be a write-back. Filtering these lists at
 * render time — the cheaper-looking option — cannot work: at the moment `vendor` re-enters the
 * result there is no longer anything on screen that distinguishes "hidden a second ago" from
 * "hidden before it ever left", so the decision has to be recorded when the column leaves. This is
 * what Metabase's `syncTableColumns` does (`querying/viz-settings/utils/sync-viz-settings.ts`),
 * and it is why a column removed and re-added there comes back ENABLED.
 *
 * NOT PRUNED: `pinnedLeft`. It is a COUNT, not a name, so it carries no stale reference to prune —
 * and clamping it down to a shrunken result would silently discard the user's freeze the moment a
 * column was taken out, with no way to tell that apart from a deliberate change. `ChartTable`
 * already clamps it for rendering, which is the right place for a bound that must not be
 * remembered.
 *
 * Every read here tolerates malformed input. The backend asserts only that `params.settings` is a
 * JSON object (`crates/egret-bi/src/chartparams.rs`) and a test pins that permissiveness, so a row
 * written by anything but this editor can put a number in `hiddenColumns` or a string in
 * `columnWidths` and it will reach this function.
 *
 * Returns `state` BY IDENTITY when there is nothing to prune, so a caller can pass it straight to
 * a `setState` updater and get React's bail-out instead of a re-render on every result.
 */
export function reconcileTableState(state: TableState, resultColumns: readonly string[]): TableState {
  const present = new Set(resultColumns);
  const next: TableState = { ...state };
  let changed = false;

  const pruneNames = (key: "hiddenColumns" | "columnOrder") => {
    const list = state[key];
    if (!Array.isArray(list)) return;
    const kept = list.filter((n) => typeof n === "string" && present.has(n));
    if (kept.length === list.length) return;
    next[key] = kept;
    changed = true;
  };
  pruneNames("hiddenColumns");
  pruneNames("columnOrder");

  const widths = state.columnWidths;
  if (widths && typeof widths === "object") {
    const kept = Object.keys(widths).filter((n) => present.has(n));
    if (kept.length !== Object.keys(widths).length) {
      next.columnWidths = Object.fromEntries(kept.map((n) => [n, widths[n]]));
      changed = true;
    }
  }

  return changed ? next : state;
}
