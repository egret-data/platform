import { describe, it, expect, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { QuerySpec } from "@/api/charts";
// Imported with the file, not inside the first test: the route's cold import took 5-8 s
// under load, every time inside whichever test mounted first, against a 5 s budget.
import ChartExplore from "../ChartExplore";

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

/**
 * A bar chart saved WITH a sort its author asked for. Its stored `orderby` is the sum of two
 * clauses — `total_revenue DESC` the author wrote, and `region ASC` the cartesian descriptor
 * derives for its breakout — which is exactly what `paramsForSave` writes and what the studio has
 * to be able to take apart and put back together.
 */
const BAR_SORTED = {
  ...PIE_V1,
  id: "c-bar-sorted", name: "Bar sorted", viz_type: "bar",
  params: {
    version: 2,
    spec: {
      metrics: ["total_revenue"], dimensions: ["region"], columns: [], filters: [],
      orderby: [{ field: "total_revenue", desc: true }, { field: "region", desc: false }],
      row_limit: 1000, rollup_groups: [],
    },
    settings: { "cartesian.xAxis": "region", "cartesian.yMetrics": ["total_revenue"] },
  },
};

/** A raw-records viz: its spec carries `columns`, `orderby_sql`'s third arm. */
const BOXPLOT = {
  ...PIE_V1,
  id: "c-boxplot", name: "Box plot", viz_type: "boxplot",
  params: {
    version: 2,
    spec: {
      metrics: [], dimensions: [], columns: ["region", "amount"], filters: [], orderby: [],
      row_limit: 1000, rollup_groups: [],
    },
    settings: { "boxplot.category": "region", "boxplot.value": "amount" },
  },
};

/** The one viz whose descriptor ignores an authored sort, so the control must not appear. */
const TREND = {
  ...PIE_V1,
  id: "c-trend", name: "Trend", viz_type: "smartscalar",
  params: {
    version: 2,
    spec: {
      metrics: ["total_revenue"], dimensions: ["region"], columns: [], filters: [],
      orderby: [{ field: "region", desc: false }], row_limit: 1000, rollup_groups: [],
    },
    settings: { "smartscalar.dimension": "region", "smartscalar.metric": "total_revenue" },
  },
};

/**
 * A pie broken out by a TEMPORAL EXTRACTION unit. Its ring aggregates on `temporalLabel(unit, raw)`
 * — "Mon"/"Wed", never the 1/3 in the column — so it is the shape that tells a settings panel keyed
 * on the chart apart from one keyed on the raw result. A truncation unit (`month`) would not: there
 * the raw value IS the key on both sides.
 */
const PIE_DOW = {
  ...PIE_V1,
  id: "c-pie-dow", name: "Pie by weekday", viz_type: "pie",
  params: {
    version: 2,
    spec: {
      metrics: ["total_revenue"], dimensions: ["dow"], columns: [], filters: [], orderby: [],
      row_limit: 1000, rollup_groups: [], temporal_units: { dow: "day_of_week" },
    },
    settings: { "pie.dimensions": ["dow"], "pie.metric": "total_revenue" },
  },
};

/** A one-row chart: its query is a single aggregate whatever the Row limit says (KI-051). */
const SCALAR = {
  ...PIE_V1,
  id: "c-scalar", name: "Big number", viz_type: "scalar",
  params: {
    version: 2,
    spec: { metrics: ["total_revenue"], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 1, rollup_groups: [] },
    settings: { "scalar.metric": "total_revenue" },
  },
};

const CHARTS: Record<string, unknown> = {
  "c-scalar": SCALAR,
  "c-pie": PIE_V1, "c-bar": BAR_V2, "c-table": TABLE_V2, "c-bar-sorted": BAR_SORTED,
  "c-boxplot": BOXPLOT, "c-trend": TREND, "c-pie-dow": PIE_DOW,
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

/** The spec the studio last asked the backend to run — i.e. `specForRun()`'s output, observed at
 *  the only seam it crosses. */
let lastRanSpec: QuerySpec | null = null;

vi.mock("@/api/useSpecData", () => ({
  useSpecData: (_ws: string, _ds: string | null, spec: QuerySpec | null) => {
    lastRanSpec = spec;
    // The bucketed chart's result is what the engine returns for a `day_of_week` grouping: the
    // integer, not the word. Deliberately not pre-sorted — the pie draws Mon (40) before Wed (20).
    if (spec?.dimensions?.includes("dow")) {
      return {
        columns: [{ name: "dow", type: "bigint" }, { name: "total_revenue", type: "double" }],
        rows: [[3, 20], [1, 40]],
        isLoading: false, error: null, hasResult: true,
      };
    }
    return {
      columns: [{ name: "region", type: "varchar" }, { name: "total_revenue", type: "double" }],
      rows: [["North", 1]],
      isLoading: false, error: null, hasResult: true,
    };
  },
}));

vi.mock("@/api/datasets", () => ({
  useDatasets: () => ({ data: [{ id: "d1", name: "Demo Sales" }], isLoading: false }),
  useDataset: () => ({
    data: {
      id: "d1", name: "Demo Sales",
      columns: [
        { column_name: "region", data_type: "varchar", is_temporal: false },
        { column_name: "category", data_type: "varchar", is_temporal: false },
        { column_name: "amount", data_type: "double", is_temporal: false },
        // Temporal, so the studio offers it a unit at all — the saved chart already picks one, so
        // `withDefaultTemporalUnits` leaves its `day_of_week` alone.
        { column_name: "dow", data_type: "timestamp", is_temporal: true },
      ],
      metrics: [{ metric_name: "total_revenue", data_type: "double" }],
    },
    isLoading: false,
  }),
}));

async function mountChart(id: string, tab?: "data" | "display" | "rules") {
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
  it("offers Row limit where the query reads it, and not on a one-row chart (KI-051)", { timeout: 15_000 }, async () => {
    const { unmount } = await mountChart("c-bar");
    expect(await screen.findByLabelText("Row limit")).toBeInTheDocument();
    unmount();
    await mountChart("c-scalar");
    await screen.findByDisplayValue("Big number");
    expect(screen.queryByLabelText("Row limit")).not.toBeInTheDocument();
  });

  /**
   * KI-047: bar, line, area and combo share one settings namespace (`cartesian.*`), so a Change
   * between them has nothing to translate — it used to drop the X-axis and the Y-axis anyway. A
   * switch to a type with its own namespace (pie) still starts from that type's defaults.
   */
  it("keeps the X-axis and Y-axis across a Change between cartesian types", { timeout: 15_000 }, async () => {
    lastRanSpec = null;
    const user = userEvent.setup();
    await mountChart("c-bar");
    await screen.findByDisplayValue("Bar v2");
    await user.click(screen.getByRole("button", { name: "Change" }));
    await user.click(within(await screen.findByRole("dialog")).getByText("Line", { exact: true }));
    expect(await screen.findByText("Line")).toBeInTheDocument();
    expect(screen.queryByText("Pick a dimension…")).not.toBeInTheDocument();
    await waitFor(() => expect(lastRanSpec).toMatchObject({ dimensions: ["region"], metrics: ["total_revenue"] }));

    await user.click(screen.getByRole("button", { name: "Change" }));
    await user.click(within(await screen.findByRole("dialog")).getByText("Pie", { exact: true }));
    expect(await screen.findByText("Add a dimension…")).toBeInTheDocument();
  });

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

  /**
   * TWO ASSERTIONS HERE USED TO PIN THE DEFECT THIS PR FIXES, and both are rewritten rather than
   * updated in place, because what they were reaching for is now spelled differently.
   *
   * `getByRole("tab", { name: "Colors" })` asserted the tab's OLD name. The tab holds Metabase's
   * `Conditional Formatting` section (`Table.tsx`, `PivotTable/settings.ts`), and the panel's own
   * section heading inside it has read "Conditional formatting" since it was written — so "Colors"
   * was the one place in egret that named the feature by its colour rather than by itself, and the
   * test made that permanent.
   *
   * `queryByText("Conditional formatting")).not.toBeInTheDocument()` was the sharper trap. It meant
   * "the rules EDITOR is not stacked into the Display tab" — the panel-length defect the split
   * closed — but it asserted it by the absence of a STRING, and that string is now the tab's own
   * label, which is present on every tab. It is replaced by a query for the editor's own prose,
   * which is what "the editor is not here" actually means and cannot be satisfied by a tab label.
   */
  // The column section moved to the Data tab, beside the Metrics list it answers the same kind of
  // question as. The rules editor staying OUT of it is what this has always been about.
  it("keeps the table's column section on the Data tab, without the rules editor", async () => {
    await mountChart("c-table", "data");
    expect(await screen.findByText("Columns")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Add rule" })).not.toBeInTheDocument();
  });

  it("names the rules tab after the feature, the way the reference does", async () => {
    await mountChart("c-table", "display");
    expect(screen.getByRole("tab", { name: "Conditional formatting" })).toBeInTheDocument();
    expect(screen.queryByRole("tab", { name: "Colors" })).not.toBeInTheDocument();
  });

  it("puts conditional formatting on its own tab", async () => {
    await mountChart("c-table", "rules");
    expect(await screen.findByRole("button", { name: "Add rule" })).toBeInTheDocument();
  });

  /**
   * THE STUDIO'S OWN SEAM, driven through the route that owns it. The temporal unit is QUERY state
   * — `spec.temporal_units` — and lives nowhere in any viz's settings, so the only way a control
   * can key its rows the way the chart keys its slices is for the shell to hand the units over.
   * Until it did, the pie's slice list read "3", "1" against a chart drawing "Mon" and "Wed", and
   * every hide, rename and pinned colour authored there was written under a key no slice carries.
   *
   * Asserted on the route rather than on the control: a control test passes just as happily when
   * nothing hands the prop over (PR 02 and PR 06 both found that shape).
   */
  it("keys the pie's slice list on the extracted word the bucketed chart drew", async () => {
    await mountChart("c-pie-dow", "display");
    await screen.findByDisplayValue("Pie by weekday");
    // Auto-run is debounced, so the result — and with it the slice list — arrives a beat later.
    expect(await screen.findByLabelText("Show Mon", {}, { timeout: 3000 })).toBeInTheDocument();
    expect(screen.getByLabelText("Show Wed")).toBeInTheDocument();
    // The raw column values are not keys of anything the chart drew.
    expect(screen.queryByLabelText("Show 1")).not.toBeInTheDocument();
    expect(screen.queryByLabelText("Show 3")).not.toBeInTheDocument();
  });

  /**
   * …AND ON THE UNITS THAT RAN, NOT THE ONES BEING TYPED. With Auto-run off, changing the grouping
   * edits the NEXT query; the rows on screen are still the old ones, and re-reading them under a
   * unit they were never fetched with describes a chart nobody is looking at. Switching `dow` to
   * `month` here would re-key the very same integers as "3" and "1" — the raw list, arrived at from
   * the other direction — while the chart beside the panel still draws Mon and Wed.
   *
   * The same rule (and the same `previewParams` source) the table's drill already follows.
   */
  it("keys the list on the units the result ran with, not on an unrun edit", async () => {
    await mountChart("c-pie-dow", "display");
    await screen.findByLabelText("Show Mon", {}, { timeout: 3000 });

    // Auto off, so the edit below cannot become the result while the assertion runs.
    await userEvent.click(screen.getByLabelText("Auto-run"));
    await userEvent.click(screen.getByRole("tab", { name: "Data" }));
    // The unit picker has no label of its own (it is captioned by the "Unit \u00b7 dow" row beside
    // it), and a `combobox` takes no accessible name from its contents — so it is found by the
    // grouping it currently shows.
    const unitPicker = screen.getByRole("combobox", { name: /group .* by/i });
    await userEvent.click(unitPicker);
    await userEvent.click(await screen.findByRole("option", { name: "Month" }));
    await userEvent.click(screen.getByRole("tab", { name: "Display" }));

    expect(await screen.findByLabelText("Show Mon")).toBeInTheDocument();
    expect(screen.getByLabelText("Show Wed")).toBeInTheDocument();
    expect(screen.queryByLabelText("Show 3")).not.toBeInTheDocument();
  });

  /**
   * `orderby` is shared query state, so it has to survive the same round trip `filters` and
   * `rowLimit` do: stored spec -> studio state -> `specForRun()` -> the query that runs. The
   * failure this catches is the quiet one — the studio hydrating the sort into state and then not
   * passing it back to `buildSpec`, which draws a chart in the wrong order while every descriptor
   * unit test still passes.
   */
  it("runs a saved chart's authored sort, and does not double the derived clause", async () => {
    lastRanSpec = null;
    await mountChart("c-bar-sorted");
    await screen.findByDisplayValue("Bar sorted");
    // Auto-run is debounced (`useAutoRun`, 500ms), so the spec reaches the query a beat after the
    // editor has hydrated — nothing to assert until it does.
    await waitFor(() => expect(lastRanSpec?.orderby).toEqual([
      { field: "total_revenue", desc: true },
      { field: "region", desc: false },
    ]), { timeout: 3000 });
  });

  // A chart with no authored sort still runs PR #394's derived clause, and only that one — the
  // half a "just always emit shared.orderby" change would silently delete.
  it("runs the derived ascending clause for a chart with no authored sort", async () => {
    lastRanSpec = null;
    await mountChart("c-bar");
    await screen.findByDisplayValue("Bar v2");
    await waitFor(
      () => expect(lastRanSpec?.orderby).toEqual([{ field: "region", desc: false }]),
      { timeout: 3000 },
    );
  });
});

/**
 * The control itself, rendered by the only thing that renders it in production. A sort control can
 * pass a component test and still be invisible or unwired — PR 06 shipped exactly that shape — so
 * these drive the real route and read the spec that reaches the query.
 */
describe("the sort control in the Data tab", () => {
  it("offers an aggregate viz's breakout and aggregation, and sends what is picked", async () => {
    lastRanSpec = null;
    await mountChart("c-bar");
    await screen.findByDisplayValue("Bar v2");
    await userEvent.click(screen.getByRole("combobox", { name: /add a sort/i }));
    // Exactly the spec's fields — `orderby_sql`'s metric and dimension arms — and not `category`,
    // which is on the dataset but not in this chart's spec.
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual(["region", "total_revenue"]);
    await userEvent.click(screen.getByRole("option", { name: "total_revenue" }));
    // Ascending, Metabase's default for a new clause, composed with the derived breakout order.
    await waitFor(() => expect(lastRanSpec?.orderby).toEqual([
      { field: "total_revenue", desc: false },
      { field: "region", desc: false },
    ]), { timeout: 3000 });

    await userEvent.click(screen.getByRole("button", { name: /sort 1 direction: ascending/i }));
    await waitFor(() => expect(lastRanSpec?.orderby).toEqual([
      { field: "total_revenue", desc: true },
      { field: "region", desc: false },
    ]), { timeout: 3000 });
  });

  it("offers a raw-records viz's columns, and sends what is picked", async () => {
    lastRanSpec = null;
    await mountChart("c-boxplot");
    await screen.findByDisplayValue("Box plot");
    await userEvent.click(screen.getByRole("combobox", { name: /add a sort/i }));
    expect(screen.getAllByRole("option").map((o) => o.textContent)).toEqual(["region", "amount"]);
    await userEvent.click(screen.getByRole("option", { name: "amount" }));
    await waitFor(
      () => expect(lastRanSpec?.orderby).toEqual([{ field: "amount", desc: false }]),
      { timeout: 3000 },
    );
  });

  // Not offered on a trend: `SmartScalarChart` reads the last two rows as this period and the
  // previous one, so its dimension order is a precondition of the arithmetic. An absent control
  // beats a silently wrong number.
  it("is not offered on a trend", async () => {
    await mountChart("c-trend");
    await screen.findByDisplayValue("Trend");
    expect(screen.queryByRole("combobox", { name: /add a sort/i })).not.toBeInTheDocument();
    expect(screen.queryByText("Sort")).not.toBeInTheDocument();
  });
});
