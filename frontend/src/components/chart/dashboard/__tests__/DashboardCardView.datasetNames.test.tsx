import { describe, it, expect, vi } from "vitest";
import { render } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { DashboardCardView } from "../DashboardCardView";
import type { DashboardCard } from "@/api/dashboards";
import type { AnyChartParams, Chart } from "@/api/charts";
import { shellSettings } from "@/components/chart/viz/renderConfig";

vi.mock("@/api/useSavedChartData", () => ({
  useDashboardCardData: () => ({ columns: [{ name: "revenue", type: "double" }], rows: [[5]], isLoading: false, error: null }),
}));
vi.mock("@/api/useChartData", () => ({
  useChartData: () => ({ columns: [{ name: "revenue", type: "double" }], rows: [[5]], isLoading: false, error: null }),
}));
const rendered = vi.hoisted(() => ({ params: undefined as unknown }));
vi.mock("@/components/chart/ChartRenderer", () => ({
  ChartRenderer: ({ params }: { params: unknown }) => { rendered.params = params; return <div data-testid="chart" />; },
}));

const chart = (settings: Record<string, unknown> = {}): Chart => ({
  id: "c1", dataset_id: "ds1", name: "Revenue by priority", viz_type: "table", description: "",
  params: {
    version: 2,
    spec: { metrics: [], dimensions: [], columns: ["revenue"], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] },
    settings,
  } as unknown as AnyChartParams,
  created_by: null, updated_at: "", folder_id: null,
});
const card: DashboardCard = {
  id: "k", chart_id: "c1", card_type: "chart", grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: {}, tab_id: null,
};
const drawnSettings = () => shellSettings(rendered.params as AnyChartParams).columnSettings;

// KI-290 / KI-251: a card did not load its chart's dataset, so it drew `revenue` where the chart
// page draws the dataset's display name "Revenue", and the dataset's formats did not apply.
describe("DashboardCardView with the dataset's column defaults (KI-290, KI-251)", () => {
  it("draws the dataset's display name and format under the chart's own settings", () => {
    render(<MemoryRouter><DashboardCardView ws="eng" dashboardId="d1" card={card} chart={chart()}
      datasetDefaults={{ revenue: { column_title: "Revenue", decimals: 0 } }} /></MemoryRouter>);
    expect(drawnSettings().revenue).toEqual({ column_title: "Revenue", decimals: 0 });
  });

  it("keeps a column title the chart set itself", () => {
    render(<MemoryRouter><DashboardCardView ws="eng" dashboardId="d1" card={card}
      chart={chart({ column_settings: { revenue: { column_title: "Sales" } } })}
      datasetDefaults={{ revenue: { column_title: "Revenue" } }} /></MemoryRouter>);
    expect(drawnSettings().revenue.column_title).toBe("Sales");
  });
});
