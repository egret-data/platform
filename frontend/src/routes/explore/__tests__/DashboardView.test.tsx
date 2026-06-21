import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import DashboardView from "../DashboardView";

vi.mock("@/api/dashboards", () => ({
  useDashboard: () => ({
    data: {
      id: "d1", name: "Ops", description: "", created_by: null, updated_at: "",
      cards: [{ id: "k1", chart_id: "c1", card_type: "chart", grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: {} }],
    },
    isLoading: false,
  }),
  useDashboardMutations: () => ({ update: { mutateAsync: vi.fn(), isPending: false } }),
}));
vi.mock("@/api/charts", async (orig) => ({
  ...(await orig<typeof import("@/api/charts")>()),
  useCharts: () => ({ data: [{ id: "c1", dataset_id: "ds1", name: "Sales", viz_type: "table", description: "", params: { spec: { metrics: [], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] } }, created_by: null, updated_at: "" }] }),
  useChart: () => ({ data: undefined }),
}));
vi.mock("@/api/useChartData", () => ({
  useChartData: () => ({ columns: [{ name: "rev", type: "double" }], rows: [[5]], isLoading: false, error: null }),
}));

function renderAt() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/dashboards/d1"]}>
        <Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("DashboardView (view mode)", () => {
  it("renders the dashboard name and a card with its chart title", () => {
    renderAt();
    expect(screen.getByText("Ops")).toBeInTheDocument();
    expect(screen.getByText("Sales")).toBeInTheDocument();
  });
  it("shows an Edit button (not editing by default)", () => {
    renderAt();
    expect(screen.getByRole("button", { name: /edit/i })).toBeInTheDocument();
  });
});
