import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { DashboardCardView } from "../DashboardCardView";
import type { DashboardCard } from "@/api/dashboards";
import type { Chart } from "@/api/charts";

vi.mock("@/api/useChartData", () => ({
  useChartData: () => ({ columns: [{ name: "rev", type: "double" }], rows: [[5]], isLoading: false, error: null }),
}));
vi.mock("@/components/chart/ChartRenderer", () => ({ ChartRenderer: () => <div data-testid="chart" /> }));

const chart: Chart = {
  id: "c1", dataset_id: "ds1", name: "Sales", viz_type: "table", description: "",
  params: { spec: { metrics: [], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] } },
  created_by: null, updated_at: "",
};
const card = (over: Partial<DashboardCard>): DashboardCard => ({
  id: "k", chart_id: null, card_type: "chart", grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: {}, ...over,
});

describe("DashboardCardView", () => {
  it("renders a chart card with the chart name as header by default", () => {
    render(<DashboardCardView ws="eng" card={card({ chart_id: "c1" })} chart={chart} />);
    expect(screen.getByText("Sales")).toBeInTheDocument();
    expect(screen.getByTestId("chart")).toBeInTheDocument();
  });
  it("uses options.title to override the header", () => {
    render(<DashboardCardView ws="eng" card={card({ chart_id: "c1", options: { title: "Revenue" } })} chart={chart} />);
    expect(screen.getByText("Revenue")).toBeInTheDocument();
    expect(screen.queryByText("Sales")).not.toBeInTheDocument();
  });
  it("hides the header when options.title is empty string", () => {
    render(<DashboardCardView ws="eng" card={card({ chart_id: "c1", options: { title: "" } })} chart={chart} />);
    expect(screen.queryByText("Sales")).not.toBeInTheDocument();
    expect(screen.getByTestId("chart")).toBeInTheDocument();
  });
  it("routes a text card to markdown", () => {
    render(<DashboardCardView ws="eng" card={card({ card_type: "text", options: { text: "# Hi" } })} chart={undefined} />);
    expect(screen.getByRole("heading", { name: "Hi" })).toBeInTheDocument();
  });
  it("routes a heading card", () => {
    render(<DashboardCardView ws="eng" card={card({ card_type: "heading", options: { text: "Section" } })} chart={undefined} />);
    expect(screen.getByText("Section")).toBeInTheDocument();
  });
  it("routes a link card", () => {
    render(<DashboardCardView ws="eng" card={card({ card_type: "link", options: { url: "https://x.com", label: "X" } })} chart={undefined} />);
    expect(screen.getByRole("link", { name: "X" })).toHaveAttribute("href", "https://x.com");
  });
});
