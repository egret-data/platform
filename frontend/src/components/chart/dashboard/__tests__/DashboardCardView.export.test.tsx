import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";

vi.mock("@/components/chart/ChartRenderer", () => ({ ChartRenderer: () => <div data-testid="cr" /> }));
const useChartData = vi.fn();
vi.mock("@/api/useChartData", () => ({ useChartData: (...a: unknown[]) => useChartData(...a) }));

import { DashboardCardView } from "../DashboardCardView";

const chart = { id: "c", dataset_id: "d", name: "C", description: "", created_by: null, updated_at: "", viz_type: "bar", params: { spec: { metrics: [], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [] } } } as never;
const card = { id: "k", card_type: "chart", chart_id: "c", options: {} } as never;

describe("DashboardCardView export affordance", () => {
  it("shows the export button for a loaded chart card in view mode", () => {
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    render(<DashboardCardView ws="eng" card={card} chart={chart} parameters={[]} paramValues={{}} />);
    expect(screen.getByRole("button", { name: /export chart/i })).toBeTruthy();
  });
  it("hides the export button while loading", () => {
    useChartData.mockReturnValue({ columns: [], rows: [], isLoading: true, error: null });
    render(<DashboardCardView ws="eng" card={card} chart={chart} parameters={[]} paramValues={{}} />);
    expect(screen.queryByRole("button", { name: /export chart/i })).toBeNull();
  });
  it("hides the export button in edit mode", () => {
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    render(<DashboardCardView ws="eng" card={card} chart={chart} parameters={[]} paramValues={{}} editing />);
    expect(screen.queryByRole("button", { name: /export chart/i })).toBeNull();
  });
});
