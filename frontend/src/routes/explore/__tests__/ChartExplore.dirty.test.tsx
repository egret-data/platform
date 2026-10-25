import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { MemoryRouter, Routes, Route, useNavigate } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { QuerySpec } from "@/api/charts";
import ChartExplore from "../ChartExplore";

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
 * reconcile against the result columns actually fires — the second load-time write-back, and the
 * one whose ordering against the baseline decides whether a table loads clean.
 *
 * Mounted TWICE below, and the pair is the point. With the result already present (the mock's
 * default) the reconcile batches into the hydration flush and the chart is clean whatever the
 * baseline gate says — that is the arrangement this file used to have, and it could not see the
 * bug. With the result arriving a beat later (`ctl.lateResult`, which is what a real query does)
 * the prune lands after hydration, and only a baseline that waited for it stays clean.
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
 * Per-test switches for the query mock, hoisted so the module factory can read them. Set BEFORE
 * mounting; the hook reads each one once, at mount.
 */
const ctl = vi.hoisted(() => ({ lateResult: false, failed: false }));

/**
 * The result carries a column per dimension the spec asked for, plus the metric — i.e. it CHANGES
 * when the spec changes, the way a real result does. A fixed column list would leave the table
 * reconcile's dependencies identical before and after hydration, so that effect would never re-run
 * and the table fixture below would exercise nothing at all.
 *
 * WHEN it arrives is the other half. By default it is already there on the first render, which no
 * real query ever is — the request has not even been sent at that point — and that instantaneous
 * arrival is precisely what hides an ordering bug between the result and the baseline: the prune it
 * triggers batches into the hydration flush and lands before anything can observe it. `lateResult`
 * pushes the arrival one macrotask out, which is enough to put it in a later flush than hydration
 * and is the shape a browser always has. `failed` is the third case: a query that comes back with
 * an error, so no result is ever coming.
 */
vi.mock("@/api/useSpecData", async () => {
  const { useState, useEffect } = await import("react");
  return {
    useSpecData: (_ws: string, _ds: string | null, spec: QuerySpec | null) => {
      const [arrived, setArrived] = useState(!ctl.lateResult && !ctl.failed);
      const [failed] = useState(ctl.failed);
      useEffect(() => {
        if (arrived || failed) return;
        const t = setTimeout(() => setArrived(true), 0);
        return () => clearTimeout(t);
      }, []);
      if (!arrived) {
        return { columns: [], rows: [], isLoading: !failed, error: failed ? "boom" : null, hasResult: false };
      }
      return {
        columns: [
          ...(spec?.dimensions ?? []).map((d) => ({ name: d, type: "varchar" })),
          { name: "total_revenue", type: "double" },
        ],
        rows: [["North", 1]],
        isLoading: false, error: null, hasResult: true,
      };
    },
  };
});

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
  // Back to "the result is already there" for every test that does not ask otherwise.
  afterEach(() => { ctl.lateResult = false; ctl.failed = false; });

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
    // the proof that the settling write has landed, so the guard is asked afterwards. Matched on
    // the trigger CONTAINING "Month" — it reads `<column>: Month` now that the bucket is named on
    // the column itself, so an equality check on the whole string no longer holds.
    await waitFor(() =>
      expect(screen.getAllByRole("combobox").some((c) => /Month/.test(c.textContent ?? ""))).toBe(true));
    expect(guardArmed()).toBe(false);
  });

  it("leaves a table clean when its stored column state is reconciled on load", async () => {
    await mountChart("c-table");
    expect(await screen.findByDisplayValue("Table v2")).toBeInTheDocument();
    await waitFor(() => expect(screen.getByLabelText("Row limit")).toHaveValue(1000));
    expect(guardArmed()).toBe(false);
  });

  /**
   * THE SAME TABLE, with the result arriving when a result actually arrives — after hydration.
   *
   * The prune itself is deliberately invisible: `reconcileTableState` only ever drops names the
   * result no longer carries, and dropping them changes neither what is drawn nor its order (that
   * is the whole reason the leak went unnoticed for so long). So the observable IS the dirty flag,
   * and what makes this test mean something is the ORDER: the assertion is taken after the
   * rendered result proves the query landed, which is one commit after the reconcile effect that
   * commit scheduled. Confirmed by mutation — dropping `tableStateSettled` from the baseline gate
   * fails this test and nothing else in the suite.
   */
  it("leaves a table clean when the prune lands after hydration, then arms on a real edit", async () => {
    ctl.lateResult = true;
    await mountChart("c-table");
    expect(await screen.findByDisplayValue("Table v2")).toBeInTheDocument();
    // The drawn header is the proof the result arrived: before it there are no columns at all.
    await waitFor(() => expect(screen.getAllByRole("columnheader").length).toBeGreaterThan(0));
    expect(guardArmed()).toBe(false);
    // The transition, and not the clean half alone: a baseline that is NEVER taken also reads as
    // clean, so "waits for the prune" and "waits forever" are indistinguishable without this. It
    // is the same reason the row-limit test below asserts a transition rather than an end state.
    const rowLimit = screen.getByLabelText("Row limit");
    fireEvent.change(rowLimit, { target: { value: "500" } });
    await waitFor(() => expect(rowLimit).toHaveValue(500));
    expect(guardArmed()).toBe(true);
  });

  /**
   * …and the other end of that wait. A baseline that waits for a result which is never coming is a
   * guard that never arms, which loses an edit silently — strictly worse than the spurious prompt
   * the waiting exists to prevent. A failed query ends the wait.
   */
  it("still arms after a real edit when the query failed and no result is coming", async () => {
    ctl.failed = true;
    await mountChart("c-table");
    const rowLimit = await screen.findByLabelText("Row limit");
    await waitFor(() => expect(rowLimit).toHaveValue(1000));
    expect(guardArmed()).toBe(false);
    fireEvent.change(rowLimit, { target: { value: "500" } });
    await waitFor(() => expect(rowLimit).toHaveValue(500));
    expect(guardArmed()).toBe(true);
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
      expect(screen.getAllByRole("combobox").some((c) => /Month/.test(c.textContent ?? ""))).toBe(true));
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
