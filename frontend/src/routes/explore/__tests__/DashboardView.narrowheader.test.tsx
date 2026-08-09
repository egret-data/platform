import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import DashboardView from "../DashboardView";

vi.mock("@/components/chart/dashboard/DashboardCardView", () => ({ DashboardCardView: () => <div /> }));
vi.mock("@/api/charts", async (orig) => ({
  ...(await orig<typeof import("@/api/charts")>()),
  useCharts: () => ({ data: [] }),
}));

const update = { mutateAsync: vi.fn().mockResolvedValue({}), isPending: false };
const detail = {
  id: "d1", name: "ZZ Chart Parity", description: "", created_by: null, updated_at: "",
  parameters: [], tabs: [], width: "full",
  cards: [
    { id: "c1", chart_id: null, card_type: "heading", grid_col: 0, grid_row: 0, size_x: 12, size_y: 1, options: { text: "h" }, tab_id: null },
  ],
};
vi.mock("@/api/dashboards", async (orig) => {
  const mod = await (orig as () => Promise<Record<string, unknown>>)();
  return { ...mod, useDashboard: () => ({ data: detail, isLoading: false }), useDashboardMutations: () => ({ update }) };
});

// jsdom cannot lay anything out, so the acceptance measurement (document scrollWidth vs
// innerWidth at 390px) lives in tests/e2e/narrow-viewport-kind.spec.ts. What is pinned here is
// the mechanism: the header row wraps rather than running off the edge.
function stubMatchMedia(matches: boolean) {
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  }));
}

const renderView = () =>
  render(
    <QueryClientProvider client={new QueryClient()}>
      <MemoryRouter initialEntries={["/explore/eng/dashboards/d1"]}>
        <Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );

/** The row holding the title and the view-mode actions. */
function headerRow() {
  return screen.getByRole("heading", { name: "ZZ Chart Parity" }).parentElement!;
}

beforeEach(() => {
  localStorage.clear();
});

describe("the dashboard header row on a narrow viewport", () => {
  it("wraps instead of overflowing, and every action is still rendered", () => {
    stubMatchMedia(true);
    renderView();
    expect(headerRow().className).toContain("flex-wrap");
    // The title takes its own line so the actions get the full width beneath it.
    expect(screen.getByRole("heading", { name: "ZZ Chart Parity" }).className).toContain("basis-full");
    // Wrapping, not hiding: all four view-mode controls are present.
    // "Auto Refresh" is the popover trigger that replaced the native <select> (W4); the
    // <select>'s label was "auto-refresh".
    for (const label of ["add bookmark", "Auto Refresh", "fullscreen", "dashboard info"]) {
      expect(screen.getByLabelText(label)).toBeInTheDocument();
    }
  });

  it("leaves the desktop row unwrapped", () => {
    stubMatchMedia(false);
    renderView();
    expect(headerRow().className).not.toContain("flex-wrap");
    expect(screen.getByRole("heading", { name: "ZZ Chart Parity" }).className).not.toContain("basis-full");
  });
});
