import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
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
});
