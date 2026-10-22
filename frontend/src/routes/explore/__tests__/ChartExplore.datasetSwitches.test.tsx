import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { QuerySpec } from "@/api/charts";

/**
 * The dataset's switches, read by the chart route (B2).
 *
 * KI-096: a column with **Dimension** off was still offered as an X axis — the switch was stored
 * and read by nothing. Driven through the ROUTE, because the fix is which columns the route hands
 * the viz controls; a unit test of the filter would stay green with nothing calling it.
 *
 * KI-111: a chart whose dataset is in the Trash ran on with nothing saying so.
 */

const BAR = {
  id: "c-bar", dataset_id: "d1", name: "Bar", description: "", viz_type: "bar",
  params: {
    version: 2,
    spec: {
      metrics: ["revenue"], dimensions: ["legacy"], columns: [], filters: [], orderby: [],
      row_limit: 100, rollup_groups: [],
    },
    // Saved BEFORE `legacy` had Dimension switched off: it must keep rendering and keep showing.
    settings: { "cartesian.xAxis": "legacy", "cartesian.yMetrics": ["revenue"] },
  },
  created_by: null, updated_at: "2026-07-01T00:00:00Z", folder_id: null,
};

vi.mock("@/api/charts", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/api/charts")>()),
  useChart: () => ({ data: BAR, isLoading: false }),
  useChartMutations: () => ({
    create: { mutateAsync: vi.fn(), isPending: false },
    update: { mutateAsync: vi.fn(), isPending: false },
  }),
  useBuildChartData: () => ({ data: undefined, isLoading: false, error: null }),
}));

vi.mock("@/api/useSpecData", () => ({
  useSpecData: (_ws: string, _ds: string | null, spec: QuerySpec | null) => {
    const names = spec ? [...(spec.dimensions ?? []), ...(spec.metrics ?? [])] : [];
    return {
      columns: names.map((n) => ({ name: n, type: n === "revenue" ? "double" : "varchar" })),
      rows: names.length ? [names.map((n) => (n === "revenue" ? 1 : `${n}-v`))] : [],
      isLoading: false, error: null, hasResult: !!spec,
    };
  },
}));

const dataset = { deleted_at: null as string | null };
const c = (column_name: string, groupby: boolean) => ({
  column_name, data_type: "varchar", type_generic: "string", is_temporal: false, verbose_name: "",
  groupby, filterable: true, format: "", semantic_type: "",
});
vi.mock("@/api/datasets", () => ({
  useDatasets: () => ({ data: [{ id: "d1", name: "Orders" }], isLoading: false }),
  useDataset: () => ({
    data: {
      id: "d1", name: "Orders", deleted_at: dataset.deleted_at,
      columns: [c("region", true), c("orderkey", false), c("legacy", false)],
      metrics: [{ metric_name: "revenue", data_type: "double", verbose_name: "", format: "" }],
    },
    isLoading: false,
  }),
}));

async function mount() {
  const { default: ChartExplore } = await import("../ChartExplore");
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <TooltipProvider>
        <MemoryRouter initialEntries={["/explore/demo/charts/c-bar"]}>
          <Routes>
            <Route path="/explore/:workspace/charts/:id" element={<ChartExplore />} />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

describe("ChartExplore and the dataset's switches", () => {
  it("offers only Dimension-on columns as the X axis, plus the one the chart already uses (KI-096)", { timeout: 20_000 }, async () => {
    dataset.deleted_at = null;
    await mount();
    // The X-axis trigger shows the chart's current pick.
    const xAxis = (await screen.findAllByRole("combobox")).find((el) => el.textContent === "legacy");
    expect(xAxis, "the X-axis picker still shows the chart's own pick").toBeTruthy();
    await userEvent.click(xAxis!);
    const options = screen.getAllByRole("option").map((o) => o.textContent);
    expect(options).toContain("region");
    expect(options).toContain("legacy");
    expect(options).not.toContain("orderkey");
    expect(screen.queryByText(/is in the Trash/)).toBeNull();
  });

  it("says when the chart's dataset is in the Trash (KI-111)", { timeout: 20_000 }, async () => {
    dataset.deleted_at = "2026-10-21T00:00:00Z";
    await mount();
    expect(await screen.findByText(/dataset “Orders” is in the Trash\. The chart keeps working until the dataset is deleted/)).toBeInTheDocument();
  });
});
