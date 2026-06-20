import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { DrillMenu } from "../DrillMenu";

const ctx = { dims: [{ col: "region", value: "EU" }] };

describe("DrillMenu", () => {
  it("shows a Filter-by item per dim and See-records, fires callbacks", () => {
    const onFilterBy = vi.fn(); const onSeeRecords = vi.fn(); const onClose = vi.fn();
    render(<DrillMenu ctx={ctx} x={10} y={10} onFilterBy={onFilterBy} onSeeRecords={onSeeRecords} onClose={onClose} />);
    fireEvent.click(screen.getByText(/Filter by region is EU/i));
    expect(onFilterBy).toHaveBeenCalledWith({ col: "region", op: "eq", value: "EU" });
    fireEvent.click(screen.getByText(/See these records/i));
    expect(onSeeRecords).toHaveBeenCalledWith([{ col: "region", op: "eq", value: "EU" }]);
  });
  it("with no dims shows only See-records", () => {
    render(<DrillMenu ctx={{ dims: [] }} x={0} y={0} onFilterBy={() => {}} onSeeRecords={() => {}} onClose={() => {}} />);
    expect(screen.queryByText(/Filter by/i)).not.toBeInTheDocument();
    expect(screen.getByText(/See these records/i)).toBeInTheDocument();
  });
});
