import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import DashboardView from "../DashboardView";

vi.mock("@/components/chart/dashboard/DashboardCardView", () => ({ DashboardCardView: ({ card }: { card: { id: string } }) => <div data-testid={`disp-${card.id}`} /> }));
vi.mock("@/api/charts", async (orig) => ({ ...(await (orig as () => Promise<Record<string, unknown>>)()), useCharts: () => ({ data: [{ id: "ch", name: "Sales", dataset_id: "d", viz_type: "bar", params: { spec: { dimensions: [], columns: [], metrics: ["m"], filters: [], orderby: [], row_limit: 100, rollup_groups: [] } } }] }) }));
const detail = { id: "d1", name: "D", description: "", created_by: null, updated_at: "", parameters: [], tabs: [], width: "full",
  cards: [{ id: "c1", chart_id: null, card_type: "heading", grid_col: 0, grid_row: 0, size_x: 6, size_y: 1, options: { text: "h" }, tab_id: null }] };
vi.mock("@/api/dashboards", async (orig) => { const m = await (orig as () => Promise<Record<string, unknown>>)();
  return { ...m, useDashboard: () => ({ data: detail, isLoading: false }), useDashboardMutations: () => ({ update: { mutateAsync: vi.fn(), isPending: false } }) }; });
const renderView = () => render(<QueryClientProvider client={new QueryClient()}><MemoryRouter initialEntries={["/explore/eng/dashboards/d1"]}><Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes></MemoryRouter></QueryClientProvider>);

describe("DashboardView edit sidebar", () => {
  it("entering edit shows the Add panel; +Filter opens a filter panel", () => {
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    expect(screen.getByLabelText("search charts")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "+ Filter" }));
    expect(screen.getByLabelText("parameter name")).toBeTruthy();
  });
  it("clicking a card's settings opens the card panel", () => {
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.click(screen.getByRole("button", { name: "card settings" }));
    expect(screen.getByLabelText(/heading text|text body/)).toBeTruthy();
  });
});
