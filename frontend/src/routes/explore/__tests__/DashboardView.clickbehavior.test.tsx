import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import DashboardView from "../DashboardView";

const dashboard = {
  id: "d1", name: "Ops", description: "", created_by: null, updated_at: "",
  parameters: [{ id: "pr", name: "Region", slug: "region", type: "text", operator: "is" }],
  cards: [{ id: "k1", chart_id: "c1", card_type: "chart", grid_col: 0, grid_row: 0, size_x: 6, size_y: 4,
    options: { clickBehavior: { type: "crossfilter", parameterMapping: { pr: "region" } } } }],
};
vi.mock("@/api/dashboards", () => ({
  useDashboard: () => ({ data: dashboard, isLoading: false }),
  useDashboardMutations: () => ({ update: { mutateAsync: vi.fn(), isPending: false } }),
}));
vi.mock("@/api/charts", async (orig) => ({
  ...(await orig<typeof import("@/api/charts")>()),
  useCharts: () => ({ data: [{ id: "c1", dataset_id: "ds1", name: "Sales", viz_type: "table", description: "",
    params: { spec: { metrics: ["rev"], dimensions: ["region"], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] } },
    created_by: null, updated_at: "" }] }),
}));
vi.mock("@/components/chart/dashboard/DashboardCardView", () => ({
  DashboardCardView: ({ card, onValueClick }: { card: { id: string }; onValueClick?: (ctx: unknown, x: number, y: number) => void }) =>
    <button data-testid={`cardclick-${card.id}`} onClick={() => onValueClick?.({ dims: [{ col: "region", value: "EU" }] }, 0, 0)} />,
}));

function renderAt() {
  return render(
    <QueryClientProvider client={new QueryClient()}>
      <MemoryRouter initialEntries={["/explore/eng/dashboards/d1"]}>
        <Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("DashboardView click behaviour", () => {
  it("cross-filters when a card's click behaviour is crossfilter", () => {
    renderAt();
    fireEvent.click(screen.getByTestId("cardclick-k1"));
    expect((screen.getByLabelText("Region") as HTMLInputElement).value).toBe("EU");
  });
});
