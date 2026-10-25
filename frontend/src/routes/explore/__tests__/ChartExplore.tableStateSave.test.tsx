import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { QuerySpec } from "@/api/charts";
import ChartExplore from "../ChartExplore";

/**
 * WHAT THE EDITOR SENDS TO THE SERVER WHEN A TABLE'S COLUMN LAYOUT HAS BEEN EDITED.
 *
 * `tableState` — hidden columns, display order, widths, the freeze count — was live editor state
 * that `paramsForSave` never put into the payload. Every one of those edits drew immediately and
 * was dropped on Save; a chart that already carried a `table.state` only kept it because the whole
 * hydrated `settings` blob rides through `paramsForSave` verbatim, so what was stored survived and
 * what the author did in this editor did not. `ChartExplore.tableColumns.test.tsx` proves the edits
 * reach the SCREEN; nothing proved they reach the SERVER, which is the half that was missing.
 *
 * Asserted on the argument the mutation is actually called with — the last thing this route owns
 * before the payload leaves it. Anything shallower (spying on `paramsForSave`, or on state) cannot
 * tell "built into the payload" from "held in a hook", which is precisely the distinction the
 * defect turned on.
 */

const { updateMutate } = vi.hoisted(() => ({ updateMutate: vi.fn() }));

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
    create: { mutateAsync: vi.fn(), isPending: false },
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
        { column_name: "state", data_type: "varchar", is_temporal: false },
        { column_name: "vendor", data_type: "varchar", is_temporal: false },
      ],
      metrics: [],
    },
    isLoading: false,
  }),
}));

async function mountTable(id = "c-raw-table") {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <TooltipProvider>
        <MemoryRouter initialEntries={[`/explore/demo/charts/${id}?tab=data`]}>
          <Routes>
            <Route path="/explore/:workspace/charts/:id" element={<ChartExplore />} />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

const drawnHeaders = () =>
  screen.queryAllByRole("columnheader").map((th) => th.textContent?.replace(/[ ▲▼]+$/, "") ?? "");

/** Toolbar Update saves an existing chart in one step (KI-008). */
async function saveChart() {
  await userEvent.click(screen.getByRole("button", { name: "Update" }));
}

/** The `settings` blob of the payload the last Update sent. */
const savedSettings = () =>
  (updateMutate.mock.calls.at(-1)![0] as { params: { settings: Record<string, unknown> } }).params.settings;

beforeEach(() => updateMutate.mockClear());

describe("a table's column layout is part of what Save sends", () => {
  it("carries the hidden column and the display order into the saved params", async () => {
    await mountTable("c-ordered");
    await waitFor(() => expect(drawnHeaders()).toEqual(["state", "id", "vendor"]), { timeout: 3000 });

    // TWO STORED KEYS, and a payload carrying only one of them would be just as broken — the point
    // of this test. The ORDER arrives from the fixture rather than from an edit: reordering is a
    // drag now and no test here drives one (dnd-kit's keyboard sensor reads element rects, and
    // jsdom has none). `reorderColumnOrder` pins what a drop WRITES; this pins that whatever sits
    // in `tableState` reaches the save alongside the hide.
    await userEvent.click(screen.getByRole("button", { name: "Show vendor" }));
    expect(drawnHeaders()).toEqual(["state", "id"]);

    await saveChart();
    await waitFor(() => expect(updateMutate).toHaveBeenCalled());

    // `toStrictEqual`, and the whole object: the point is WHICH keys are there. `toEqual` would
    // pass against a payload that carried an `undefined` under either name.
    expect(savedSettings()["table.state"]).toStrictEqual({
      hiddenColumns: ["vendor"],
      columnOrder: ["state", "id", "vendor"],
    });
  }, 30000);

  /**
   * The other half of `mergeVizSettings`' rule, applied to this key: a setting at its DEFAULT
   * leaves the stored settings rather than lingering as an empty object. Without it every chart
   * ever saved — a pie, a scalar, a table nobody has touched the columns of — would grow a
   * `"table.state": {}` it does not use, and the junk fixture's `null` would be written straight
   * back out.
   */
  it("omits the key entirely when there is no column state to store", async () => {
    await mountTable();
    await waitFor(() => expect(drawnHeaders()).toEqual(["id", "state", "vendor"]), { timeout: 3000 });

    await saveChart();
    await waitFor(() => expect(updateMutate).toHaveBeenCalled());

    expect(savedSettings()).not.toHaveProperty("table.state");
  }, 30000);

  it("drops a stored value the editor cannot represent instead of writing it back", async () => {
    await mountTable("c-junk");
    await waitFor(() => expect(drawnHeaders()).toEqual(["id", "state", "vendor"]), { timeout: 3000 });

    await saveChart();
    await waitFor(() => expect(updateMutate).toHaveBeenCalled());

    expect(savedSettings()).not.toHaveProperty("table.state");
  }, 30000);
});
