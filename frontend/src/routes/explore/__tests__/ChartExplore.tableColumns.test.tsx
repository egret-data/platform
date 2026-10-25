import { describe, it, expect, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { QuerySpec } from "@/api/charts";
import ChartExplore from "../ChartExplore";

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

/**
 * The same table with an ORDER ALREADY STORED. It exists because the reorder is a drag now, and no
 * test in this repo drives a dnd drop — dnd-kit's keyboard sensor reads element rects and jsdom has
 * none. The WRITE half is unit-tested (`reorderColumnOrder`); this fixture keeps the READ half
 * covered end to end, which is the half that decides what a reader actually sees.
 */
const TABLE_ORDERED = {
  ...TABLE,
  id: "c-ordered",
  params: {
    ...TABLE.params,
    settings: { ...TABLE.params.settings, "table.state": { columnOrder: ["id", "vendor", "state"] } },
  },
};

const CHARTS: Record<string, unknown> = {
  "c-raw-table": TABLE,
  "c-renamed": TABLE_RENAMED,
  "c-ordered": TABLE_ORDERED,
  // Renamed AND ordered: the order keys on `vendor`, the header reads "Supplier".
  "c-renamed-ordered": {
    ...TABLE_RENAMED,
    id: "c-renamed-ordered",
    params: {
      ...(TABLE_RENAMED as { params: Record<string, unknown> }).params,
      settings: {
        ...((TABLE_RENAMED as { params: { settings: Record<string, unknown> } }).params.settings),
        "table.state": { columnOrder: ["id", "vendor", "state"] },
      },
    },
  },
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

const TYPES: Record<string, string> = { id: "bigint", state: "varchar", vendor: "varchar", revenue: "double" };

/**
 * The engine's contract, not a fixture: the result carries exactly the columns the spec projected.
 * That is the whole point of the scenario — unticking a box in the Data tab is what makes a column
 * leave the RESULT, which is the event the reconcile has to notice.
 */
vi.mock("@/api/useSpecData", () => ({
  useSpecData: (_ws: string, _ds: string | null, spec: QuerySpec | null) => {
    // Metrics included: an aggregated table's result is its dimensions THEN its measures, and
    // leaving them out made an aggregation look like a query that returned nothing new.
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
        // `id` is the one column no other assertion in this file queries by name, which is why it
        // carries the rename: `Add or remove columns` has to SAY "Mã" while every stored list keeps
        // keying on `id`. See the last test in this file.
        { column_name: "id", data_type: "bigint", is_temporal: false, verbose_name: "Mã" },
        { column_name: "state", data_type: "varchar", is_temporal: false },
        { column_name: "vendor", data_type: "varchar", is_temporal: false },
      ],
      // One metric the chart does NOT use. It is what makes the fourth parallel field list real:
      // an aggregated table shows `revenue` in the display list while it lives in a Metrics list
      // one tab over. The `Add or remove columns` panel is what brings both into one place.
      metrics: [{ metric_name: "revenue", data_type: "double" }],
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

/**
 * The headers the rendered table actually draws, in draw order. `id` draws as "Mã": a table header
 * reads the dataset's Display name (KI-103), while every stored list keeps keying on `id`.
 */
const drawnHeaders = () =>
  screen.queryAllByRole("columnheader").map((th) => th.textContent?.replace(/[ ▲▼]+$/, "") ?? "");

const tab = (name: string) => userEvent.click(screen.getByRole("tab", { name }));

/**
 * The Display tab's row for one column, by the RAW name it is keyed on.
 *
 * MOVED IN PR B. This used to be `getByLabelText(<title>)` — the checkbox in the old
 * `Visible columns` list, whose <Label> was the column's DISPLAY title. The three lists have since
 * merged into one row per column, and its eye toggle is labelled by the raw column name (see
 * `TableColumnsSection`), which is also the only name every stored list keys on. `toBeChecked()`
 * becomes `aria-pressed`, since the control is a toggle button and no longer a checkbox.
 *
 * The Data tab's checkboxes are untouched and are still driven by `getByLabelText(name)` below —
 * they were always labelled by the raw name, because that tab edits the query.
 */
/**
 * The Data tab's checkbox for one column, BY ROLE.
 *
 * `getByLabelText(name)` until the column headers gained an explicit `aria-label` of the plain
 * column title — added so that the new "Sort by …" button inside a header would not become the
 * cell's own accessible name, which a screen reader repeats for every cell under it. The header and
 * the checkbox then shared a label and the bare query matched both. Naming the role is what the
 * query always meant.
 */
const dataColumn = (name: string) => screen.getByRole("checkbox", { name });

/**
 * Toggle a column in the QUERY.
 *
 * It lives behind Display ▸ "Add or remove columns" now: the Data tab's duplicate list — the same
 * fields writing the same `config.columns` — was removed, and this fold is the only field picker a
 * table has. The opener is only clicked when it is still closed; once open it reads
 * "Done picking columns" and the fold stays open between toggles.
 */
const openPicker = async () => {
  await tab("Data");
  const opener = screen.queryByRole("button", { name: /add or remove columns/i });
  if (opener) await userEvent.click(opener);
};
const toggleQueryColumn = async (name: string) => {
  await openPicker();
  await userEvent.click(dataColumn(name));
};
/** The picker REPLACES the column list while it is open, so anything that then wants a row's eye
 *  or its drill-in has to close it first. That replacement is deliberate — the defect being fixed
 *  was two column lists on screen at once. */
const closePicker = async () => {
  const done = screen.queryByRole("button", { name: /done picking columns/i });
  if (done) await userEvent.click(done);
};

const eye = (name: string) => screen.getByRole("button", { name: `Show ${name}` });
const noEye = (name: string) => screen.queryByRole("button", { name: `Show ${name}` });

describe("a column put back into the query is drawn again", () => {
  it("does not keep a stale hidden flag across a column leaving and re-entering the result", async () => {
    await mountTable();
    // Auto-run is debounced (`useAutoRun`, 500ms), so the first result lands a beat after mount.
    await waitFor(() => expect(drawnHeaders()).toEqual(["Mã", "state", "vendor"]), { timeout: 3000 });

    // ── step 1: hide `vendor` in Display ▸ Visible columns
    await userEvent.click(eye("vendor"));
    expect(drawnHeaders()).toEqual(["Mã", "state"]);

    // ── step 2: drop `vendor` from the query, through Display's field picker
    await toggleQueryColumn("vendor");
    expect(dataColumn("vendor")).not.toBeChecked();
    await closePicker();
    // Waiting on the DRAWN HEADERS here would prove nothing — they are already `[id, state]` from
    // step 1, so the assertion passes before the debounced re-run has even gone out and step 3
    // then measures a column that never left. The Visible-columns list is built from the RESULT,
    // so `vendor` dropping out of it is the signal that the new query came back without it.
    await waitFor(() => expect(noEye("vendor")).not.toBeInTheDocument(), { timeout: 3000 });
    expect(drawnHeaders()).toEqual(["Mã", "state"]);

    // ── step 3: put `vendor` back into the query
    await toggleQueryColumn("vendor");
    expect(dataColumn("vendor")).toBeChecked();
    await closePicker();
    await waitFor(() => expect(drawnHeaders()).toEqual(["Mã", "state", "vendor"]), { timeout: 3000 });

    // …and the Columns tab agrees, rather than showing it as still hidden.
    await tab("Data");
    expect(eye("vendor")).toHaveAttribute("aria-pressed", "true");
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
    await waitFor(() => expect(drawnHeaders()).toEqual(["Mã", "state", "Supplier"]), { timeout: 3000 });

    await userEvent.click(eye("vendor"));
    expect(drawnHeaders()).toEqual(["Mã", "state"]);
    // The rename is live in the PANEL too, not only in the header the click just removed: the row
    // shows "Supplier" while its toggle is still addressed as `vendor`. That pairing is the whole
    // point of this fixture, and it is what the old `getByLabelText("Supplier")` used to carry.
    expect(screen.getByText("Supplier")).toBeInTheDocument();

    // An edit that re-runs the query without touching `vendor`: drop `state` instead.
    await toggleQueryColumn("state");
    await closePicker();
    await waitFor(() => expect(drawnHeaders()).toEqual(["Mã"]), { timeout: 3000 });

    await tab("Data");
    expect(eye("vendor")).toHaveAttribute("aria-pressed", "false");
    expect(within(document.body).queryByRole("columnheader", { name: /Supplier/ })).not.toBeInTheDocument();
  }, 30000);
});

/**
 * B2: the query's field lists are reachable from the DISPLAY tab, folded behind Metabase's own
 * `Add or remove columns` affordance.
 *
 * Driven through the route on purpose. `TableColumnsSection`'s own tests prove the panel emits
 * what it is handed; only this file can prove `ChartExplore` hands it anything — a `queryFields`
 * prop that is never passed is a green unit test over dead wiring, which this programme has
 * shipped twice.
 */
describe("Add or remove columns, from the Display tab", () => {

  it("removes a column from the QUERY and the table stops drawing it", async () => {
    await mountTable();
    await waitFor(() => expect(drawnHeaders()).toEqual(["Mã", "state", "vendor"]), { timeout: 3000 });

    // The picker floats over the list now rather than replacing it, so the row stays put while
    // its query membership is edited — the pivot's field-zone shape.
    await openPicker();
    expect(noEye("vendor")).toBeInTheDocument();

    await userEvent.click(dataColumn("vendor"));
    await waitFor(() => expect(drawnHeaders()).toEqual(["Mã", "state"]), { timeout: 3000 });

    // …and the picker agrees, because there is one stored key, not two.
    await openPicker();
    expect(dataColumn("vendor")).not.toBeChecked();
  }, 30000);

  /**
   * DISAGREEMENT #1, closed. A metric was in a fourth field list that the Display tab could not
   * reach at all, so an aggregated table drew a column no list on that tab mentioned.
   */
  it("reaches the metrics list from the same panel", async () => {
    await mountTable();
    await waitFor(() => expect(drawnHeaders()).toEqual(["Mã", "state", "vendor"]), { timeout: 3000 });

    expect(screen.queryByLabelText("revenue")).not.toBeInTheDocument();
    await openPicker();
    await userEvent.click(dataColumn("revenue"));

    // Aggregating turns the raw columns into dimensions and adds the metric to the result.
    await waitFor(() => expect(drawnHeaders()).toEqual(["Mã", "state", "vendor", "revenue"]), { timeout: 3000 });
  }, 30000);

  /**
   * THE LABELS ARE WIRED, and only this file can say so.
   *
   * `TableColumnsSection` renders whatever `QueryFieldGroup.labels` holds — its own tests prove
   * that. The map itself is built HERE, in `ChartExplore`, out of the same `columns` array the Data
   * tab's list reads, and a `labels` prop that is never passed is a green unit test over dead
   * wiring — the failure this file's opening comment says the programme has shipped twice.
   *
   * Live, before this: the Data tab said "Khu vực" and this picker said `region`, one click apart.
   * `Mã` here is the same shape on the fixture.
   *
   * The last two lines are the half that matters most. The row SAYS the friendly name and the
   * checkbox is still found under the machine name — `getByRole("checkbox", { name: "id" })` would
   * throw if the accessible name had become the display string, which is how a picker that had
   * started storing the wrong thing would first show itself.
   */
  it("names a renamed column the way the dataset does, and still keys on the raw name", async () => {
    await mountTable();
    await waitFor(() => expect(drawnHeaders()).toEqual(["Mã", "state", "vendor"]), { timeout: 3000 });
    await openPicker();
    // The header now says "Mã" too (KI-103), so the picker's row is found by its hover title.
    const row = screen.getByTitle("Mã (id)");
    expect(row).toHaveTextContent("Mã");
    expect(within(row).queryByText("id")).toBeNull();
    expect(dataColumn("Mã")).toBeChecked();
  }, 30000);
});

/**
 * DISAGREEMENT #2, closed: the panel's list is the order the table draws in, and moving a row
 * moves the column.
 *
 * Reorder used to exist ONLY as a drag on the rendered header. `TableColumnsSection` never read
 * `columnOrder` at all, so the panel drew its list in result order and the table drew itself in
 * stored order — two lists on one screen claiming to be the same thing while disagreeing.
 */
describe("reordering from the panel moves the drawn column", () => {
  it("draws the table AND the panel in the stored order, not in result order", async () => {
    await mountTable("c-ordered");
    // The query projects id, state, vendor; the stored order says id, vendor, state.
    await waitFor(() => expect(drawnHeaders()).toEqual(["Mã", "vendor", "state"]), { timeout: 3000 });
    // …and the panel's own list is the same list, not a second one that disagrees.
    expect(Array.from(document.querySelectorAll("[data-column]")).map((e) => e.getAttribute("data-column")))
      .toEqual(["id", "vendor", "state"]);
  }, 30000);

  /**
   * THE RAW-NAME MUTATION GUARD, end to end. `ChartTable` resolves `columnOrder` through the raw
   * column name; `column_title` renames the header without renaming the column. If the panel wrote
   * the DISPLAYED string, `ChartTable` would resolve nothing for it, drop it out of the order and
   * re-append it at the end — so the header would land at position 3 instead of position 2, and the
   * user's drag would be silently ignored.
   *
   * Three columns with the renamed one MOVING INTO THE MIDDLE is the smallest fixture that can see
   * that: with two, losing a name from the order and re-appending it gives the identical layout.
   */
  /**
   * The write half of this — that a move emits `vendor` and never "Supplier" — is pinned on
   * `reorderColumnOrder` directly, because the reorder is a drag now and no test here drives one.
   * What is still worth asserting end to end is the READ: an order stored under the RAW name must
   * move the column even though its header says something else. A `columnOrder` of display strings
   * would resolve nothing and leave this in result order.
   */
  it("resolves a stored order by the RAW name even when the header is renamed", async () => {
    await mountTable("c-renamed-ordered");
    await waitFor(() => expect(drawnHeaders()).toEqual(["Mã", "Supplier", "state"]), { timeout: 3000 });
  }, 30000);
});
