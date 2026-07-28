import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";

// A saved pie chart in the pre-A2 (v1) shape: a viz still on the bridge, i.e. driven by
// ChartExplore's inline branches rather than a descriptor.
const PIE_V1 = {
  id: "c-pie", dataset_id: "d1", name: "Pie v1", description: "", viz_type: "pie",
  params: {
    spec: { metrics: ["total_revenue"], dimensions: ["category"], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] },
    pie: { dimension: "category", metric: "total_revenue" },
  },
  created_by: null, updated_at: "2026-07-01T00:00:00Z", folder_id: null,
};

// A migrated viz in the v2 shape, to prove both paths mount.
const BAR_V2 = {
  ...PIE_V1,
  id: "c-bar", name: "Bar v2", viz_type: "bar",
  params: {
    version: 2,
    spec: { metrics: ["total_revenue"], dimensions: ["region"], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] },
    settings: { "cartesian.xAxis": "region", "cartesian.yMetrics": ["total_revenue"] },
  },
};

const CHARTS: Record<string, unknown> = { "c-pie": PIE_V1, "c-bar": BAR_V2 };

vi.mock("@/api/charts", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/api/charts")>()),
  useChart: (_ws: string, id: string | null) => ({ data: id ? CHARTS[id] : null, isLoading: false }),
  useChartMutations: () => ({
    create: { mutateAsync: vi.fn(), isPending: false },
    update: { mutateAsync: vi.fn(), isPending: false },
  }),
  useBuildChartData: () => ({ data: undefined, isLoading: false, error: null }),
}));

vi.mock("@/api/useSpecData", () => ({
  useSpecData: () => ({ columns: [], rows: [], isLoading: false, error: null, hasResult: false }),
}));

vi.mock("@/api/datasets", () => ({
  useDatasets: () => ({ data: [{ id: "d1", name: "Demo Sales" }], isLoading: false }),
  useDataset: () => ({
    data: {
      id: "d1", name: "Demo Sales",
      columns: [
        { column_name: "region", data_type: "varchar", is_temporal: false },
        { column_name: "category", data_type: "varchar", is_temporal: false },
      ],
      metrics: [{ metric_name: "total_revenue", data_type: "double" }],
    },
    isLoading: false,
  }),
}));

async function mountChart(id: string) {
  const { default: ChartExplore } = await import("../ChartExplore");
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <TooltipProvider>
        <MemoryRouter initialEntries={[`/explore/demo/charts/${id}`]}>
          <Routes>
            <Route path="/explore/:workspace/charts/:id" element={<ChartExplore />} />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

describe("ChartExplore mounts", () => {
  // Regression guard: the run hook sits above the helpers it calls, so any of them written
  // as a `const` arrow puts the shell in a temporal dead zone and the whole route white-
  // screens. Cartesian never noticed (its descriptor answers instead) — a bridge viz did.
  it("renders a bridge viz (pie) saved in the v1 shape", async () => {
    await mountChart("c-pie");
    expect(await screen.findByDisplayValue("Pie v1")).toBeInTheDocument();
    expect(screen.getByText("Dimension")).toBeInTheDocument();
  });

  it("renders a migrated viz (bar) saved in the v2 shape", async () => {
    await mountChart("c-bar");
    expect(await screen.findByDisplayValue("Bar v2")).toBeInTheDocument();
    expect(screen.getByText("X-axis")).toBeInTheDocument();
  });
});
