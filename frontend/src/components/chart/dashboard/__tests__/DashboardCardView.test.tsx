import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { DashboardCardView } from "../DashboardCardView";
import type { DashboardCard } from "@/api/dashboards";
import type { Chart } from "@/api/charts";

// DashboardCardView renders react-router <Link>s (the card title and the Edit-question
// affordance), which throw outside a Router.
function renderCard(ui: React.ReactElement) {
  return render(<MemoryRouter>{ui}</MemoryRouter>);
}

// View-mode chart cards read through the object-scoped card-data hook.
const useDashboardCardData = vi.fn(() => ({ columns: [{ name: "rev", type: "double" }], rows: [[5]], isLoading: false, error: null }));
vi.mock("@/api/useSavedChartData", () => ({
  useDashboardCardData: (...a: unknown[]) => useDashboardCardData(...(a as [])),
}));
// Edit-mode path (unused by these view-mode tests) — mocked so the real hook's react-query deps never load.
vi.mock("@/api/useChartData", () => ({
  useChartData: () => ({ columns: [{ name: "rev", type: "double" }], rows: [[5]], isLoading: false, error: null }),
}));
vi.mock("@/components/chart/ChartRenderer", () => ({
  ChartRenderer: ({ onValueClick }: { onValueClick?: (ctx: unknown, x: number, y: number) => void }) =>
    <button data-testid="chart" onClick={() => onValueClick?.({ dims: [{ col: "region", value: "EU" }] }, 1, 2)} />,
}));

const chart: Chart = {
  id: "c1", dataset_id: "ds1", name: "Sales", viz_type: "table", description: "",
  // A table with a column: a chart that is not set up renders a hint instead (KI-003).
  params: { spec: { metrics: [], dimensions: [], columns: ["rev"], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] } },
  created_by: null, updated_at: "", folder_id: null,
};
const card = (over: Partial<DashboardCard>): DashboardCard => ({
  id: "k", chart_id: null, card_type: "chart", grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: {}, tab_id: null, ...over,
});

describe("DashboardCardView", () => {
  beforeEach(() => useDashboardCardData.mockClear());

  it("renders a chart card with rows from the card-data hook and the chart name as header", () => {
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={card({ chart_id: "c1" })} chart={chart} />);
    expect(screen.getByText("Sales")).toBeInTheDocument();
    expect(screen.getByTestId("chart")).toBeInTheDocument();
    // The chart-backed card fetches via the object-scoped card-data endpoint
    // (ws, dashboardId, cardId, nonce, enabled, paramFilters, paramTemporal). No params
    // → empty filters + empty temporal overrides.
    expect(useDashboardCardData).toHaveBeenCalledWith("eng", "d1", "k", undefined, true, [], {});
  });
  it("forwards resolved dashboard-param filters to the card-data hook", () => {
    const paramFilters = [{ col: "region", op: "eq" as const, value: "EU" }];
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={card({ chart_id: "c1" })} chart={chart} paramFilters={paramFilters} />);
    // The filter body reaches the hook, so the endpoint applies it server-side.
    expect(useDashboardCardData).toHaveBeenCalledWith("eng", "d1", "k", undefined, true, paramFilters, {});
  });
  it("forwards resolved temporal-unit overrides to the card-data hook", () => {
    const paramTemporal = { created_at: "month" };
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={card({ chart_id: "c1" })} chart={chart} paramTemporal={paramTemporal} />);
    // The temporal overrides reach the hook, so the endpoint re-grids server-side.
    expect(useDashboardCardData).toHaveBeenCalledWith("eng", "d1", "k", undefined, true, [], paramTemporal);
  });
  it("a param-filter change re-invokes the hook with the new filters (drives a refetch)", () => {
    const { rerender } = renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={card({ chart_id: "c1" })} chart={chart} paramFilters={[{ col: "region", op: "eq" as const, value: "EU" }]} />);
    expect(useDashboardCardData).toHaveBeenLastCalledWith("eng", "d1", "k", undefined, true, [{ col: "region", op: "eq", value: "EU" }], {});
    // Wrapped by hand: `rerender` of a bare element would drop the Router `renderCard` added.
    rerender(<MemoryRouter><DashboardCardView ws="eng" dashboardId="d1" card={card({ chart_id: "c1" })} chart={chart} paramFilters={[{ col: "region", op: "eq" as const, value: "US" }]} /></MemoryRouter>);
    // A changed filter value → a new filters array → the hook (and its query key) sees it.
    expect(useDashboardCardData).toHaveBeenLastCalledWith("eng", "d1", "k", undefined, true, [{ col: "region", op: "eq", value: "US" }], {});
  });
  it("uses options.title to override the header", () => {
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={card({ chart_id: "c1", options: { title: "Revenue" } })} chart={chart} />);
    expect(screen.getByText("Revenue")).toBeInTheDocument();
    expect(screen.queryByText("Sales")).not.toBeInTheDocument();
  });
  it("hides the header when options.title is empty string", () => {
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={card({ chart_id: "c1", options: { title: "" } })} chart={chart} />);
    expect(screen.queryByText("Sales")).not.toBeInTheDocument();
    expect(screen.getByTestId("chart")).toBeInTheDocument();
  });
  it("routes a text card to markdown and does NOT fetch data", () => {
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={card({ card_type: "text", options: { text: "# Hi" } })} chart={undefined} />);
    expect(screen.getByRole("heading", { name: "Hi" })).toBeInTheDocument();
    expect(useDashboardCardData).not.toHaveBeenCalled();
  });
  it("routes a heading card and does NOT fetch data", () => {
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={card({ card_type: "heading", options: { text: "Section" } })} chart={undefined} />);
    expect(screen.getByText("Section")).toBeInTheDocument();
    expect(useDashboardCardData).not.toHaveBeenCalled();
  });
  it("routes a link card and does NOT fetch data", () => {
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={card({ card_type: "link", options: { url: "https://x.com", label: "X" } })} chart={undefined} />);
    expect(screen.getByRole("link", { name: "X" })).toHaveAttribute("href", "https://x.com");
    expect(useDashboardCardData).not.toHaveBeenCalled();
  });
  it("routes an iframe card and does NOT fetch data", () => {
    const { container } = renderCard(
      <DashboardCardView ws="eng" dashboardId="d1" chart={undefined}
        card={card({ card_type: "iframe", options: { iframe: "https://www.loom.com/share/abc" } })} />,
    );
    expect(container.querySelector("iframe")).toHaveAttribute("src", "https://www.loom.com/embed/abc");
    expect(useDashboardCardData).not.toHaveBeenCalled();
  });
  it("routes an iframe card with a forbidden host to the refusal, still without fetching", () => {
    const { container } = renderCard(
      <DashboardCardView ws="eng" dashboardId="d1" chart={undefined}
        card={card({ card_type: "iframe", options: { iframe: "https://example.com" } })} />,
    );
    expect(container.querySelector("iframe")).toBeNull();
    expect(useDashboardCardData).not.toHaveBeenCalled();
  });
  it("threads onValueClick to a chart card", () => {
    const onValueClick = vi.fn();
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={card({ chart_id: "c1" })} chart={chart} onValueClick={onValueClick} />);
    fireEvent.click(screen.getByTestId("chart"));
    expect(onValueClick).toHaveBeenCalledWith({ dims: [{ col: "region", value: "EU" }] }, 1, 2);
  });
});

// KI-003: a chart saved without an X-axis drew an empty card with no word of why, while the
// chart editor says "Set up your Bar". The card now says the same thing.
describe("DashboardCardView — a chart that is not set up", () => {
  const unfinished: Chart = {
    ...chart, name: "Half a bar", viz_type: "bar",
    params: { version: 2, spec: { metrics: ["rev"], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] }, settings: { "cartesian.yMetrics": ["rev"] } } as Chart["params"],
  };

  it("says what the chart still needs instead of drawing an empty card", () => {
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={card({ chart_id: "c1" })} chart={unfinished} />);
    expect(screen.getByText("Half a bar")).toBeInTheDocument();
    expect(screen.getByText("This chart is not set up yet")).toBeInTheDocument();
    expect(screen.getByText("Add: X-axis")).toBeInTheDocument();
    expect(screen.queryByTestId("chart")).toBeNull();
  });

  it("says it in edit mode too", () => {
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={card({ chart_id: "c1" })} chart={unfinished} editing />);
    expect(screen.getByText("This chart is not set up yet")).toBeInTheDocument();
  });
});
