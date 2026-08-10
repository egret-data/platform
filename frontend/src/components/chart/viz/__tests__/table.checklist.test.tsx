import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, within, cleanup } from "@testing-library/react";
import { createElement } from "react";
import { objectDescriptor, tableDescriptor, type ObjectConfig, type TableConfig } from "../table";

const d = tableDescriptor();
import type { VizControlProps } from "../types";
import { CheckList } from "@/components/chart/config/CheckList";

const COLS = ["id", "created_at", "title", "category", "vendor", "subtotal", "discount", "total", "quantity", "state"];

/**
 * MOUNTED ON `CheckList` DIRECTLY, not through the table's Data-tab control, because that control
 * no longer exists: it listed the same fields, writing the same `config.columns`, as the Display
 * tab's `Add or remove columns` fold, and was removed as the duplicate. `CheckList` is unchanged —
 * it moved to `config/CheckList.tsx` so the fold could use the component instead of a copy of its
 * markup — so every assertion below still describes the widget it always described.
 *
 * The `{ columns: … }` shape the callbacks emit is what the table's control used to build, kept
 * here so the assertions read the same.
 */
function renderColumns(config: { columns?: string[] } = {}, onChange = vi.fn()) {
  const selected = config.columns ?? [];
  render(createElement(CheckList, {
    idPrefix: "col",
    empty: "Select a dataset to see columns.",
    options: COLS,
    selected,
    onToggle: (name: string) => onChange({
      columns: selected.includes(name) ? selected.filter((x) => x !== name) : [...selected, name],
    }),
    onSetAll: (next: string[]) => onChange({ columns: next }),
  }));
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

  /**
   * THE SAME FIELD, NAMED TWO WAYS, ONE SCREEN APART. `VizControlProps.columns` is
   * `DatasetColumn[]` and has always carried `verbose_name`; this list read `column_name` and
   * nothing else, while the pivot's zones one screen down have read `verbose_name` since #438.
   * Measured on the kind deployment against the seeded `Demo Sales`, whose `region` column is named
   * "Khu vực": the pivot's "Add rows…" popover offered `Khu vực`, the table's Data-tab COLUMNS list
   * offered `region`. That divergence — not the widget's shape — is what is left of "the table's
   * list looks ugly next to the pivot's field zones" after #441.
   */
  it("shows a column's verbose name, the way the pivot's zones do", () => {
    render(createElement(CheckList, {
      idPrefix: "col", empty: "Select a dataset to see columns.",
      options: ([{ column_name: "region", type: "text", verbose_name: "Khu vực" },
        { column_name: "category", type: "text" }]).map((c) => c.column_name),
      labels: Object.fromEntries(([{ column_name: "region", type: "text", verbose_name: "Khu vực" },
        { column_name: "category", type: "text" }]).filter((c) => "verbose_name" in c).map((c) => [c.column_name, (c as { verbose_name: string }).verbose_name])),
      selected: [],
      onToggle: vi.fn(), onSetAll: vi.fn(),
      }));
    expect(screen.getByText("Khu vực")).toBeInTheDocument();
    expect(screen.queryByText("region")).not.toBeInTheDocument();
    // Unnamed columns are untouched — the fallback is the machine name, as everywhere else.
    expect(screen.getByText("category")).toBeInTheDocument();
  });

  /**
   * IDENTITY STAYS RAW. The row may SAY "Khu vực" but what it toggles is `region` — the same rule
   * #410 pinned for the Display tab's merged rows, and the one that keeps a rename from rewriting
   * a chart's stored projection.
   */
  it("still toggles the raw column name behind a renamed row", () => {
    const onChange = vi.fn();
    render(createElement(CheckList, {
      idPrefix: "col", empty: "Select a dataset to see columns.",
      options: ["region"],
      labels: { region: "Khu vực" },
      selected: [],
      onToggle: (name: string) => onChange({ columns: [name] }),
      onSetAll: (next: string[]) => onChange({ columns: next }),
    }));
    fireEvent.click(screen.getByText("Khu vực"));
    expect(onChange).toHaveBeenCalledWith({ columns: ["region"] });
  });

  /**
   * The real column stays REACHABLE, not just stored. A row that reads only "Khu vực" hides the
   * name the query will use; `FieldZone`'s chip has carried it in the `title` since #438 and this
   * row now says it the same way. An unrenamed column gets its own name, not `region (region)`.
   */
  it("keeps the machine name on hover behind the displayed one", () => {
    render(createElement(CheckList, {
      idPrefix: "col", empty: "Select a dataset to see columns.",
      options: ([{ column_name: "region", type: "text", verbose_name: "Khu vực" },
        { column_name: "category", type: "text" }]).map((c) => c.column_name),
      labels: Object.fromEntries(([{ column_name: "region", type: "text", verbose_name: "Khu vực" },
        { column_name: "category", type: "text" }]).filter((c) => "verbose_name" in c).map((c) => [c.column_name, (c as { verbose_name: string }).verbose_name])),
      selected: [],
      onToggle: vi.fn(), onSetAll: vi.fn(),
      }));
    expect(screen.getByText("Khu vực")).toHaveAttribute("title", "Khu vực (region)");
    expect(screen.getByText("category")).toHaveAttribute("title", "category");
  });

  /** The METRICS list above it is the same widget over the same kind of names, and a dataset may
   *  rename a metric exactly as it renames a column. It was the second call site, not a second
   *  decision. */
  it("shows a metric's verbose name too", () => {
    const metricsControl = d.controls.find((c) => c.id === "metrics")!;
    render(createElement(metricsControl.render, {
      // SELECTED, because Metrics is a `FieldZone` now: an unpicked field lives in the popover,
      // and what this asserts is that a chip says the dataset's name for it.
      config: { columns: [], metrics: ["total_revenue"] },
      onChange: vi.fn(),
      columns: [],
      metrics: [{ metric_name: "total_revenue", verbose_name: "Tổng doanh thu" }],
      resultColumns: [], rows: [],
      temporalUnits: {}, columnSettings: {}, unitRow: () => null,
    } as unknown as VizControlProps<TableConfig>));
    expect(screen.getByText("Tổng doanh thu")).toBeInTheDocument();
    expect(screen.queryByText("total_revenue")).not.toBeInTheDocument();
  });

  /**
   * A SQL-facing user types the column they know. Filtering only the displayed name would make a
   * renamed column unreachable by its real name in the very list that decides the projection.
   */
  it("filters on the machine name as well as the displayed one", () => {
    render(createElement(CheckList, {
      idPrefix: "col", empty: "Select a dataset to see columns.",
      options: ([...COLS.map((column_name) => ({ column_name, type: "text" })),
        { column_name: "region", type: "text", verbose_name: "Khu vực" }]).map((c) => c.column_name),
      labels: Object.fromEntries(([...COLS.map((column_name) => ({ column_name, type: "text" })),
        { column_name: "region", type: "text", verbose_name: "Khu vực" }]).filter((c) => "verbose_name" in c).map((c) => [c.column_name, (c as { verbose_name: string }).verbose_name])),
      selected: [],
      onToggle: vi.fn(), onSetAll: vi.fn(),
      }));
    fireEvent.change(screen.getByLabelText("Filter fields"), { target: { value: "region" } });
    expect(screen.getByText("Khu vực")).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText("Filter fields"), { target: { value: "khu" } });
    expect(screen.getByText("Khu vực")).toBeInTheDocument();
  });

  /** Object detail picks from the same dataset columns through the same widget. Pinned separately
   *  because it is its own call site: the table's list could be named correctly while this one was
   *  left reading raw, which is precisely how the divergence with the pivot arose in the first
   *  place — one component learning something the one beside it did not. */
  it("names object-detail fields the same way", () => {
    const objControl = objectDescriptor().controls.find((c) => c.id === "fields")!;
    render(createElement(objControl.render, {
      config: { columns: [] },
      onChange: vi.fn(),
      columns: [{ column_name: "region", type: "text", verbose_name: "Khu vực" }],
      metrics: [], resultColumns: [], rows: [],
      temporalUnits: {}, columnSettings: {}, unitRow: () => null,
    } as unknown as VizControlProps<ObjectConfig>));
    expect(screen.getByText("Khu vực")).toBeInTheDocument();
  });

  // Under the threshold the filter would be more chrome than help — Metabase's own picker has none.
  it("offers no filter on a short list", () => {
    render(createElement(CheckList, {
      idPrefix: "col", empty: "Select a dataset to see columns.",
      options: ([{ column_name: "a", type: "text" }, { column_name: "b", type: "text" }]).map((c) => c.column_name),
      labels: Object.fromEntries(([{ column_name: "a", type: "text" }, { column_name: "b", type: "text" }]).filter((c) => "verbose_name" in c).map((c) => [c.column_name, (c as { verbose_name: string }).verbose_name])),
      selected: [],
      onToggle: vi.fn(), onSetAll: vi.fn(),
      }));
    expect(screen.queryByLabelText("Filter fields")).not.toBeInTheDocument();
    expect(screen.getByText("0 of 2 selected")).toBeInTheDocument();
  });
});
