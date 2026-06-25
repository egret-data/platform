import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import DashboardView from "../DashboardView";

vi.mock("@/components/chart/dashboard/DashboardCardView", () => ({ DashboardCardView: () => <div /> }));
vi.mock("@/api/charts", async (orig) => ({
  ...(await orig<typeof import("@/api/charts")>()),
  useCharts: () => ({ data: [] }),
}));

const update = { mutateAsync: vi.fn().mockResolvedValue({}), isPending: false };
const base = {
  id: "d1", name: "D", description: "", created_by: null, updated_at: "", parameters: [], tabs: [],
  cards: [{ id: "c1", chart_id: null, card_type: "heading", grid_col: 0, grid_row: 0, size_x: 6, size_y: 1, options: { text: "h" }, tab_id: null }],
  width: "full",
};
let detail: Record<string, unknown> = { ...base };
vi.mock("@/api/dashboards", async (orig) => {
  const mod = await (orig as () => Promise<Record<string, unknown>>)();
  return { ...mod, useDashboard: () => ({ data: detail, isLoading: false }), useDashboardMutations: () => ({ update }) };
});

const renderView = () => render(
  <QueryClientProvider client={new QueryClient()}>
    <MemoryRouter initialEntries={["/explore/eng/dashboards/d1"]}>
      <Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes>
    </MemoryRouter>
  </QueryClientProvider>,
);

describe("DashboardView width", () => {
  it("view mode 'fixed' wraps content in a centered max-width container", () => {
    detail = { ...base, width: "fixed" };
    const { container } = renderView();
    expect(container.querySelector(".max-w-\\[1200px\\]")).toBeTruthy();
  });
  it("view mode 'full' has no fixed wrapper", () => {
    detail = { ...base, width: "full" };
    const { container } = renderView();
    expect(container.querySelector(".max-w-\\[1200px\\]")).toBeNull();
  });
  it("edit mode: saving sends the stored dashboard width in the payload", () => {
    detail = { ...base, width: "fixed" };
    update.mutateAsync.mockClear();
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(update.mutateAsync).toHaveBeenCalledWith(expect.objectContaining({ width: "fixed" }));
  });
});
