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
  it("sidebar is hidden on entering edit; add chart opens Add panel; +Filter opens filter panel", () => {
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    // Sidebar must NOT be visible yet — the Add panel only appears after clicking the toolbar button
    expect(screen.queryByLabelText("search charts")).toBeNull();
    // Click the "add chart" toolbar button to open the Add panel
    fireEvent.click(screen.getByRole("button", { name: "add chart" }));
    expect(screen.getByLabelText("search charts")).toBeTruthy();
    // +Filter opens the filter panel directly (and replaces the Add panel)
    fireEvent.click(screen.getByRole("button", { name: "add filter" }));
    expect(screen.getByLabelText("parameter name")).toBeTruthy();
  });
  it("close panel (X) hides the sidebar", () => {
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.click(screen.getByRole("button", { name: "add chart" }));
    expect(screen.getByLabelText("search charts")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "close panel" }));
    expect(screen.queryByLabelText("search charts")).toBeNull();
  });
  it("clicking a card opens the card panel", () => {
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.click(screen.getAllByTestId("dashcard-body")[0]);
    expect(screen.getByText(/Insert a filter value/i)).toBeInTheDocument();
  });
  // KI-068: a drag ends with a mouseup on the card, and the click that follows opened the panel,
  // which then narrowed the canvas under the card that had just been placed.
  it("the click that ends a drag does not open the card panel", () => {
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    const body = screen.getAllByTestId("dashcard-body")[0];
    fireEvent.mouseDown(body, { clientX: 10, clientY: 10 });
    fireEvent.click(body, { clientX: 210, clientY: 60 });
    expect(screen.queryByText(/Insert a filter value/i)).toBeNull();
    // A click that did not move still opens it.
    fireEvent.mouseDown(body, { clientX: 10, clientY: 10 });
    fireEvent.click(body, { clientX: 11, clientY: 10 });
    expect(screen.getByText(/Insert a filter value/i)).toBeInTheDocument();
  });
});
