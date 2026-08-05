import { describe, it, expect, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ChartTable } from "../ChartTable";
import { PivotTable } from "../pivot/PivotTable";
import type { PivotInput } from "../pivot/engine";

/**
 * THE TABLE HEADERS ARE REACHABLE FROM THE KEYBOARD, and every assertion here was written to fail
 * against the code that shipped before it.
 *
 * The audit measured three separate holes, all in header controls that advertise themselves to
 * assistive tech and could not be operated:
 *
 *  1. `ChartTable`'s resize separator carried `role="separator"` and `aria-label="Resize column"`
 *     with `onMouseDown`/`onTouchStart` only — `tabindex: null`, no key handler, and no numeric
 *     width control anywhere in the panel. The pivot had had the answer since chart 03.
 *  2. `ChartTable`'s `<th>` announced itself `sortable` (dnd-kit's word for drag-to-reorder) and
 *     answered Enter with a drag lift. Sorting was wired to `onClick` alone, so it had NO keyboard
 *     route at all; the direction was carried only by a `" ▲"` glyph, invisible to a screen reader.
 *  3. The pivot's row-dimension `<th>` was a bare `<th onClick>` — no `tabIndex`, so it could not
 *     even be focused, and Enter fired the handler zero times.
 *
 * WHY EVERY CASE TABS RATHER THAN CALLING `.focus()`: reachability IS the defect. A `.focus()` call
 * plus a synthetic keydown proves the handler exists, not that a keyboard user can ever get to it —
 * and (2) and (3) both had a working mouse handler sitting on an element no Tab would ever land on.
 */

const COLS = [{ name: "region", type: "varchar" }, { name: "revenue", type: "double" }];
const ROWS = [["EU", 10], ["US", 20]];

/** Tab until `el` has focus, or fail saying how far we got. Deliberately not `.focus()`. */
async function tabTo(user: ReturnType<typeof userEvent.setup>, el: HTMLElement, max = 20) {
  for (let i = 0; i < max; i++) {
    if (document.activeElement === el) return;
    await user.tab();
  }
  expect(document.activeElement, `never reached the control in ${max} tabs`).toBe(el);
}

describe("ChartTable: the resize separator is operable from the keyboard", () => {
  it("grows the column by one step on ArrowRight and persists it", async () => {
    const user = userEvent.setup();
    const onTableStateChange = vi.fn();
    render(
      <ChartTable
        columns={COLS}
        rows={ROWS}
        tableState={{ columnWidths: { region: 200 } }}
        onTableStateChange={onTableStateChange}
      />,
    );
    const th = screen.getByRole("columnheader", { name: /region/ });
    const handle = within(th).getByRole("separator", { name: "Resize region" });
    await tabTo(user, handle);
    await user.keyboard("{ArrowRight}");
    // The whole table state comes back, not just the widths — `toStrictEqual` because a patch that
    // fails to carry a key is indistinguishable from one that clears it under `toEqual`.
    expect(onTableStateChange).toHaveBeenCalledTimes(1);
    // ONLY the column that was resized — `revenue` was never sized, and inventing a width for it
    // here would freeze an auto-sized column at whatever jsdom's zero-rect happened to produce.
    // `toStrictEqual` because a patch that drops a key and one that never carried it are the same
    // object under `toEqual`.
    expect(onTableStateChange.mock.calls[0][0]).toStrictEqual({ columnWidths: { region: 216 } });
  });

  it("shrinks on ArrowLeft and clamps at the minimum", async () => {
    const user = userEvent.setup();
    const onTableStateChange = vi.fn();
    render(
      <ChartTable
        columns={COLS}
        rows={ROWS}
        tableState={{ columnWidths: { region: 64 } }}
        onTableStateChange={onTableStateChange}
      />,
    );
    const handle = within(screen.getByRole("columnheader", { name: /region/ }))
      .getByRole("separator", { name: "Resize region" });
    await tabTo(user, handle);
    await user.keyboard("{ArrowLeft}");
    expect(onTableStateChange.mock.calls[0][0].columnWidths.region).toBe(60);
  });

  it("names each separator for its own column", () => {
    // One `aria-label="Resize column"` on every handle means a screen-reader user hears N
    // identical controls with nothing to tell them apart. The pivot has taken a `label` since
    // chart 03; this table was left behind.
    render(<ChartTable columns={COLS} rows={ROWS} onTableStateChange={vi.fn()} />);
    expect(screen.getByRole("separator", { name: "Resize region" })).toBeInTheDocument();
    expect(screen.getByRole("separator", { name: "Resize revenue" })).toBeInTheDocument();
    expect(screen.queryByRole("separator", { name: "Resize column" })).toBeNull();
  });

  it("does not let a drag on the separator cycle the sort", async () => {
    // The guarantee the old handler's `onPointerDown`/`onClick` stopPropagation existed for: a
    // header click cycles the sort and a drag must not. Whichever handler set survives the
    // extraction, this has to survive with it.
    const user = userEvent.setup();
    render(<ChartTable columns={COLS} rows={ROWS} onTableStateChange={vi.fn()} />);
    const th = screen.getByRole("columnheader", { name: /region/ });
    await user.click(within(th).getByRole("separator", { name: "Resize region" }));
    expect(th).toHaveAttribute("aria-sort", "none");
  });
});

describe("both new controls carry a focus indicator that does not depend on --ring", () => {
  // WHAT THIS DOES AND DOES NOT PROVE, said plainly: jsdom runs no layout and computes no colour,
  // so nothing here measures contrast. The 3:1 numbers are in `TableHeaderControls.tsx`, from a
  // numeric OKLCH->sRGB conversion. This pins only that the class survives — which is what a
  // refactor would silently drop, leaving both controls on the global `outline-ring/50` fallback
  // that measures 1.54:1 in light mode.
  it("names --primary rather than falling through to the global outline-ring/50", () => {
    render(<ChartTable columns={COLS} rows={ROWS} onTableStateChange={vi.fn()} />);
    const th = screen.getByRole("columnheader", { name: /region/ });
    for (const el of [
      within(th).getByRole("button", { name: /^Sort by region$/ }),
      within(th).getByRole("separator", { name: "Resize region" }),
    ]) {
      expect(el.className).toContain("focus-visible:outline-primary");
      expect(el.className).not.toContain("ring-ring");
    }
  });
});

describe("ChartTable: sorting has a keyboard route and a spoken direction", () => {
  it("sorts on Enter and reports the direction with aria-sort", async () => {
    const user = userEvent.setup();
    render(<ChartTable columns={COLS} rows={ROWS} />);
    const th = screen.getByRole("columnheader", { name: /region/ });
    expect(th).toHaveAttribute("aria-sort", "none");
    const sort = within(th).getByRole("button", { name: /^Sort by region$/ });
    await tabTo(user, sort);
    await user.keyboard("{Enter}");
    expect(th).toHaveAttribute("aria-sort", "ascending");
    await user.keyboard("{Enter}");
    expect(th).toHaveAttribute("aria-sort", "descending");
  });

  it("is named for its column and not for the controls inside it", () => {
    // The cell's contents are now two aria-labelled controls, which changes what the cell itself is
    // called. MEASURED IN CHROME by deleting the attribute from one live header and re-reading the
    // accessibility tree: that columnheader came back with NO NAME, beside nine siblings still
    // named for their column — and a columnheader's name is announced for every cell under it.
    //
    // SAID PLAINLY: jsdom does not reproduce that. dom-accessibility-api answers "region" with or
    // without the attribute, so this assertion states the intent and the mutation that deletes the
    // attribute survives it. The browser measurement is the evidence; this is the reminder.
    render(<ChartTable columns={COLS} rows={ROWS} onTableStateChange={vi.fn()} />);
    // A STRING `name` IS ALREADY AN EXACT MATCH here — `ByRoleOptions` has no `exact` key, and
    // passing one is a type error rather than a silently ignored option. `/region/` would pass on
    // "Sort by region" too, which is the whole thing being ruled out.
    expect(screen.getByRole("columnheader", { name: "region" })).toBeInTheDocument();
    expect(screen.queryByRole("columnheader", { name: /Sort by/ })).toBeNull();
    expect(screen.queryByRole("columnheader", { name: /Resize/ })).toBeNull();
  });

  it("does not call the header 'sortable' when that word means drag-to-reorder", () => {
    // dnd-kit's own word, and the one the audit found on the cell: a keyboard user heard
    // "sortable", pressed Enter and got a drag. dnd-kit's screen-reader instructions on the same
    // context already say "draggable item", so this is the cell agreeing with them.
    render(<ChartTable columns={COLS} rows={ROWS} />);
    const th = screen.getByRole("columnheader", { name: /region/ });
    expect(th).toHaveAttribute("aria-roledescription", "draggable column");
  });

  it("keeps Enter on the HEADER itself for the drag, without sorting as well", async () => {
    // The two controls are distinct and both reachable; one keypress must still do one thing.
    // Driven live too: Enter on the `<th>` set `opacity: 0.6` and announced "Draggable item col_9
    // was moved over droppable area col_9" while `aria-sort` stayed put.
    const user = userEvent.setup();
    render(<ChartTable columns={COLS} rows={ROWS} />);
    const th = screen.getByRole("columnheader", { name: /region/ });
    await tabTo(user, th);
    await user.keyboard("{Enter}");
    expect(th).toHaveAttribute("aria-sort", "none");
  });

  it("still sorts on a click anywhere in the header, exactly once", async () => {
    // The header keeps its click; the button only adds a route. A button that let its click bubble
    // to the <th> would toggle twice and land back where it started.
    const user = userEvent.setup();
    render(<ChartTable columns={COLS} rows={ROWS} />);
    const th = screen.getByRole("columnheader", { name: /region/ });
    await user.click(within(th).getByRole("button", { name: /^Sort by region$/ }));
    expect(th).toHaveAttribute("aria-sort", "ascending");
  });
});

const pivotInput: PivotInput = {
  columns: ["region", "seg", "revenue", "__grp_0", "__grp_1"],
  rows: [
    ["EU", "SMB", 10, 0, 0], ["EU", null, 10, 0, 1],
    ["US", "SMB", 5, 0, 0], ["US", null, 5, 0, 1],
    [null, "SMB", 15, 1, 0], [null, null, 15, 1, 1],
  ],
  rowDims: ["region"], colDims: ["seg"], metrics: ["revenue"],
  showRowTotals: true, showColTotals: true, condense: true, collapsed: new Set(),
};

describe("PivotTable: the row-dimension header is reachable", () => {
  it("cycles the sort on Enter", async () => {
    const user = userEvent.setup();
    const onCycleSort = vi.fn();
    render(<PivotTable input={pivotInput} onCycleSort={onCycleSort} />);
    const sort = screen.getByRole("button", { name: /^Sort by region$/ });
    await tabTo(user, sort);
    await user.keyboard("{Enter}");
    expect(onCycleSort).toHaveBeenCalledTimes(1);
    expect(onCycleSort).toHaveBeenCalledWith("region");
  });

  it("is named for its dimension and not for the button inside it", () => {
    // Same reason, and the same jsdom caveat, as the ChartTable case above; pinned separately
    // because the two cells are built by different components.
    render(<PivotTable input={pivotInput} onCycleSort={vi.fn()} />);
    // A STRING `name` IS ALREADY AN EXACT MATCH here — `ByRoleOptions` has no `exact` key, and
    // passing one is a type error rather than a silently ignored option. `/region/` would pass on
    // "Sort by region" too, which is the whole thing being ruled out.
    expect(screen.getByRole("columnheader", { name: "region" })).toBeInTheDocument();
    expect(screen.queryByRole("columnheader", { name: /Sort by/ })).toBeNull();
  });

  it("reports the direction with aria-sort rather than only a glyph", () => {
    render(
      <PivotTable
        input={{ ...pivotInput, sortDims: { region: "desc" } }}
        onCycleSort={vi.fn()}
      />,
    );
    expect(screen.getByRole("columnheader", { name: /region/ })).toHaveAttribute("aria-sort", "descending");
  });

  it("leaves the header inert where no sort handler was passed", () => {
    // A dashboard card renders this read-only. A control that announces itself and does nothing is
    // the defect this whole file is about, so the button must not appear at all.
    render(<PivotTable input={pivotInput} />);
    expect(screen.queryByRole("button", { name: /^Sort by region$/ })).toBeNull();
  });
});
