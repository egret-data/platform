import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import DashboardView from "../DashboardView";

vi.mock("@/components/chart/dashboard/DashboardCardView", () => ({ DashboardCardView: () => <div /> }));
vi.mock("@/api/charts", async (orig) => ({
  ...(await orig<typeof import("@/api/charts")>()),
  useCharts: () => ({ data: [] }),
}));

const update = { mutateAsync: vi.fn().mockResolvedValue({}), isPending: false };

// Two headings side by side on the desktop grid. The one-column projection moves the second to
// grid_col 0 and shrinks both to size_x 1 — which is exactly what must never be saved.
const DESKTOP_CARDS = [
  { id: "c1", chart_id: null, card_type: "heading", grid_col: 0, grid_row: 0, size_x: 12, size_y: 1, options: { text: "left" }, tab_id: null },
  { id: "c2", chart_id: null, card_type: "heading", grid_col: 12, grid_row: 0, size_x: 12, size_y: 1, options: { text: "right" }, tab_id: null },
];
const detail = {
  id: "d1", name: "D", description: "", created_by: null, updated_at: "",
  parameters: [], tabs: [], cards: DESKTOP_CARDS, width: "full",
};
vi.mock("@/api/dashboards", async (orig) => {
  const mod = await (orig as () => Promise<Record<string, unknown>>)();
  return { ...mod, useDashboard: () => ({ data: detail, isLoading: false }), useDashboardMutations: () => ({ update }) };
});

// The grid measures its own container with a ResizeObserver. jsdom has neither a real
// clientWidth nor a firing ResizeObserver, so both are driven by hand: `setViewport` changes
// the reported width and then runs every observer callback, which is what a real resize does.
const observers: (() => void)[] = [];
vi.stubGlobal(
  "ResizeObserver",
  class {
    constructor(cb: () => void) {
      observers.push(cb);
    }
    observe() {}
    unobserve() {}
    disconnect() {}
  },
);
function setViewport(px: number) {
  Object.defineProperty(HTMLElement.prototype, "clientWidth", { configurable: true, get: () => px });
}
function resizeTo(px: number) {
  setViewport(px);
  act(() => {
    observers.forEach((cb) => cb());
  });
}

afterEach(() => {
  // Hand clientWidth back to jsdom so this file cannot leak a width into the next one.
  Reflect.deleteProperty(HTMLElement.prototype, "clientWidth");
  observers.length = 0;
});

const renderView = () =>
  render(
    <QueryClientProvider client={new QueryClient()}>
      <MemoryRouter initialEntries={["/explore/eng/dashboards/d1"]}>
        <Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );

function savedCards() {
  const body = update.mutateAsync.mock.calls.at(-1)?.[0] as { cards: { grid_col: number; size_x: number }[] };
  return body.cards.map((c) => ({ grid_col: c.grid_col, size_x: c.size_x }));
}

const DESKTOP_GEOMETRY = [
  { grid_col: 0, size_x: 12 },
  { grid_col: 12, size_x: 12 },
];

describe("editing a dashboard that becomes narrow", () => {
  it("keeps the desktop geometry when the canvas shrinks below the breakpoint mid-edit", () => {
    // Measured with a probe against react-grid-layout 1.5.3: crossing to the mobile breakpoint
    // makes RGL emit onLayoutChange with the derived one-column layout
    // ([["a",0,1],["b",0,1]]). Guarding on `editing` alone lets that through, and the next Save
    // writes grid_col 0 / size_x 1 for every card — the saved desktop layout is gone.
    setViewport(1136);
    update.mutateAsync.mockClear();
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    resizeTo(390);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(savedCards()).toEqual(DESKTOP_GEOMETRY);
  });

  it("keeps the desktop geometry across a shrink and a grow", () => {
    setViewport(1136);
    update.mutateAsync.mockClear();
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    resizeTo(390);
    resizeTo(1136);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(savedCards()).toEqual(DESKTOP_GEOMETRY);
  });

  it("keeps the desktop geometry for a dashboard opened and edited entirely at 390px", () => {
    setViewport(390);
    update.mutateAsync.mockClear();
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(savedCards()).toEqual(DESKTOP_GEOMETRY);
  });
});
