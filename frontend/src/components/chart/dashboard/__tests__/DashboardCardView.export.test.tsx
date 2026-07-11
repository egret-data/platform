import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

// Mock ChartRenderer faithfully: chart viz renders an <svg> (ECharts), table renders an HTML
// table (no <svg>). This lets us assert the PNG item only appears when the *chart* svg exists —
// the export menu must read the render container, not the whole tile (whose affordance holds a
// Download icon <svg> that would otherwise make every card — even tables — offer PNG).
vi.mock("@/components/chart/ChartRenderer", () => ({
  ChartRenderer: ({ vizType }: { vizType: string }) =>
    vizType === "table" ? <table data-testid="cr-table" /> : <svg data-testid="chart-svg" />,
}));
// One mock fn drives both the edit-mode (useChartData) and view-mode (useDashboardCardData) paths,
// so each test's mockReturnValue applies whichever mode it renders.
const useChartData = vi.fn();
vi.mock("@/api/useChartData", () => ({ useChartData: (...a: unknown[]) => useChartData(...a) }));
vi.mock("@/api/useSavedChartData", () => ({ useDashboardCardData: (...a: unknown[]) => useChartData(...a) }));

import { DashboardCardView } from "../DashboardCardView";

const chart = { id: "c", dataset_id: "d", name: "C", description: "", created_by: null, updated_at: "", viz_type: "bar", params: { spec: { metrics: [], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [] } } } as never;
const tableChart = { ...(chart as object), viz_type: "table" } as never;
const card = { id: "k", card_type: "chart", chart_id: "c", options: {} } as never;

describe("DashboardCardView export affordance", () => {
  it("shows the export button for a loaded chart card in view mode", () => {
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    render(<DashboardCardView ws="eng" dashboardId="d1" card={card} chart={chart} parameters={[]} paramValues={{}} />);
    expect(screen.getByRole("button", { name: /export chart/i })).toBeTruthy();
  });
  it("hides the export button while loading", () => {
    useChartData.mockReturnValue({ columns: [], rows: [], isLoading: true, error: null });
    render(<DashboardCardView ws="eng" dashboardId="d1" card={card} chart={chart} parameters={[]} paramValues={{}} />);
    expect(screen.queryByRole("button", { name: /export chart/i })).toBeNull();
  });
  it("hides the export button in edit mode", () => {
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    render(<DashboardCardView ws="eng" dashboardId="d1" card={card} chart={chart} parameters={[]} paramValues={{}} editing />);
    expect(screen.queryByRole("button", { name: /export chart/i })).toBeNull();
  });

  it("offers Download PNG for a chart card (chart svg present in the render container)", async () => {
    const user = userEvent.setup();
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    render(<DashboardCardView ws="eng" dashboardId="d1" card={card} chart={chart} parameters={[]} paramValues={{}} />);
    await user.click(screen.getByRole("button", { name: /export chart/i }));
    expect(await screen.findByText("Download CSV")).toBeTruthy();
    expect(screen.getByText("Download PNG")).toBeTruthy();
  });

  // Regression: the export menu must read the render-only container, not the whole tile. The tile's
  // affordance contains a Download icon <svg>; if the menu pointed at the tile, a table card (which
  // has no chart svg) would still find that icon and wrongly offer PNG — exporting a 32×32 icon.
  it("does NOT offer Download PNG for a table card (no chart svg)", async () => {
    const user = userEvent.setup();
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    render(<DashboardCardView ws="eng" dashboardId="d1" card={card} chart={tableChart} parameters={[]} paramValues={{}} />);
    await user.click(screen.getByRole("button", { name: /export chart/i }));
    expect(await screen.findByText("Download CSV")).toBeTruthy();
    expect(screen.queryByText("Download PNG")).toBeNull();
  });
});
