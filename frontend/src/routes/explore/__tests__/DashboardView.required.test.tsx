import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import DashboardView from "../DashboardView";

vi.mock("@/components/chart/dashboard/DashboardCardView", () => ({
  DashboardCardView: ({ card, blocked }: { card: { id: string }; blocked?: boolean }) => <div data-testid={`card-${card.id}`} data-blocked={blocked ? "1" : "0"} />,
}));
vi.mock("@/api/charts", async (orig) => ({
  ...(await orig<typeof import("@/api/charts")>()),
  useCharts: () => ({ data: [] }),
}));

let detail: Record<string, unknown> = {
  id: "d1", name: "D", description: "", created_by: null, updated_at: "",
  parameters: [{ id: "pr", name: "Region", slug: "region", type: "category", operator: "is", required: true }],
  tabs: [],
  cards: [{ id: "c1", chart_id: null, card_type: "heading", grid_col: 0, grid_row: 0, size_x: 6, size_y: 1, options: { text: "h" }, tab_id: null }],
};
vi.mock("@/api/dashboards", async (orig) => {
  const mod = await (orig as () => Promise<Record<string, unknown>>)();
  return { ...mod, useDashboard: () => ({ data: detail, isLoading: false }), useDashboardMutations: () => ({ update: { mutateAsync: vi.fn(), isPending: false } }) };
});

const renderView = (path = "/explore/eng/dashboards/d1") => render(
  <QueryClientProvider client={new QueryClient()}>
    <MemoryRouter initialEntries={[path]}>
      <Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes>
    </MemoryRouter>
  </QueryClientProvider>,
);

describe("DashboardView required-filter gating", () => {
  it("blocks cards while a required filter is unset", () => {
    renderView();
    expect(screen.getByTestId("card-c1").getAttribute("data-blocked")).toBe("1");
  });
  it("unblocks once the required filter is set (via URL)", () => {
    renderView("/explore/eng/dashboards/d1?region=EU");
    expect(screen.getByTestId("card-c1").getAttribute("data-blocked")).toBe("0");
  });
});

describe("DashboardView empty tab", () => {
  it("shows 'No cards on this tab.' when the active tab has no cards", () => {
    detail = { ...detail, parameters: [], tabs: [{ id: "t1", name: "A", position: 0 }, { id: "t2", name: "B", position: 1 }],
      cards: [{ id: "c1", chart_id: null, card_type: "heading", grid_col: 0, grid_row: 0, size_x: 6, size_y: 1, options: { text: "h" }, tab_id: "t1" }] };
    renderView("/explore/eng/dashboards/d1?tab=1");
    expect(screen.getByText("No cards on this tab.")).toBeTruthy();
  });
});
