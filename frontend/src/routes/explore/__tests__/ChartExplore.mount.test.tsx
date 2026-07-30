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

const TABLE_V2 = {
  ...PIE_V1,
  id: "c-table", name: "Table v2", viz_type: "table",
  params: {
    version: 2,
    spec: { metrics: ["total_revenue"], dimensions: ["region"], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] },
    settings: { "table.columns": ["region"], "table.metrics": ["total_revenue"] },
  },
};

const CHARTS: Record<string, unknown> = { "c-pie": PIE_V1, "c-bar": BAR_V2, "c-table": TABLE_V2 };

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
  useSpecData: () => ({
    columns: [{ name: "region", type: "varchar" }, { name: "total_revenue", type: "double" }],
    rows: [["North", 1]],
    isLoading: false, error: null, hasResult: true,
  }),
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

async function mountChart(id: string, tab?: "display" | "rules") {
  const { default: ChartExplore } = await import("../ChartExplore");
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <TooltipProvider>
        <MemoryRouter initialEntries={[`/explore/demo/charts/${id}${tab ? `?tab=${tab}` : ""}`]}>
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
    // "Dimensions" plural since chart 05 — a pie takes up to three, drawn as nested rings.
    expect(screen.getByText("Dimensions")).toBeInTheDocument();
  });

  it("renders a migrated viz (bar) saved in the v2 shape", async () => {
    await mountChart("c-bar");
    expect(await screen.findByDisplayValue("Bar v2")).toBeInTheDocument();
    expect(screen.getByText("X-axis")).toBeInTheDocument();
  });

  // A2 PR1/PR2 replaced ChartSettingsSections with the descriptor's controls for migrated
  // viz types and silently dropped the shared display sections with it. They are shell-owned
  // settings, so they must show up next to the descriptor's own controls.
  it("keeps the shared formatting sections on a migrated viz's Display tab", async () => {
    await mountChart("c-bar", "display");
    expect(await screen.findByText("Formatting")).toBeInTheDocument();
  });

  // The other half of "the chart card uses the height it is given" (see PreviewPane's own
  // test): the card's `flex-1` only resolves to pixels if the column it sits in is a flex
  // column with a definite height. jsdom does no layout, so this pins the chain that
  // produces the height rather than the height itself.
  it("lays the preview column out as a flex column so the card can grow into it", async () => {
    await mountChart("c-bar");
    // The sr-only title is the column's first child, so it names the column without needing
    // a run to have finished.
    const column = (await screen.findByRole("heading", { name: "Bar v2" })).parentElement!;
    expect(column.className).toContain("flex-col");
    expect(column.className).toContain("flex-1");
    // The row above it is what makes that column's own height definite.
    expect(column.parentElement!.className).toContain("h-full");
    expect(column.parentElement!.className).toContain("min-h-0");
    // PreviewPane's output lands directly in that column — nothing in between re-introduces
    // an auto-height wrapper that would swallow the card's `flex-1`.
    expect(screen.getByText(/^Set up your/).parentElement!.parentElement).toBe(column);
  });

  it("keeps the table's column section on the Display tab", async () => {
    await mountChart("c-table", "display");
    expect(await screen.findByText("Columns")).toBeInTheDocument();
    // Conditional formatting moved to its own tab — stacked in here it made the panel long
    // enough to read as cut off, which is what Metabase avoids by splitting the two.
    expect(screen.queryByText("Conditional formatting")).not.toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Colors" })).toBeInTheDocument();
  });

  it("puts conditional formatting on its own tab", async () => {
    await mountChart("c-table", "rules");
    expect(await screen.findByText("Conditional formatting")).toBeInTheDocument();
  });
});
