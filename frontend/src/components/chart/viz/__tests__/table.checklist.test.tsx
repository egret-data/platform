import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, within, cleanup } from "@testing-library/react";
import { createElement } from "react";
import { tableDescriptor, type TableConfig } from "../table";
import type { VizControlProps } from "../types";

const d = tableDescriptor();
const columnsControl = d.controls.find((c) => c.id === "columns")!;

const COLS = ["id", "created_at", "title", "category", "vendor", "subtotal", "discount", "total", "quantity", "state"];

function renderColumns(config: Partial<TableConfig> = {}, onChange = vi.fn()) {
  render(createElement(columnsControl.render, {
    config: { columns: [], metrics: [], ...config },
    onChange,
    columns: COLS.map((column_name) => ({ column_name, type: "text" })),
    metrics: [], resultColumns: [], rows: [],
    temporalUnits: {}, columnSettings: {}, unitRow: () => null,
  } as unknown as VizControlProps<TableConfig>));
  return onChange;
}

/**
 * The table's Data-tab field list, which the pivot's drag zones sit one screen away from.
 *
 * The complaint was that the two look nothing alike. The answer is not to make them the same
 * widget — Metabase draws exactly these two shapes for exactly these two jobs — it is that this one
 * had none of a field picker's affordances: no count, no select-all, no filter, no bound on its
 * height, and a click target measured live at 16×16 (the checkbox) beside an 11×20 label, in a
 * 255px row of which 235px did nothing. WCAG 2.2 SC 2.5.8 asks for 24×24.
 *
 * jsdom cannot measure any of that. What it CAN hold is the markup those pixels come from — that
 * the row is a `<label>` wrapping the checkbox, so the whole row is the target rather than the
 * checkbox alone — and all of the behaviour.
 */
describe("the table's column list", () => {
  it("counts what is selected, which a scrolled list otherwise hides", () => {
    renderColumns({ columns: ["id", "total"] });
    expect(screen.getByText("2 of 10 selected")).toBeInTheDocument();
  });

  it("selects all and clears all through the one control, tri-state in between", () => {
    const onChange = renderColumns({ columns: ["id"] });
    const all = screen.getByLabelText("Select all fields");
    // Partial: neither checked nor unchecked, so the control states the list as well as acting on it.
    expect(all).toHaveAttribute("data-state", "indeterminate");
    fireEvent.click(all);
    expect(onChange).toHaveBeenCalledWith({ columns: COLS });
  });

  /**
   * The state has to be VISIBLE, not just in the DOM. Radix renders its Indicator for
   * `indeterminate` exactly as it does for `checked`, so before `ui/checkbox.tsx` grew a branch a
   * partial selection drew a tick — the control announcing "all ten selected" over one. The two
   * marks are told apart by their lucide class, which is the only thing about them jsdom can see.
   */
  it("draws a different mark for a partial selection than for a complete one", () => {
    const { unmount } = render(<div />);
    unmount();
    renderColumns({ columns: ["id"] });
    expect(screen.getByLabelText("Select all fields").querySelector(".lucide-minus")).not.toBeNull();
    cleanup();
    renderColumns({ columns: COLS });
    expect(screen.getByLabelText("Select all fields").querySelector(".lucide-check")).not.toBeNull();
  });

  it("clears the selection when everything is already on", () => {
    const onChange = renderColumns({ columns: COLS });
    const all = screen.getByLabelText("Select all fields");
    expect(all).toHaveAttribute("data-state", "checked");
    fireEvent.click(all);
    expect(onChange).toHaveBeenCalledWith({ columns: [] });
  });

  it("filters a long list down to what was typed", () => {
    renderColumns();
    fireEvent.change(screen.getByLabelText("Filter fields"), { target: { value: "tot" } });
    expect(screen.getByLabelText("subtotal")).toBeInTheDocument();
    expect(screen.getByLabelText("total")).toBeInTheDocument();
    expect(screen.queryByLabelText("vendor")).not.toBeInTheDocument();
  });

  /**
   * FILTERING IS A VIEW, NOT AN EDIT. A filter that also dropped the hidden columns from the
   * selection would silently delete a chart's configuration for typing in a search box, which is
   * the worst failure a "harmless" control can have. The count stays whole for the same reason.
   */
  it("narrowing the view never touches the selection", () => {
    const onChange = renderColumns({ columns: ["vendor"] });
    fireEvent.change(screen.getByLabelText("Filter fields"), { target: { value: "tot" } });
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByText("1 of 10 selected")).toBeInTheDocument();
  });

  it("says nothing matched rather than showing an empty list", () => {
    renderColumns();
    fireEvent.change(screen.getByLabelText("Filter fields"), { target: { value: "zzz" } });
    expect(screen.getByText(/No field matches/)).toBeInTheDocument();
  });

  // The whole row toggles, not the 16px box in it. `getByLabelText` resolving to the checkbox is
  // what proves the wrapping `<label>` is there: an unwrapped checkbox with a sibling label would
  // still resolve, so the row element itself is checked for being that label.
  it("makes the whole row the target, not the checkbox alone", () => {
    renderColumns();
    const box = screen.getByLabelText("vendor");
    const row = box.closest("label")!;
    expect(row).not.toBeNull();
    expect(within(row).getByLabelText("vendor")).toBe(box);
    expect(row.className).toContain("min-h-8");
  });

  it("still toggles one field at a time", () => {
    const onChange = renderColumns({ columns: ["id"] });
    fireEvent.click(screen.getByLabelText("vendor"));
    expect(onChange).toHaveBeenCalledWith({ columns: ["id", "vendor"] });
  });

  // Under the threshold the filter would be more chrome than help — Metabase's own picker has none.
  it("offers no filter on a short list", () => {
    render(createElement(columnsControl.render, {
      config: { columns: [], metrics: [] },
      onChange: vi.fn(),
      columns: [{ column_name: "a", type: "text" }, { column_name: "b", type: "text" }],
      metrics: [], resultColumns: [], rows: [],
      temporalUnits: {}, columnSettings: {}, unitRow: () => null,
    } as unknown as VizControlProps<TableConfig>));
    expect(screen.queryByLabelText("Filter fields")).not.toBeInTheDocument();
    expect(screen.getByText("0 of 2 selected")).toBeInTheDocument();
  });
});
