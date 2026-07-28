import { describe, it, expect } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { vi } from "vitest";
import { PivotTable } from "../PivotTable";
import type { PivotInput } from "../engine";

const input: PivotInput = {
  columns: ["region", "seg", "revenue", "__grp_0", "__grp_1"],
  rows: [
    ["EU", "SMB", 10, 0, 0], ["EU", "ENT", 20, 0, 0], ["EU", null, 30, 0, 1],
    ["US", "SMB", 5, 0, 0], ["US", null, 5, 0, 1],
    [null, "SMB", 15, 1, 0], [null, "ENT", 20, 1, 0], [null, null, 35, 1, 1],
  ],
  rowDims: ["region"], colDims: ["seg"], metrics: ["revenue"],
  showRowTotals: true, showColTotals: true, condense: true, collapsed: new Set(),
};

describe("PivotTable", () => {
  it("renders column headers, a leaf value, and the grand total", () => {
    render(<PivotTable input={input} />);
    expect(screen.getByText("SMB")).toBeInTheDocument();
    expect(screen.getByText("ENT")).toBeInTheDocument();
    expect(screen.getByText("Grand totals")).toBeInTheDocument();
    expect(screen.getByText("35")).toBeInTheDocument();
  });
  it("applies a column format to value cells", () => {
    render(<PivotTable input={input} metricTypes={{ revenue: "double" }} columnSettings={{ revenue: { prefix: "$" } }} />);
    expect(screen.getByText("$35")).toBeInTheDocument();
  });
});

const grouped = {
  columns: ["region", "seg", "revenue", "__grp_0", "__grp_1"],
  rows: [
    ["EU", "SMB", 10, 0, 0], ["EU", "ENT", 20, 0, 0], ["EU", null, 30, 0, 1],
    ["US", "SMB", 5, 0, 0], ["US", "ENT", 7, 0, 0], ["US", null, 12, 0, 1],
    [null, null, 42, 1, 1],
  ],
  rowDims: ["region", "seg"], colDims: [], metrics: ["revenue"],
  showRowTotals: false, showColTotals: true, condense: true, collapsed: new Set<string>(),
};

describe("PivotTable interactions", () => {
  it("renders a collapse toggle on a collapsible group and fires onToggleCollapse", () => {
    const onToggleCollapse = vi.fn();
    render(<PivotTable input={grouped} onToggleCollapse={onToggleCollapse} />);
    const btns = screen.getAllByRole("button", { name: "Collapse group" });
    fireEvent.click(btns[0]);
    expect(onToggleCollapse).toHaveBeenCalledWith('["EU"]');
  });

  it("fires onCycleSort when a row-dimension header is clicked", () => {
    const onCycleSort = vi.fn();
    render(<PivotTable input={grouped} onCycleSort={onCycleSort} />);
    fireEvent.click(screen.getByText("region"));
    expect(onCycleSort).toHaveBeenCalledWith("region");
  });
});

describe("PivotTable — column_title header rename", () => {
  it("shows column_title as the metric header instead of the raw name", () => {
    // metric sub-header only renders with >1 metric or no column dims; `grouped` has colDims: []
    // so its single-metric sub-header row is present and shows the raw metric name "revenue".
    render(<PivotTable input={grouped} columnSettings={{ revenue: { column_title: "Revenue" } }} />);
    expect(screen.getByText("Revenue")).toBeInTheDocument();
    expect(screen.queryByText("revenue")).not.toBeInTheDocument();
  });

  it("shows column_title as a row-dimension header instead of the raw name", () => {
    render(<PivotTable input={grouped} columnSettings={{ region: { column_title: "Region" } }} />);
    expect(screen.getByText("Region")).toBeInTheDocument();
    expect(screen.queryByText("region")).not.toBeInTheDocument();
  });

  it("still sorts by the raw row-dimension name when its header is renamed", () => {
    const onCycleSort = vi.fn();
    render(
      <PivotTable
        input={grouped}
        columnSettings={{ region: { column_title: "Region" } }}
        onCycleSort={onCycleSort}
      />,
    );
    fireEvent.click(screen.getByText("Region"));
    expect(onCycleSort).toHaveBeenCalledWith("region");
  });

  it("keeps the row-dimension and metric column_title lookups independent", () => {
    // Both a metric entry and a same-key-space dimension entry are set; each header must read
    // only its own key — a shared/merged lookup would leak one title into the other's header.
    render(
      <PivotTable
        input={grouped}
        columnSettings={{ revenue: { column_title: "Metric Title" }, region: { column_title: "Dim Title" } }}
      />,
    );
    expect(screen.getByText("Dim Title")).toBeInTheDocument();
    expect(screen.getByText("Metric Title")).toBeInTheDocument();
    expect(screen.queryByText("region")).not.toBeInTheDocument();
    expect(screen.queryByText("revenue")).not.toBeInTheDocument();
  });

  it("falls back to the raw name for unset columns", () => {
    render(<PivotTable input={grouped} columnSettings={{}} />);
    expect(screen.getByText("region")).toBeInTheDocument();
  });
});
