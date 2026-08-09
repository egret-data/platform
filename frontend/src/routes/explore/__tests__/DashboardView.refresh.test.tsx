import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route, useLocation } from "react-router-dom";
import DashboardView from "../DashboardView";

vi.mock("@/components/chart/dashboard/DashboardCardView", () => ({ DashboardCardView: () => <div /> }));
vi.mock("@/api/charts", async (orig) => ({
  ...(await orig<typeof import("@/api/charts")>()),
  useCharts: () => ({ data: [] }),
}));

const detail = {
  id: "d1", name: "D", description: "", created_by: null, updated_at: "", parameters: [], tabs: [],
  cards: [{ id: "c1", chart_id: null, card_type: "heading", grid_col: 0, grid_row: 0, size_x: 6, size_y: 1, options: { text: "h" }, tab_id: null }],
  width: "full",
};
vi.mock("@/api/dashboards", async (orig) => {
  const mod = await (orig as () => Promise<Record<string, unknown>>)();
  return {
    ...mod,
    useDashboard: () => ({ data: detail, isLoading: false }),
    useDashboards: () => ({ data: [] }),
    useDashboardMutations: () => ({
      update: { mutateAsync: vi.fn().mockResolvedValue({}), isPending: false },
      restore: { mutateAsync: vi.fn().mockResolvedValue(undefined) },
      duplicate: { mutateAsync: vi.fn().mockResolvedValue({ id: "d2" }) },
      trash: { mutateAsync: vi.fn().mockResolvedValue(undefined) },
      addBookmark: { mutateAsync: vi.fn().mockResolvedValue(undefined) },
      removeBookmark: { mutateAsync: vi.fn().mockResolvedValue(undefined) },
    }),
  };
});

/** Renders the current search string so the URL sync can be asserted without reaching into the
 *  router's internals. */
function ShowSearch() {
  return <span data-testid="search">{useLocation().search}</span>;
}

const renderView = () => render(
  <QueryClientProvider client={new QueryClient()}>
    <MemoryRouter initialEntries={["/explore/eng/dashboards/d1"]}>
      <ShowSearch />
      <Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes>
    </MemoryRouter>
  </QueryClientProvider>,
);

describe("DashboardView auto-refresh control", () => {
  it("offers the popover, not a native select", () => {
    renderView();
    expect(screen.getByRole("button", { name: "Auto Refresh" })).toBeInTheDocument();
    // The <select> that was here carried aria-label="auto-refresh".
    expect(screen.queryByRole("combobox", { name: "auto-refresh" })).toBeNull();
  });

  it("picking a period writes ?refresh=<seconds>, keeping egret's existing URL sync", async () => {
    // The URL contract is unchanged by design — a bookmarked ?refresh=300 must keep working.
    const user = userEvent.setup();
    renderView();
    await user.click(screen.getByRole("button", { name: "Auto Refresh" }));
    await user.click(await screen.findByRole("menuitemradio", { name: "5 minutes" }));
    expect(screen.getByTestId("search").textContent).toBe("?refresh=300");
  });
});
