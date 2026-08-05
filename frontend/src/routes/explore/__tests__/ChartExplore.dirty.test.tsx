import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { MemoryRouter, Routes, Route, useNavigate } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { QuerySpec } from "@/api/charts";

/**
 * THE UNSAVED-CHANGES GUARD, TESTED AT THE SEAM WHERE THE DIRTY FLAG IS ACTUALLY DECIDED.
 *
 * `useUnsavedChangesWarning(dirty)` is a three-line hook and `dirty` is one expression in
 * ChartExplore — so a test that mocks either of them proves nothing at all. What decides the flag
 * is the interaction between two effects: the one that seeds builder state from the saved chart,
 * and the one that captures the "clean" baseline the live config is compared against. Only a real
 * mount runs both in the order React runs them, so these tests mount the real route and ask the
 * real `window` whether a `beforeunload` would be cancelled.
 *
 * WHAT THIS PROVES: after a saved chart has loaded and settled, nothing is registered that cancels
 * an unload — and after one genuine edit, something is. The transition is asserted in one test on
 * purpose: asserting only the armed half would pass against the broken code, where the guard is
 * armed from the moment the chart loads.
 *
 * WHAT IT DOES NOT PROVE: that a click *inside the chart canvas* leaves the chart clean. jsdom has
 * no layout and ECharts draws to a canvas it cannot measure, so a legend/plot click is not
 * reachable here. The live check in the PR body covers that half. It also does not prove anything
 * about Chrome's own rule that a `beforeunload` dialog is only *shown* to a user who has interacted
 * with the page — that rule is why the bug looked click-triggered, and it is browser behaviour, not
 * ours: jsdom runs the handler either way, which is exactly what makes the real state observable
 * here with no click at all.
 */

const BASE = {
  id: "c-bar", dataset_id: "d1", name: "Bar v2", description: "", viz_type: "bar",
  created_by: null, updated_at: "2026-07-01T00:00:00Z", folder_id: null,
  params: {
    version: 2,
    spec: { metrics: ["total_revenue"], dimensions: ["region"], columns: [], filters: [], orderby: [{ field: "region", desc: false }], row_limit: 1000, rollup_groups: [] },
    settings: { "cartesian.xAxis": "region", "cartesian.yMetrics": ["total_revenue"] },
  },
};

/**
 * A chart broken out by a TEMPORAL column with no unit stored on it — the shape that settles a
 * second time after hydration, because `withDefaultTemporalUnits` adds the Metabase-style monthly
 * default on the render after the state is seeded. A baseline captured before that write lands
 * makes the chart dirty for a reason the author never touched, so this fixture is the one that
 * tells "baseline taken at hydration" apart from "baseline taken once the load has settled".
 */
const TEMPORAL = {
  ...BASE,
  id: "c-bar-temporal", name: "Bar over time",
  params: {
    version: 2,
    spec: { metrics: ["total_revenue"], dimensions: ["dow"], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] },
    settings: { "cartesian.xAxis": "dow", "cartesian.yMetrics": ["total_revenue"] },
  },
};

/**
 * A table whose stored column state names a column the result does not carry, so the load-time
 * reconcile against the result columns actually fires. It is here as the second load-time
 * write-back, and it is worth saying what it turned out to prove: measured, that write is batched
 * into the same render as hydration, and `paramsForSave` does not serialize `tableState` anyway —
 * so this fixture pins a table mounting clean, not the ordering. See the note on
 * `loadNormalizersSettled`.
 */
const TABLE = {
  ...BASE,
  id: "c-table", name: "Table v2", viz_type: "table",
  params: {
    version: 2,
    spec: { metrics: ["total_revenue"], dimensions: ["region"], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] },
    settings: {
      "table.columns": ["region"], "table.metrics": ["total_revenue"],
      // Names a column the result does not carry, so the reconcile actually fires.
      "table.state": { hiddenColumns: ["gone_column"] },
    },
  },
};

const CHARTS: Record<string, unknown> = {
  "c-bar": BASE, "c-bar-temporal": TEMPORAL, "c-table": TABLE,
};

vi.mock("@/api/charts", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/api/charts")>()),
  useChart: (_ws: string, id: string | null) => ({ data: id ? CHARTS[id] : null, isLoading: false }),
  useChartMutations: () => ({
    create: { mutateAsync: vi.fn(), isPending: false },
    update: { mutateAsync: vi.fn(), isPending: false },
  }),
  useBuildChartData: () => ({ data: undefined, isLoading: false, error: null }),
}));

/**
 * The result carries a column per dimension the spec asked for, plus the metric — i.e. it CHANGES
 * when the spec changes, the way a real result does. A fixed column list would leave the table
 * reconcile's dependencies identical before and after hydration, so that effect would never re-run
 * and the table fixture below would exercise nothing at all.
 */
vi.mock("@/api/useSpecData", () => ({
  useSpecData: (_ws: string, _ds: string | null, spec: QuerySpec | null) => ({
    columns: [
      ...(spec?.dimensions ?? []).map((d) => ({ name: d, type: "varchar" })),
      { name: "total_revenue", type: "double" },
    ],
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
        { column_name: "amount", data_type: "double", is_temporal: false },
        { column_name: "dow", data_type: "timestamp", is_temporal: true },
      ],
      metrics: [{ metric_name: "total_revenue", data_type: "double" }],
    },
    isLoading: false,
  }),
}));

/**
 * Ask the real `window` what a real unload would do: dispatch a cancellable `beforeunload` and
 * report whether anything cancelled it. This is the whole guard, end to end — the hook's
 * `addEventListener`, its `when` argument, and the expression that computes it — with nothing
 * stubbed in between.
 */
function guardArmed(): boolean {
  const e = new Event("beforeunload", { cancelable: true });
  window.dispatchEvent(e);
  return e.defaultPrevented;
}

/** Navigates the editor to another chart id WITHOUT unmounting it — the same thing the route does
 *  when Save on a new chart redirects to /charts/<the id the server gave it>. */
function GoTo({ id }: { id: string }) {
  const navigate = useNavigate();
  return <button onClick={() => navigate(`/explore/demo/charts/${id}`)}>go-to-{id}</button>;
}

async function mountChart(id: string, alsoOffer?: string) {
  const { default: ChartExplore } = await import("../ChartExplore");
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <TooltipProvider>
        <MemoryRouter initialEntries={[`/explore/demo/charts/${id}`]}>
          <Routes>
            <Route
              path="/explore/:workspace/charts/:id"
              element={<>{alsoOffer ? <GoTo id={alsoOffer} /> : null}<ChartExplore /></>}
            />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

describe("ChartExplore unsaved-changes guard", () => {
  it("leaves a freshly loaded chart clean, so nothing cancels an unload", async () => {
    await mountChart("c-bar");
    expect(await screen.findByDisplayValue("1000")).toBeInTheDocument();
    // Let every load-time effect settle before asking — the question is about the state the
    // author is looking at, not about a frame in the middle of loading.
    await waitFor(() => expect(screen.getByLabelText("Row limit")).toHaveValue(1000));
    expect(guardArmed()).toBe(false);
  });

  it("leaves a chart clean when a load-time default lands after hydration", async () => {
    await mountChart("c-bar-temporal");
    // The monthly default is applied to the temporal dimension on load; the picker showing it is
    // the proof that the settling write has landed, so the guard is asked afterwards. Found by
    // its rendered value the way the mount test's unit picker is — a combobox takes no accessible
    // name from its contents.
    await waitFor(() =>
      expect(screen.getAllByRole("combobox").some((c) => c.textContent === "Month")).toBe(true));
    expect(guardArmed()).toBe(false);
  });

  it("leaves a table clean when its stored column state is reconciled on load", async () => {
    await mountChart("c-table");
    expect(await screen.findByDisplayValue("Table v2")).toBeInTheDocument();
    await waitFor(() => expect(screen.getByLabelText("Row limit")).toHaveValue(1000));
    expect(guardArmed()).toBe(false);
  });

  /**
   * The editor is not remounted when the route's :id changes — the same component instance
   * re-seeds itself from the new chart. That is the path Save takes on a NEW chart: it captures a
   * baseline from local state, then redirects to /charts/<the id the server gave it>, and the
   * server's copy comes back through hydration. A baseline left over from before that re-seed
   * belongs to a different chart, so the just-saved chart would report unsaved changes.
   */
  it("re-baselines when a different chart is loaded into the same editor", async () => {
    await mountChart("c-bar", "c-bar-temporal");
    await waitFor(() => expect(screen.getByLabelText("Row limit")).toHaveValue(1000));
    expect(guardArmed()).toBe(false);
    fireEvent.click(screen.getByText("go-to-c-bar-temporal"));
    await waitFor(() =>
      expect(screen.getAllByRole("combobox").some((c) => c.textContent === "Month")).toBe(true));
    expect(guardArmed()).toBe(false);
  });

  it("arms on a real edit — the guard must still do its job", async () => {
    await mountChart("c-bar");
    const rowLimit = await screen.findByLabelText("Row limit");
    await waitFor(() => expect(rowLimit).toHaveValue(1000));
    // The transition is the assertion. Checking only the armed half would pass against the
    // unfixed code, where the guard is already armed before this edit.
    expect(guardArmed()).toBe(false);
    fireEvent.change(rowLimit, { target: { value: "500" } });
    await waitFor(() => expect(rowLimit).toHaveValue(500));
    expect(guardArmed()).toBe(true);
  });
});
