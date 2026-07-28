import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { TableColumnsSection } from "../TableColumnsSection";

const COLS = [{ name: "a", type: "varchar" }, { name: "b", type: "double" }];

const manyCols = (n: number) =>
  Array.from({ length: n }, (_, i) => ({ name: `col_${i}`, type: "varchar" }));

describe("TableColumnsSection", () => {
  it("renders a checked checkbox per visible column", () => {
    render(<TableColumnsSection columns={COLS} value={{}} onChange={() => {}} />);
    expect(screen.getByLabelText("a")).toBeChecked();
    expect(screen.getByLabelText("b")).toBeChecked();
  });

  it("emits hiddenColumns when a column is unchecked", () => {
    const onChange = vi.fn();
    render(<TableColumnsSection columns={COLS} value={{}} onChange={onChange} />);
    fireEvent.click(screen.getByLabelText("a"));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ hiddenColumns: ["a"] }));
  });

  it("emits pinnedLeft from the freeze input", () => {
    const onChange = vi.fn();
    render(<TableColumnsSection columns={COLS} value={{}} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText("Freeze first columns"), { target: { value: "1" } });
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ pinnedLeft: 1 }));
  });

  it("emits pagination enabled with default page size", () => {
    const onChange = vi.fn();
    render(<TableColumnsSection columns={COLS} value={{}} onChange={onChange} />);
    fireEvent.click(screen.getByLabelText("Paginate"));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ pagination: { enabled: true, pageSize: 25 } }));
  });

  // Part A: the filter box only earns its keep once the list is long enough that finding a
  // column by eye stops working.
  describe("column filter box", () => {
    it("appears with 40 columns", () => {
      render(<TableColumnsSection columns={manyCols(40)} value={{}} onChange={() => {}} />);
      expect(screen.getByLabelText(/search columns/i)).toBeInTheDocument();
    });

    it("does not appear with 5 columns", () => {
      render(<TableColumnsSection columns={manyCols(5)} value={{}} onChange={() => {}} />);
      expect(screen.queryByLabelText(/search columns/i)).not.toBeInTheDocument();
    });

    it("narrows the list to matching columns when typing", () => {
      const cols = [
        { name: "revenue", type: "double" },
        { name: "region", type: "varchar" },
        ...manyCols(15),
      ];
      render(<TableColumnsSection columns={cols} value={{}} onChange={() => {}} />);
      fireEvent.change(screen.getByLabelText(/search columns/i), { target: { value: "rev" } });
      expect(screen.getByLabelText("revenue")).toBeInTheDocument();
      expect(screen.queryByLabelText("region")).not.toBeInTheDocument();
      expect(screen.queryByLabelText("col_0")).not.toBeInTheDocument();
    });
  });

  // Part B: Show all / Hide all must act on every column, not the filtered subset — the trap
  // this task calls out explicitly.
  describe("Show all / Hide all", () => {
    it("Hide all emits every column name in hiddenColumns", () => {
      const onChange = vi.fn();
      render(<TableColumnsSection columns={COLS} value={{}} onChange={onChange} />);
      fireEvent.click(screen.getByRole("button", { name: "Hide all" }));
      expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ hiddenColumns: ["a", "b"] }));
    });

    it("Show all emits an empty hiddenColumns", () => {
      const onChange = vi.fn();
      render(
        <TableColumnsSection columns={COLS} value={{ hiddenColumns: ["a", "b"] }} onChange={onChange} />,
      );
      fireEvent.click(screen.getByRole("button", { name: "Show all" }));
      expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ hiddenColumns: [] }));
    });

    // Load-bearing (pinned): with a filter active, "Hide all" must still hide every column —
    // not just the ones the filter currently shows on screen. A filter is for FINDING a
    // column, not for scoping a bulk action.
    it("Hide all while a filter is active still hides every column, not just the matches", () => {
      const onChange = vi.fn();
      const cols = [
        { name: "revenue", type: "double" },
        { name: "region", type: "varchar" },
        ...manyCols(15),
      ];
      render(<TableColumnsSection columns={cols} value={{}} onChange={onChange} />);
      fireEvent.change(screen.getByLabelText(/search columns/i), { target: { value: "rev" } });
      fireEvent.click(screen.getByRole("button", { name: "Hide all" }));
      const [{ hiddenColumns }] = onChange.mock.calls[0];
      expect(hiddenColumns).toEqual(cols.map((c) => c.name));
    });

    it("toggling one column still works while a filter is active, and emits the raw name", () => {
      const onChange = vi.fn();
      const cols = [
        { name: "revenue", type: "double" },
        { name: "region", type: "varchar" },
        ...manyCols(15),
      ];
      render(<TableColumnsSection columns={cols} value={{}} onChange={onChange} />);
      fireEvent.change(screen.getByLabelText(/search columns/i), { target: { value: "rev" } });
      fireEvent.click(screen.getByLabelText("revenue"));
      expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ hiddenColumns: ["revenue"] }));
    });
  });

  // Part C: a renamed column shows its title, but stays findable by its raw name too, and
  // every non-display path (hiddenColumns, setAllHidden) still deals in raw names.
  describe("renamed columns (columnSettings)", () => {
    const cols = [
      { name: "total_revenue", type: "double" },
      { name: "region", type: "varchar" },
    ];
    const columnSettings = { total_revenue: { column_title: "Revenue" } };

    it("displays the column_title instead of the raw name", () => {
      render(
        <TableColumnsSection columns={cols} value={{}} onChange={() => {}} columnSettings={columnSettings} />,
      );
      expect(screen.getByLabelText("Revenue")).toBeInTheDocument();
      expect(screen.queryByLabelText("total_revenue")).not.toBeInTheDocument();
    });

    it("is findable by typing the new title", () => {
      render(
        <TableColumnsSection
          columns={[...cols, ...manyCols(12)]}
          value={{}}
          onChange={() => {}}
          columnSettings={columnSettings}
        />,
      );
      fireEvent.change(screen.getByLabelText(/search columns/i), { target: { value: "Revenue" } });
      expect(screen.getByLabelText("Revenue")).toBeInTheDocument();
    });

    it("is still findable by typing the original raw name", () => {
      render(
        <TableColumnsSection
          columns={[...cols, ...manyCols(12)]}
          value={{}}
          onChange={() => {}}
          columnSettings={columnSettings}
        />,
      );
      fireEvent.change(screen.getByLabelText(/search columns/i), { target: { value: "total_revenue" } });
      expect(screen.getByLabelText("Revenue")).toBeInTheDocument();
    });

    it("toggling the renamed column's checkbox emits its raw name", () => {
      const onChange = vi.fn();
      render(
        <TableColumnsSection columns={cols} value={{}} onChange={onChange} columnSettings={columnSettings} />,
      );
      fireEvent.click(screen.getByLabelText("Revenue"));
      expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ hiddenColumns: ["total_revenue"] }));
    });
  });
});
