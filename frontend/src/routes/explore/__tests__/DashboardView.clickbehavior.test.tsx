import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import DashboardView from "../DashboardView";

const dashboard = {
  id: "d1", name: "Ops", description: "", created_by: null, updated_at: "",
  parameters: [{ id: "pr", name: "Region", slug: "region", type: "text", operator: "is" }],
  cards: [
    { id: "k1", chart_id: "c1", card_type: "chart", grid_col: 0, grid_row: 0, size_x: 6, size_y: 4,
      options: { clickBehavior: { type: "crossfilter", parameterMapping: { pr: "region" } } } },
    { id: "k2", chart_id: "c1", card_type: "chart", grid_col: 0, grid_row: 4, size_x: 6, size_y: 4,
      options: { clickBehavior: { type: "menu" } } },
  ],
};
vi.mock("@/api/dashboards", () => ({
  useDashboard: () => ({ data: dashboard, isLoading: false }),
  useDashboardMutations: () => ({ update: { mutateAsync: vi.fn(), isPending: false }, revert: { mutateAsync: vi.fn(), isPending: false } }),
  // The info sidebar's History tab query; disabled until the tab is opened, so an empty result is
  // the right stub for every test that never opens it.
  useDashboardRevisions: () => ({ data: [], isLoading: false }),
  // The view-mode "…" menu reads the dashboard's folder from the LIST query. (W8 added folder_id
  // to the detail DTO for the info sidebar's "Saved in" row, but the menu still reads the list.)
  // An empty list leaves it folderless.
  useDashboards: () => ({ data: [] }),
}));
vi.mock("@/api/charts", async (orig) => ({
  ...(await orig<typeof import("@/api/charts")>()),
  useCharts: () => ({ data: [{ id: "c1", dataset_id: "ds1", name: "Sales", viz_type: "table", description: "",
    params: { spec: { metrics: ["rev"], dimensions: ["region"], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] } },
    created_by: null, updated_at: "" }] }),
}));
vi.mock("@/api/datasets", () => ({
  useDatasetsFilterOff: () => new Map(),
  // The card's dataset names `region` "Khu vực" (KI-290).
  useDatasetDetails: () => new Map([["ds1", { columns: [{ column_name: "region", verbose_name: "Khu vực", data_type: "varchar" }], metrics: [] }]]),
  useDataset: (_ws: string, id: string | null) => ({
    data: id
      ? { columns: [{ column_name: "region", data_type: "varchar" }, { column_name: "revenue", data_type: "double" }] }
      : undefined,
  }),
}));
vi.mock("@/components/chart/dashboard/DashboardCardView", () => ({
  DashboardCardView: ({ card, onValueClick, datasetDefaults }: { card: { id: string }; onValueClick?: (ctx: unknown, x: number, y: number) => void; datasetDefaults?: unknown }) =>
    <button data-testid={`cardclick-${card.id}`} data-defaults={JSON.stringify(datasetDefaults ?? null)}
      onClick={() => onValueClick?.({ dims: [{ col: "region", value: "EU" }] }, 0, 0)} />,
}));
vi.mock("@/components/chart/DrillRecordsDialog", () => ({
  DrillRecordsDialog: ({ open, columns }: { open: boolean; columns: { name: string }[] }) =>
    open ? <div data-testid="records-columns">{columns.map((c) => c.name).join(",")}</div> : null,
}));

function renderAt() {
  return render(
    <QueryClientProvider client={new QueryClient()}>
      <MemoryRouter initialEntries={["/explore/eng/dashboards/d1"]}>
        <Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("DashboardView click behaviour", () => {
  it("cross-filters when a card's click behaviour is crossfilter", () => {
    renderAt();
    fireEvent.click(screen.getByTestId("cardclick-k1"));
    // The region filter is now a pill; the crossfilter value shows in its summary.
    expect(screen.getByLabelText("Region").textContent).toContain("EU");
  });

  it("passes the dataset's columns into the See-these-records dialog", () => {
    renderAt();
    // menu-type card → opens DrillMenu
    fireEvent.click(screen.getByTestId("cardclick-k2"));
    // click "See these records" in the drill menu
    fireEvent.click(screen.getByText("See these records"));
    // regression: dialog must receive the dataset's columns, not an empty array
    expect(screen.getByTestId("records-columns")).toHaveTextContent("region,revenue");
  });

  // KI-290: the card and its drill menu name a column as the chart page does.
  it("hands each card its dataset's display names", () => {
    renderAt();
    const defaults = JSON.parse(screen.getByTestId("cardclick-k2").getAttribute("data-defaults")!);
    expect(defaults.region.column_title).toBe("Khu vực");
  });

  it("names the drilled column by its display name in the drill menu", () => {
    renderAt();
    fireEvent.click(screen.getByTestId("cardclick-k2"));
    expect(screen.getByText(/Filter by/).textContent).toContain("Khu vực");
    expect(screen.getByText(/Filter by/).textContent).not.toContain("region");
  });
});
