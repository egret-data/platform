import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ChartSettingsSections } from "../ChartSettingsSections";
import type { ChartParams } from "@/api/charts";

const base: ChartParams = {
  spec: { metrics: ["rev"], dimensions: ["month"], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [] },
  cartesian: { yMetrics: ["rev"], xAxis: "month" },
};
const columns = [
  { name: "month", type: "VARCHAR" },
  { name: "rev", type: "DOUBLE" },
];

describe("ChartSettingsSections", () => {
  it("renders the cartesian display section for a bar viz and emits a full ChartParams on change", () => {
    const onChange = vi.fn();
    render(<ChartSettingsSections vizType={"bar"} params={base} columns={columns} rows={[]} onChange={onChange} />);
    // CartesianDisplaySection renders a "Legend" toggle (a switch) — a stable control.
    fireEvent.click(screen.getByLabelText("Legend"));
    expect(onChange).toHaveBeenCalled();
    const next = onChange.mock.calls[0][0] as ChartParams;
    // Emits a full ChartParams (still carries spec) with the patched cartesian field merged in.
    expect(next).toHaveProperty("spec");
    expect(next.cartesian).toMatchObject({ yMetrics: ["rev"], xAxis: "month", showLegend: true });
  });

  it("renders the pie display section for a pie viz and emits a full ChartParams on change", () => {
    const onChange = vi.fn();
    const pieParams: ChartParams = { spec: base.spec, pie: { dimension: "month", metric: "rev" } };
    render(<ChartSettingsSections vizType={"pie"} params={pieParams} columns={columns} rows={[]} onChange={onChange} />);
    fireEvent.click(screen.getByLabelText("Donut"));
    expect(onChange).toHaveBeenCalled();
    const next = onChange.mock.calls[0][0] as ChartParams;
    expect(next).toHaveProperty("spec");
    expect(next.pie).toMatchObject({ dimension: "month", metric: "rev", donut: true });
  });

  it("renders table column + format + conditional sections for a table viz with result columns", () => {
    const onChange = vi.fn();
    const tableParams: ChartParams = { spec: base.spec };
    render(<ChartSettingsSections vizType={"table"} params={tableParams} columns={columns} rows={[]} onChange={onChange} />);
    // Both the Columns (visibility) and Formatting sections list every result
    // column, so each name appears more than once.
    expect(screen.getAllByText("month").length).toBeGreaterThan(0);
    expect(screen.getAllByText("rev").length).toBeGreaterThan(0);
    // The Columns section header is present for a table viz.
    expect(screen.getByText("Columns")).toBeInTheDocument();
    expect(screen.getByText("Conditional formatting")).toBeInTheDocument();
  });

  it("prompts to run when there are no result columns", () => {
    render(<ChartSettingsSections vizType={"table"} params={{ spec: base.spec }} columns={[]} rows={[]} onChange={() => {}} />);
    expect(screen.getByText(/Run the chart to configure column formatting/i)).toBeInTheDocument();
  });
});
