import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import DashboardView from "../DashboardView";

vi.mock("sonner", () => ({ toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn() }) }));
import { toast } from "sonner";

vi.mock("@/components/chart/dashboard/DashboardCardView", () => ({
  DashboardCardView: ({ card, chart }: { card: { id: string }; chart?: { name: string } }) =>
    <div data-testid={`card-${card.id}`}>{chart?.name ?? "-"}</div>,
}));

const spec = { dimensions: ["country"], columns: [], metrics: ["n"], filters: [], temporal_units: {} };
const CHARTS = [
  { id: "ch1", name: "Orders", viz_type: "bar", dataset_id: "ds1", params: { spec } },
  { id: "ch2", name: "Revenue", viz_type: "line", dataset_id: "ds1", params: { spec } },
];
vi.mock("@/api/charts", async (orig) => ({
  ...(await orig<typeof import("@/api/charts")>()),
  useCharts: () => ({ data: CHARTS }),
}));
vi.mock("@/api/useChartData", () => ({ useChartData: () => ({ columns: [], rows: [] }) }));

const detail = {
  id: "d1", name: "D", description: "", created_by: null, updated_at: "", parameters: [], tabs: [], width: "full",
  cards: [
    { id: "c1", chart_id: "ch1", card_type: "chart", grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: { hideEmpty: true }, tab_id: null },
  ],
};
vi.mock("@/api/dashboards", async (orig) => {
  const mod = await (orig as () => Promise<Record<string, unknown>>)();
  return { ...mod,
    useDashboard: () => ({ data: detail, isLoading: false }),
    useDashboardMutations: () => ({ update: { mutateAsync: vi.fn(), isPending: false } }),
  };
});

const renderView = () => render(
  <QueryClientProvider client={new QueryClient()}>
    <MemoryRouter initialEntries={["/explore/eng/dashboards/d1"]}>
      <Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes>
    </MemoryRouter>
  </QueryClientProvider>,
);

const lastToast = () => (toast as unknown as { mock: { calls: unknown[][] } }).mock.calls.at(-1)!;

beforeEach(() => vi.clearAllMocks());

describe("DashboardView replace-chart undo", () => {
  it("replacing a chart offers an Undo that restores both the chart and the options it cleared", async () => {
    // Clearing `options` on replace is NOT the divergence — design §9 records that Metabase
    // discards more than egret does. What was missing is the way back, which is this toast, and
    // the assertion on `hideEmpty` is what makes the undo about the whole card rather than the id.
    const user = userEvent.setup();
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    expect(screen.getByTestId("card-c1").textContent).toBe("Orders");

    await user.click(screen.getByRole("button", { name: "replace card" }));
    await user.click(await screen.findByRole("button", { name: "Revenue" }));
    expect(screen.getByTestId("card-c1").textContent).toBe("Revenue");

    const [message, cfg] = lastToast();
    expect(message).toBe("Chart replaced");
    act(() => (cfg as { action: { onClick: () => void } }).action.onClick());
    expect(screen.getByTestId("card-c1").textContent).toBe("Orders");

    // …and the draft is byte-identical to what Edit started from, options included.
    fireEvent.click(screen.getAllByRole("button", { name: "Cancel" })
      .find((b) => b.closest("div")?.textContent?.includes("Save"))!);
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });
});
