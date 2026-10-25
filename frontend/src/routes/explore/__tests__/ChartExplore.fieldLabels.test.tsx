import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { QuerySpec } from "@/api/charts";
import ChartExplore from "../ChartExplore";

/**
 * KI-050: the editor's Sort menu names a field by the dataset's display name, as the field pickers
 * and the axis titles do, and the clause it adds still stores the machine name.
 */

const { updateMutate, createMutate } = vi.hoisted(() => ({ updateMutate: vi.fn(), createMutate: vi.fn() }));

const TABLE = {
  id: "c-raw-table", dataset_id: "d1", name: "Raw table", description: "", viz_type: "table",
  params: {
    version: 2,
    spec: {
      metrics: [], dimensions: [], columns: ["id", "state", "vendor"], filters: [], orderby: [],
      row_limit: 1000, rollup_groups: [],
    },
    settings: { "table.columns": ["id", "state", "vendor"], "table.metrics": [] },
  },
  created_by: null, updated_at: "2026-07-01T00:00:00Z", folder_id: null,
};

/**
 * The same table with junk where its column state should be.
 *
 * `null` is not a shape this editor can write — but the backend asserts only that
 * `params.settings` is a JSON object (`crates/egret-bi/src/chartparams.rs`, and a test pins that
 * permissiveness), so a row written by anything else can carry it, which is the same reason
 * `reconcileTableState` tolerates malformed input. Hydration reads it through `shellSettings`,
 * whose `?? {}` turns it into the empty state — so this is the one reachable fixture where the
 * live state is at its default while the loaded `settings` blob still holds a value under the
 * same key. It is what tells "omit the key" apart from "leave whatever was there".
 */
const TABLE_JUNK = {
  ...TABLE,
  id: "c-junk", name: "Junk state",
  params: { ...TABLE.params, settings: { ...TABLE.params.settings, "table.state": null } },
};

/** The same table with an order ALREADY STORED — the drag cannot be driven in jsdom, so the order
 *  under test arrives this way. See the note inside the test that uses it. */
const TABLE_ORDERED = {
  ...TABLE,
  id: "c-ordered",
  params: {
    ...TABLE.params,
    settings: { ...TABLE.params.settings, "table.state": { columnOrder: ["state", "id", "vendor"] } },
  },
};

const CHARTS: Record<string, unknown> = {
  "c-raw-table": TABLE, "c-junk": TABLE_JUNK, "c-ordered": TABLE_ORDERED,
};

vi.mock("@/api/charts", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/api/charts")>()),
  useChart: (_ws: string, id: string | null) => ({ data: id ? CHARTS[id] : null, isLoading: false }),
  useChartMutations: () => ({
    create: { mutateAsync: createMutate, isPending: false },
    update: { mutateAsync: updateMutate, isPending: false },
  }),
  useBuildChartData: () => ({ data: undefined, isLoading: false, error: null }),
}));

const TYPES: Record<string, string> = { id: "bigint", state: "varchar", vendor: "varchar" };

/** The engine's contract: the result carries exactly the columns the spec projected. */
vi.mock("@/api/useSpecData", () => ({
  useSpecData: (_ws: string, _ds: string | null, spec: QuerySpec | null) => {
    const names = spec ? [...(spec.dimensions ?? []), ...(spec.columns ?? []), ...(spec.metrics ?? [])] : [];
    return {
      columns: names.map((n) => ({ name: n, type: TYPES[n] ?? "varchar" })),
      rows: names.length ? [names.map((n) => (n === "id" ? 1 : `${n}-value`))] : [],
      isLoading: false, error: null, hasResult: !!spec,
    };
  },
}));

vi.mock("@/api/datasets", () => ({
  useDatasets: () => ({ data: [{ id: "d1", name: "Demo Sales" }], isLoading: false }),
  useDataset: () => ({
    data: {
      id: "d1", name: "Demo Sales",
      columns: [
        { column_name: "id", data_type: "bigint", is_temporal: false },
        { column_name: "state", data_type: "varchar", is_temporal: false, verbose_name: "State name" },
        { column_name: "vendor", data_type: "varchar", is_temporal: false },
      ],
      metrics: [],
    },
    isLoading: false,
  }),
}));

async function mountAt(path: string) {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <TooltipProvider>
        <MemoryRouter initialEntries={[path]}>
          <Routes>
            <Route path="/explore/:workspace/charts/new" element={<ChartExplore />} />
            <Route path="/explore/:workspace/charts/:id" element={<ChartExplore />} />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}


describe("the Sort menu in the chart editor", () => {
  it("lists the dataset's display names", async () => {
    await mountAt("/explore/demo/charts/c-raw-table?tab=data");
    await userEvent.click(await screen.findByRole("combobox", { name: /add a sort/i }));
    const options = screen.getAllByRole("option").map((o) => o.textContent);
    expect(options).toContain("State name");
    expect(options).not.toContain("state");
  }, 30000);
});
