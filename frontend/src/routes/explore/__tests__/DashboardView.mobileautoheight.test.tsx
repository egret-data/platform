import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import DashboardView from "../DashboardView";
import { contentRowsFor, gridRowHeight } from "@/components/chart/dashboard-grid";

// DashboardCardView is deliberately NOT mocked here: the auto-height writer under test lives
// behind it, in AutoHeightMeasure. A text card needs no network.
vi.mock("@/api/charts", async (orig) => ({
  ...(await orig<typeof import("@/api/charts")>()),
  useCharts: () => ({ data: [] }),
}));

const update = { mutateAsync: vi.fn().mockResolvedValue({}), isPending: false };

const detail = {
  id: "d1", name: "D", description: "", created_by: null, updated_at: "",
  parameters: [], tabs: [], width: "full",
  cards: [
    { id: "c1", chart_id: null, card_type: "text", grid_col: 0, grid_row: 0, size_x: 12, size_y: 1, options: { text: "a paragraph", autoHeight: true }, tab_id: null },
  ],
};
vi.mock("@/api/dashboards", async (orig) => {
  const mod = await (orig as () => Promise<Record<string, unknown>>)();
  return { ...mod, useDashboard: () => ({ data: detail, isLoading: false }), useDashboardMutations: () => ({ update }) };
});

// jsdom lays nothing out: clientWidth and scrollHeight are both 0 everywhere, and
// AutoHeightMeasure bails on a zero measurement. Both have to be supplied by hand.
let clientWidth = 1136;
let scrollHeight = 500;
Object.defineProperty(HTMLElement.prototype, "clientWidth", { configurable: true, get: () => clientWidth });
Object.defineProperty(HTMLElement.prototype, "scrollHeight", { configurable: true, get: () => scrollHeight });

// Every ResizeObserver in the tree lands here, in creation order — which is the order a browser
// delivers them in. That order is the whole point: the card's observer and the grid's observer
// fire in the SAME batch, before React re-renders either of them.
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

function withGeometry(width: number, height = 500) {
  clientWidth = width;
  scrollHeight = height;
}

/** One browser resize: the layout changes, then every observer is delivered. */
function resizeTo(width: number, height: number) {
  clientWidth = width;
  scrollHeight = height;
  act(() => {
    observers.forEach((cb) => cb());
  });
}

afterEach(() => {
  observers.length = 0;
  clientWidth = 1136;
  scrollHeight = 500;
});

const renderView = () =>
  render(
    <QueryClientProvider client={new QueryClient()}>
      <MemoryRouter initialEntries={["/explore/eng/dashboards/d1"]}>
        <Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );

function savedHeight() {
  const body = update.mutateAsync.mock.calls.at(-1)?.[0] as { cards: { size_y: number }[] };
  return body.cards[0].size_y;
}

describe("auto-height on a narrow canvas", () => {
  it("does not write a mobile-measured height back into the card", () => {
    // A card is a different width on a phone, so the height its content needs there belongs to
    // a layout nobody is editing. Writing it back corrupts the desktop rows.
    withGeometry(390);
    update.mutateAsync.mockClear();
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(savedHeight()).toBe(1);
  });

  it("still writes the measured height on a desktop canvas", () => {
    // The mirror: the guard must not disable auto-height outright. 500px of content over a
    // 1136px canvas is 11 rows, not the 1 the card was stored with.
    withGeometry(1136);
    update.mutateAsync.mockClear();
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(savedHeight()).toBeGreaterThan(1);
  });

  it("does not adopt the height the card needs once the canvas shrinks mid-edit", () => {
    // The ordering case, and the reason the route reads the breakpoint from a ref that the grid
    // writes inside its own observer. A card's ResizeObserver and the grid's fire in one batch
    // BEFORE React re-renders, so a breakpoint carried by state or by a render closure still
    // says "desktop" while the card is already measuring itself at phone width.
    const desktopRows = contentRowsFor(500, gridRowHeight(1136));
    const mobileRows = contentRowsFor(900, gridRowHeight(390));
    expect(mobileRows).not.toBe(desktopRows); // otherwise this test could not tell them apart

    withGeometry(1136, 500);
    update.mutateAsync.mockClear();
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    // Narrower card, so the same text needs far more height — the measurement that must not land.
    resizeTo(390, 900);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(savedHeight()).toBe(desktopRows);
  });
});
