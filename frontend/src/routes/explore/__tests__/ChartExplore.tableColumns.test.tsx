import { describe, it, expect, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { QuerySpec } from "@/api/charts";

/**
 * THE THREE-STEP SEQUENCE, driven through the route that owns both lists.
 *
 * egret has two column lists on one table and never reconciles them: `Data ▸ Columns` decides what
 * the QUERY projects, `Display ▸ Visible columns` decides what the RESULT draws. Hiding a column in
 * the second and then removing it from the first leaves a stale name in `hiddenColumns`, so putting
 * the column back in the query brings back a column the table still refuses to draw — with the Data
 * tab's checkbox ticked. The user is given no signal that the fix is in another tab.
 *
 * Asserted as a SEQUENCE, after every step, and not on the end state alone. An end-state test
 * ("re-add it and it shows") is satisfiable by a build that simply never hides anything, so it
 * passes against the broken code the moment step 1 silently no-ops. Step 2's assertion — the column
 * really is hidden — is what makes step 3's assertion mean something.
 *
 * Asserted on the ROUTE and not on a helper: the fix is a write-back, and a write-back that nothing
 * calls is a green unit test over a dead code path. This file is the only place that proves
 * `ChartExplore` runs it. `table-state.test.ts` pins the pruning rules themselves.
 */

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
 * The same table with `vendor` RENAMED in the panel. Its header, and its row in the Visible-columns
 * list, both read "Supplier"; the column itself is still `vendor` and every stored list still keys
 * on `vendor`. Split out as its own fixture so the sequence above stays readable, and used by the
 * second test — which is the only assertion that can tell a reconcile keyed on the raw name from
 * one keyed on the title, since keying on the title prunes a flag that should have been kept.
 */
const TABLE_RENAMED = {
  ...TABLE,
  id: "c-renamed", name: "Renamed table",
  params: {
    ...TABLE.params,
    settings: { ...TABLE.params.settings, column_settings: { vendor: { column_title: "Supplier" } } },
  },
};

const CHARTS: Record<string, unknown> = { "c-raw-table": TABLE, "c-renamed": TABLE_RENAMED };

vi.mock("@/api/charts", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/api/charts")>()),
  useChart: (_ws: string, id: string | null) => ({ data: id ? CHARTS[id] : null, isLoading: false }),
  useChartMutations: () => ({
    create: { mutateAsync: vi.fn(), isPending: false },
    update: { mutateAsync: vi.fn(), isPending: false },
  }),
  useBuildChartData: () => ({ data: undefined, isLoading: false, error: null }),
}));

const TYPES: Record<string, string> = { id: "bigint", state: "varchar", vendor: "varchar" };

/**
 * The engine's contract, not a fixture: the result carries exactly the columns the spec projected.
 * That is the whole point of the scenario — unticking a box in the Data tab is what makes a column
 * leave the RESULT, which is the event the reconcile has to notice.
 */
vi.mock("@/api/useSpecData", () => ({
  useSpecData: (_ws: string, _ds: string | null, spec: QuerySpec | null) => {
    const names = spec ? [...(spec.dimensions ?? []), ...(spec.columns ?? [])] : [];
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
        { column_name: "state", data_type: "varchar", is_temporal: false },
        { column_name: "vendor", data_type: "varchar", is_temporal: false },
      ],
      metrics: [],
    },
    isLoading: false,
  }),
}));

async function mountTable(id = "c-raw-table") {
  const { default: ChartExplore } = await import("../ChartExplore");
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <TooltipProvider>
        <MemoryRouter initialEntries={[`/explore/demo/charts/${id}?tab=display`]}>
          <Routes>
            <Route path="/explore/:workspace/charts/:id" element={<ChartExplore />} />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

/** The headers the rendered table actually draws, in draw order. */
const drawnHeaders = () =>
  screen.queryAllByRole("columnheader").map((th) => th.textContent?.replace(/[ ▲▼]+$/, "") ?? "");

const tab = (name: string) => userEvent.click(screen.getByRole("tab", { name }));

describe("a column put back into the query is drawn again", () => {
  it("does not keep a stale hidden flag across a column leaving and re-entering the result", async () => {
    await mountTable();
    // Auto-run is debounced (`useAutoRun`, 500ms), so the first result lands a beat after mount.
    await waitFor(() => expect(drawnHeaders()).toEqual(["id", "state", "vendor"]), { timeout: 3000 });

    // ── step 1: hide `vendor` in Display ▸ Visible columns
    await userEvent.click(screen.getByLabelText("vendor"));
    expect(drawnHeaders()).toEqual(["id", "state"]);

    // ── step 2: drop `vendor` from the query in the Data tab
    await tab("Data");
    await userEvent.click(screen.getByLabelText("vendor"));
    expect(screen.getByLabelText("vendor")).not.toBeChecked();
    await tab("Display");
    // Waiting on the DRAWN HEADERS here would prove nothing — they are already `[id, state]` from
    // step 1, so the assertion passes before the debounced re-run has even gone out and step 3
    // then measures a column that never left. The Visible-columns list is built from the RESULT,
    // so `vendor` dropping out of it is the signal that the new query came back without it.
    await waitFor(() => expect(screen.queryByLabelText("vendor")).not.toBeInTheDocument(), { timeout: 3000 });
    expect(drawnHeaders()).toEqual(["id", "state"]);

    // ── step 3: put `vendor` back into the query
    await tab("Data");
    await userEvent.click(screen.getByLabelText("vendor"));
    expect(screen.getByLabelText("vendor")).toBeChecked();
    await waitFor(() => expect(drawnHeaders()).toEqual(["id", "state", "vendor"]), { timeout: 3000 });

    // …and the Display tab agrees, rather than showing it as still hidden.
    await tab("Display");
    expect(screen.getByLabelText("vendor")).toBeChecked();
    // Each step waits on a debounced auto-run, so the whole sequence outlasts the 5s default.
  }, 30000);

  /**
   * THE OTHER HALF OF THE RULE, and the only assertion here that survives being pointed at the
   * wrong string. A column that is STILL in the result keeps the hidden flag the user set, across
   * an unrelated re-query — so it kills two mutants the sequence above cannot:
   *
   *  - "just clear `hiddenColumns` whenever the result changes", which would undo step 1 of every
   *    edit and which the sequence above would happily call a pass;
   *  - reconciling on the DISPLAY TITLE instead of the raw column name. `vendor` is renamed to
   *    "Supplier" here for exactly that reason: keyed on the title, the stored `vendor` matches
   *    nothing in the result, so the reconcile decides the column has left and prunes a flag the
   *    user had just set — the column silently comes back on the next re-query. That is the
   *    seventh raw-vs-display bug this programme has had, and this fixture is the only thing in
   *    the file that can see it. (Verified by mutating: without the rename it survived.)
   *
   * This one PASSED against the unfixed code, which normally means delete it. It is kept because
   * it is not coverage of the fix — it is the constraint that stops the fix from being written the
   * obvious wrong way, and both mutations above confirm it does that job.
   */
  it("keeps a hidden flag for a renamed column that never left the result", async () => {
    await mountTable("c-renamed");
    // The header, like the panel row, shows the rename; the column is still `vendor` underneath.
    await waitFor(() => expect(drawnHeaders()).toEqual(["id", "state", "Supplier"]), { timeout: 3000 });

    await userEvent.click(screen.getByLabelText("Supplier"));
    expect(drawnHeaders()).toEqual(["id", "state"]);

    // An edit that re-runs the query without touching `vendor`: drop `state` instead.
    await tab("Data");
    await userEvent.click(screen.getByLabelText("state"));
    await waitFor(() => expect(drawnHeaders()).toEqual(["id"]), { timeout: 3000 });

    await tab("Display");
    expect(screen.getByLabelText("Supplier")).not.toBeChecked();
    expect(within(document.body).queryByRole("columnheader", { name: /Supplier/ })).not.toBeInTheDocument();
  }, 30000);
});
