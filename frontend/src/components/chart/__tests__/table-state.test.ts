import { describe, it, expect } from "vitest";
import { reconcileTableState } from "../table-state";
import type { TableState } from "@/api/charts";

/**
 * The pruning rules on their own. The SEQUENCE that motivates them — hide, remove from the query,
 * re-add — is driven through the real editor in
 * `routes/explore/__tests__/ChartExplore.tableColumns.test.tsx`, because a helper test cannot tell
 * a wired reconcile from an unwired one.
 */
describe("reconcileTableState", () => {
  const RESULT = ["id", "state", "vendor"];

  it("drops names the result no longer carries, from all three name-keyed lists", () => {
    const before: TableState = {
      hiddenColumns: ["vendor", "gone"],
      columnOrder: ["state", "gone", "id", "vendor"],
      columnWidths: { state: 120, gone: 80 },
      pinnedLeft: 2,
      rowIndex: true,
    };
    expect(reconcileTableState(before, RESULT)).toEqual({
      hiddenColumns: ["vendor"],
      columnOrder: ["state", "id", "vendor"],
      columnWidths: { state: 120 },
      pinnedLeft: 2,
      rowIndex: true,
    });
  });

  // The identity contract the caller depends on: `setTableState(prev => reconcile(prev, …))` runs
  // on every result, and returning a fresh object each time would re-render the whole editor on
  // every auto-run — and, worse, mark a clean chart dirty.
  it("returns the same object when there is nothing to prune", () => {
    const before: TableState = { hiddenColumns: ["vendor"], columnOrder: ["id", "state", "vendor"] };
    expect(reconcileTableState(before, RESULT)).toBe(before);
    expect(reconcileTableState({}, RESULT)).toEqual({});
  });

  // `pinnedLeft` is a COUNT and holds no stale name, so shrinking the result must not shrink it —
  // the freeze has to survive a column being taken out and put back. `ChartTable` clamps it for
  // rendering instead.
  it("leaves pinnedLeft alone even when the result shrinks below it", () => {
    expect(reconcileTableState({ pinnedLeft: 3 }, ["id"])).toEqual({ pinnedLeft: 3 });
  });

  // The backend asserts only that `params.settings` is a JSON object
  // (`crates/egret-bi/src/chartparams.rs`) and a test pins that permissiveness, so every one of
  // these shapes can be read back off a stored chart.
  it("tolerates stored state that is not the declared shape", () => {
    const junk = {
      hiddenColumns: ["vendor", 7, null],
      columnOrder: "id,state",
      columnWidths: 42,
    } as unknown as TableState;
    expect(reconcileTableState(junk, RESULT)).toEqual({
      hiddenColumns: ["vendor"],
      columnOrder: "id,state",
      columnWidths: 42,
    });
  });

  // RAW NAMES, NOT DISPLAY TITLES. `column_settings[name].column_title` renames a column's header
  // without renaming the column, so a reconcile that compared titles would decide every renamed
  // column had left the result and prune the user's whole layout on the next run. Six bugs in this
  // programme have come from keying on a display string; this is the assertion that would catch
  // the seventh here.
  it("compares raw column names, which a renamed column still has", () => {
    const before: TableState = { hiddenColumns: ["vendor"], columnWidths: { state: 90 } };
    // What the headers say is "Supplier" and "State (US)" — irrelevant to this function, which is
    // handed the result's own names.
    expect(reconcileTableState(before, RESULT)).toBe(before);
    // …and if it were handed titles instead, it would wipe both. Pinned as the failure mode.
    expect(reconcileTableState(before, ["Id", "State (US)", "Supplier"])).toEqual({
      hiddenColumns: [], columnWidths: {},
    });
  });
});
