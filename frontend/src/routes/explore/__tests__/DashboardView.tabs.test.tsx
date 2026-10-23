import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import DashboardView from "../DashboardView";

vi.mock("@/components/chart/dashboard/DashboardCardView", () => ({
  DashboardCardView: ({ card }: { card: { id: string } }) => <div data-testid={`card-${card.id}`} />,
}));
vi.mock("@/api/charts", async (orig) => ({
  ...(await orig<typeof import("@/api/charts")>()),
  useCharts: () => ({ data: [] }),
}));

const detail = {
  id: "d1", name: "D", description: "", created_by: null, updated_at: "", parameters: [],
  tabs: [{ id: "t1", name: "A", position: 0 }, { id: "t2", name: "B", position: 1 }],
  cards: [
    { id: "c1", chart_id: null, card_type: "heading", grid_col: 0, grid_row: 0, size_x: 6, size_y: 1, options: { text: "h" }, tab_id: "t1" },
    { id: "c2", chart_id: null, card_type: "heading", grid_col: 0, grid_row: 1, size_x: 6, size_y: 1, options: { text: "h" }, tab_id: "t2" },
  ],
};
vi.mock("@/api/dashboards", async (orig) => {
  const mod = await (orig as () => Promise<Record<string, unknown>>)();
  return { ...mod,
    useDashboard: () => ({ data: detail, isLoading: false }),
    useDashboardMutations: () => ({ update: { mutateAsync: vi.fn(), isPending: false } }),
  };
});

const renderView = (path = "/explore/eng/dashboards/d1") => {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[path]}>
        <Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
};

describe("DashboardView tabs", () => {
  it("shows the tab bar and mounts only the active tab's cards", () => {
    renderView();
    expect(screen.getByRole("tab", { name: "A" })).toBeTruthy();
    expect(screen.getByTestId("card-c1")).toBeTruthy();
    expect(screen.queryByTestId("card-c2")).toBeNull();
  });
  it("switching tabs mounts the other tab's cards", () => {
    renderView();
    fireEvent.click(screen.getByRole("tab", { name: "B" }));
    expect(screen.getByTestId("card-c2")).toBeTruthy();
    expect(screen.queryByTestId("card-c1")).toBeNull();
  });
  it("honours ?tab= deep link", () => {
    renderView("/explore/eng/dashboards/d1?tab=1");
    expect(screen.getByTestId("card-c2")).toBeTruthy();
  });
});

describe("DashboardView tab edit affordances", () => {
  it("deleting a tab with cards opens a reassign dialog and reassigns", async () => {
    const user = userEvent.setup();
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    await user.click(screen.getByRole("button", { name: "tab menu A" }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    // The dialog pre-selects the first other tab; picking it explicitly drives the shared select.
    await user.click(screen.getByRole("combobox", { name: "reassign target" }));
    await user.click((await screen.findAllByRole("option"))[0]);
    fireEvent.click(screen.getByRole("button", { name: "reassign and delete" }));
    expect(screen.queryByRole("tab", { name: "A" })).toBeNull();
  });
  it("a card shows a move-to-tab control in edit mode with >=2 tabs", () => {
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.click(screen.getAllByTestId("dashcard-body")[0]);
    expect(screen.getByLabelText("move card to tab")).toBeTruthy();
  });
  it("shows a tab options trigger per tab in edit mode only", () => {
    renderView();
    // view mode: no tab menu triggers
    expect(screen.queryByRole("button", { name: /tab menu/i })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    // edit mode: one trigger per tab (fixture has 2 tabs)
    expect(screen.getAllByRole("button", { name: /tab menu/i }).length).toBeGreaterThanOrEqual(2);
  });
});
