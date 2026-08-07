import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import DashboardView from "../DashboardView";

/**
 * THE CARD SETTINGS PANEL HAS TO SEE THE CARD'S RESULT, not a list of names reconstructed from the
 * saved spec.
 *
 * `DashboardView` used to build the panel's column list out of `params.spec` and hand it no rows at
 * all — `{ name, type: "" }` for every dimension, `"double"` for every metric, `rows: []`. Three
 * separate controls read the RESULT rather than the spec, and each one degrades to a plausible,
 * silent, wrong answer under that input:
 *
 *   - the gauge's Ranges list sizes its defaults from `rows[0][metricIndex]`, so no rows meant
 *     `value = 0` and `defaultGaugeSegments(0)` — the 0/25/50/100 placeholder — shown against a card
 *     whose arc is drawn on 0 → 3,021,243.38. Nudging any bound then WRITES those three rows into
 *     the card override and re-scales the card. Data loss out of a control the panel invited you to
 *     trust;
 *   - every format popover branches on the column's TYPE, so `type: ""` made a date column and a
 *     numeric non-metric column both read as Text and dropped their entire format section;
 *   - the pie's per-slice list is derived by `pieSliceRows(resultColumns, rows, …)`, so no rows meant
 *     no slices and the whole Slices block vanished from the card panel while the builder showed it.
 *
 * These tests drive the WHOLE route rather than `CardVizPanel` in isolation, because the defect was
 * never in the panel — the panel's `rows` prop existed and was plumbed through `EditSidebar` all
 * along. It was the route that never filled it. A `CardVizPanel` unit test passing the right props
 * would have gone on passing throughout.
 *
 * The result the route sees comes from `chartsChRunSpec` (`POST /charts/run`) via `useSpecData`,
 * mocked here, which is the same query key the edit-mode card tile runs on — so the panel is reading
 * the very bytes the card was drawn from, not a second fetch that could disagree with it.
 */

const chartsChRunSpec = vi.fn();
vi.mock("@/api/generated/charts/charts", () => ({
  chartsChRunSpec: (...a: unknown[]) => chartsChRunSpec(...a),
}));

// The tile itself is stubbed out: it would fire its own copy of the same query and drag ECharts into
// the test, and nothing here asserts on what it draws. The PANEL is the subject.
//
// It does echo back the card's OVERRIDE, though, which is the one thing about the tile these tests
// care about. `DashboardCardView` reads `card.options.vizOverride` and hands it to
// `applyVizOverride` before it builds anything (DashboardCardView.tsx:62), so what arrives on this
// prop is exactly what the card would draw with. That turns "the panel offers the control" into
// "the control reaches the card", which is the assertion that actually matters for a setting whose
// whole defect was being unreachable — and it is a claim a panel-only render cannot make.
vi.mock("@/components/chart/dashboard/DashboardCardView", () => ({
  DashboardCardView: ({ card }: { card: { id: string; options?: { vizOverride?: unknown } } }) => (
    <div data-testid={`disp-${card.id}`}
      data-viz-override={JSON.stringify(card.options?.vizOverride ?? null)} />
  ),
}));

let charts: unknown[] = [];
vi.mock("@/api/charts", async (orig) => ({
  ...(await (orig as () => Promise<Record<string, unknown>>)()),
  useCharts: () => ({ data: charts }),
}));

let cards: unknown[] = [];
let parameters: unknown[] = [];
vi.mock("@/api/dashboards", async (orig) => {
  const m = await (orig as () => Promise<Record<string, unknown>>)();
  return {
    ...m,
    useDashboard: () => ({
      data: {
        id: "d1", name: "D", description: "", created_by: null, updated_at: "",
        parameters, tabs: [], width: "full", cards,
      },
      isLoading: false,
    }),
    useDashboardMutations: () => ({ update: { mutateAsync: vi.fn(), isPending: false } }),
  };
});

function chartCard(chartId: string) {
  return { id: "c1", chart_id: chartId, card_type: "chart", grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: {}, tab_id: null };
}

/** `region` is the one parameter slug any of these fixtures uses; a value here lands in the URL,
 *  which is where `useDashboardParams` reads parameter state from. */
function renderView(region?: string[]) {
  const qs = region ? `?region=${encodeURIComponent(JSON.stringify(region))}` : "";
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter initialEntries={[`/explore/eng/dashboards/d1${qs}`]}>
        <Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/** Enter edit mode and open the card's visualization-settings panel (the paintbrush on the hover
 *  toolbar), then wait for the run to land. */
async function openVizPanel() {
  fireEvent.click(screen.getByRole("button", { name: "Edit" }));
  fireEvent.click(screen.getByRole("button", { name: "card visualization" }));
  await screen.findByRole("button", { name: "reset visualization" });
}

beforeEach(() => {
  chartsChRunSpec.mockReset();
  parameters = [];
});

describe("DashboardView — the card viz panel reads the card's own result", () => {
  /**
   * D1. The one that destroys a card.
   *
   * `1_510_621.69` is the ZZ Chart Parity gauge's measured value; `defaultGaugeSegments` puts the
   * boundaries at v/2 and v over an arc of 0 → 2v, so the three rows are
   * 0–755,310.85 / 755,310.85–1,510,621.69 / 1,510,621.69–3,021,243.38 — exactly what the builder
   * route shows for the same chart, and exactly what the card's arc is drawn on.
   *
   * The fixture cannot be satisfied by accident: every one of those six numbers differs from the
   * 0/25/50/100 placeholder, so a panel that has not seen the row fails on the first assertion.
   */
  it("a gauge card's Ranges are sized from the row the card was drawn from, not from zero", async () => {
    charts = [{
      id: "g", name: "Gauge", dataset_id: "d", viz_type: "gauge",
      params: {
        version: 2,
        spec: { metrics: ["rev"], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [] },
        settings: { "gauge.metric": "rev" },
      },
    }];
    cards = [chartCard("g")];
    chartsChRunSpec.mockResolvedValue({ data: { columns: [{ name: "rev", type: "double" }], rows: [[1510621.69]] } });

    renderView();
    await openVizPanel();

    await vi.waitFor(() => expect(screen.getByLabelText("range 1 max")).toHaveValue("755310.85"));
    expect(screen.getByLabelText("range 1 min")).toHaveValue("0");
    expect(screen.getByLabelText("range 2 min")).toHaveValue("755310.85");
    expect(screen.getByLabelText("range 2 max")).toHaveValue("1510621.69");
    expect(screen.getByLabelText("range 3 min")).toHaveValue("1510621.69");
    expect(screen.getByLabelText("range 3 max")).toHaveValue("3021243.38");
  });

  /**
   * D2. A date authored in the builder could not be edited from the dashboard.
   *
   * `order_month` comes back `date`; the popover's date block is gated on `isTemporalType(c.type)`,
   * which an empty string fails. "Column title" is the vacuity guard — it is the one field that
   * shows for every type, so its presence proves the accordion really opened and "Date style is
   * absent" was a statement about the control rather than about an unexpanded row.
   */
  it("a date dimension's format popover offers the date fields, not Text-only", async () => {
    const user = userEvent.setup();
    charts = [{
      id: "w", name: "Waterfall", dataset_id: "d", viz_type: "waterfall",
      params: {
        version: 2,
        spec: { metrics: ["rev"], dimensions: ["order_month"], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [] },
        settings: { "waterfall.xAxis": "order_month", "waterfall.metric": "rev" },
      },
    }];
    cards = [chartCard("w")];
    chartsChRunSpec.mockResolvedValue({
      data: {
        columns: [{ name: "order_month", type: "date" }, { name: "rev", type: "double" }],
        rows: [["2026-01-01", 10]],
      },
    });

    renderView();
    await openVizPanel();

    await user.click(await screen.findByRole("button", { name: /^order_month/ }));
    expect(screen.getByText("Column title")).toBeInTheDocument();
    expect(screen.getByText("Date style")).toBeInTheDocument();
  });

  /**
   * D3. The pie's Slices list.
   *
   * Not a Metabase-parity fix: this Metabase build hides the settings button outright for a pie
   * dashcard, so it has no panel here to compare against. egret offers one, and it was inconsistent
   * with EGRET'S OWN builder route about what that panel contains. The fix is to make the two agree,
   * not to withdraw the panel.
   */
  it("a pie card lists its slices, the same per-slice rows the builder offers", async () => {
    charts = [{
      id: "p", name: "Pie", dataset_id: "d", viz_type: "pie",
      params: {
        version: 2,
        spec: { metrics: ["rev"], dimensions: ["region"], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [] },
        settings: { "pie.dimensions": ["region"], "pie.metric": "rev" },
      },
    }];
    cards = [chartCard("p")];
    chartsChRunSpec.mockResolvedValue({
      data: {
        columns: [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }],
        rows: [["North", 40], ["South", 30], ["East", 20], ["West", 10]],
      },
    });

    renderView();
    await openVizPanel();

    await vi.waitFor(() => expect(screen.getByLabelText("Show North")).toBeInTheDocument());
    expect(screen.getByLabelText("Show South")).toBeInTheDocument();
    expect(screen.getByLabelText("Show East")).toBeInTheDocument();
    expect(screen.getByLabelText("Show West")).toBeInTheDocument();
  });

  /**
   * THE PANEL MUST RUN THE SPEC THE CARD RAN — the parameter-INJECTED one, not the saved one.
   *
   * This is the difference between a panel that reads the card and a panel that reads the chart, and
   * it has two consequences at once. Correctness: with a dashboard filter narrowing the card to one
   * region, the gauge on screen is drawn on THAT region's total, so ranges derived from the
   * unfiltered total would disagree with the arc beside them — the same class of mismatch as the
   * zero-value defect, just with a less obvious wrong number. Cost: `useSpecData`'s query key is
   * `JSON.stringify(spec)`, so running the injected spec is a react-query CACHE HIT on the key the
   * edit-mode tile already holds, while running the saved spec would be a second round trip.
   *
   * Asserted on the request BODY rather than on a call count, because a count cannot tell the two
   * apart: the tile is stubbed out in this file, so either implementation would make exactly one
   * call. The filter in `query_spec.filters` is the discriminator — it exists only on the injected
   * spec.
   */
  it("runs the card's parameter-injected spec, the same key the card's own tile holds", async () => {
    charts = [{
      id: "g", name: "Gauge", dataset_id: "d", viz_type: "gauge",
      params: {
        version: 2,
        spec: { metrics: ["rev"], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [] },
        settings: { "gauge.metric": "rev" },
      },
    }];
    parameters = [{ id: "p1", name: "Region", slug: "region", type: "category", sectionId: "string" }];
    cards = [{ ...chartCard("g"), options: { paramMappings: { p1: "region" } } }];
    chartsChRunSpec.mockResolvedValue({ data: { columns: [{ name: "rev", type: "double" }], rows: [[42]] } });

    renderView(["North"]);
    await openVizPanel();
    await vi.waitFor(() => expect(screen.getByLabelText("range 3 max")).toHaveValue("84"));

    // NOT `calls[0]`, and not `.find()` either. A dashboard with a category parameter also runs a
    // DISTINCT-VALUES query for the filter dropdown (`useColumnValues` -> the same `useSpecData` ->
    // the same mocked function), and that one lands first — so an index or a first-match would have
    // asserted against the option list and reported the panel's spec as unfiltered. Select by the
    // metric, which only the card's own query carries, and pin the count so a future second card
    // query cannot make this silently ambiguous again.
    const cardRuns = chartsChRunSpec.mock.calls
      .map((c) => (c[1] as { query_spec: { metrics: string[]; filters: unknown[] } }).query_spec)
      .filter((s) => s.metrics.includes("rev"));
    expect(cardRuns).toHaveLength(1);
    expect(cardRuns[0].filters).toStrictEqual([{ col: "region", op: "in", value: ["North"] }]);
  });
});

/**
 * DISPLAY SETTINGS THAT WERE FILED ON THE DATA SIDE OF THE CARD PANEL'S LINE.
 *
 * `CardVizPanel` deliberately offers presentation overrides and never data overrides — a card may
 * restyle its chart, but it may not silently re-point the query the dashboard's other cards and
 * filters are reasoning about. That line is worth keeping. What went wrong is which side two
 * settings were filed on.
 *
 * `StudioPanel`'s only filter is `VizControl.section`, and `section` means WHICH BUILDER TAB — that
 * is its sole consumer (`ChartExplore.tsx` renders `tab="data" | "display" | "axes"` and the panel
 * filters each on `section`). The card panel then borrowed the same filter as a proxy for "is this a
 * data setting", which held only for as long as the two coincided. These two are where they part:
 *
 *   - F1, the trend's Comparisons. `smartscalar`'s `buildSpec` reads `dimension` and `metric` and
 *     nothing else; `computeTrend` is pure over the rows the card already fetched and no comparison
 *     type issues a second query. It is on the Data TAB because that is where Metabase puts it, and
 *     that placement is correct — so this is not a reclassification. Moving it to `section:
 *     "display"` would drag it into the builder's Display tab and break the parity that put it on
 *     Data in the first place (`fields.tsx` records that decision at the `data` field it declares).
 *   - F2, the map's region choice. `mapDescriptor.buildSpec` reads `cfg.metric` and `cfg.region`;
 *     `mapKey` picks which geojson the SAME rows are painted onto. It was excluded because it shared
 *     one control with region and metric, which genuinely are the query.
 *
 * So the fix is an explicit ALLOW-LIST, exactly the shape #427 gave the Axes group: the descriptor
 * says a data-tab control touches no query input (`cardOverridable`), and the card panel asks for
 * those by name. Nothing moves in the builder.
 *
 * DRIVEN AT THE ROUTE, per #427's lesson: a `CardVizPanel` render with hand-written props is not the
 * broken seam. And each test carries the override through to the CARD — `data-viz-override` on the
 * stubbed tile is the prop `applyVizOverride` consumes — so "the control appeared" is never the
 * whole claim.
 */
describe("DashboardView — presentation settings filed under the Data tab still reach the card", () => {
  /**
   * F1. Metabase's dashcard modal for its trend has `Data | Display`, and Data holds
   * "Comparisons — Previous month — Add comparison". egret has the identical control in the builder
   * and it never reached the card, with nothing on screen hinting it existed — so a trend on a
   * dashboard could not be re-based at all.
   *
   * `temporal_units` is on the fixture because two of the four comparison types are gated on it
   * (`TrendComparisonsSection` omits them with no unit), and "Previous month" is the option Metabase
   * shows for this exact shape — so a panel that got the units wrong would offer a different menu.
   */
  it("a trend card offers Comparisons, and adding one lands on the card", async () => {
    const user = userEvent.setup();
    charts = [{
      id: "t", name: "Trend", dataset_id: "d", viz_type: "smartscalar",
      params: {
        version: 2,
        spec: {
          metrics: ["rev"], dimensions: ["order_month"], columns: [], filters: [], orderby: [],
          row_limit: 100, rollup_groups: [], temporal_units: { order_month: "month" },
        },
        settings: { "smartscalar.dimension": "order_month", "smartscalar.metric": "rev" },
      },
    }];
    cards = [chartCard("t")];
    chartsChRunSpec.mockResolvedValue({
      data: {
        columns: [{ name: "order_month", type: "date" }, { name: "rev", type: "double" }],
        rows: [["2026-01-01", 100], ["2026-02-01", 120]],
      },
    });

    renderView();
    await openVizPanel();

    // Vacuity guard: the trend's own DISPLAY control is present, so the panel really did render this
    // descriptor and an absent Comparisons is a statement about the filter rather than about a blank
    // sidebar.
    expect(screen.getByLabelText("Compact number")).toBeInTheDocument();

    expect(await screen.findByText("Comparisons")).toBeInTheDocument();
    const add = screen.getByRole("button", { name: /Add comparison/ });
    expect(add).toBeEnabled();

    // …and it drives the card. One real click — two dispatched in a single tick share a stale
    // `override` closure, which is a measurement artifact this programme has already been bitten by.
    await user.click(add);

    await vi.waitFor(() => {
      const stored = JSON.parse(screen.getByTestId("disp-c1").getAttribute("data-viz-override")!);
      expect(stored["smartscalar.comparisons"]).toHaveLength(1);
      // Metabase's default for a month-bucketed trend, which is what "Add comparison" appends.
      expect(stored["smartscalar.comparisons"][0].type).toBe("previousPeriod");
    });
  });

  /**
   * F2. Metabase's Map dashcard modal offers Map type, Region map, Metric field, Region field and
   * Color; egret's offered Color alone, so a US-states card could not be shown as World or Vietnam.
   *
   * The second half of this test is the half that constrains the fix: region and metric ARE the
   * query and must stay excluded. A split that dropped all three into the card panel would pass the
   * first assertion and quietly hand a dashboard the power to re-point its cards' queries.
   */
  it("a map card can be switched to another map, and still cannot re-point its query", async () => {
    const user = userEvent.setup();
    charts = [{
      id: "m", name: "Map", dataset_id: "d", viz_type: "map",
      params: {
        version: 2,
        spec: { metrics: ["rev"], dimensions: ["state"], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [] },
        settings: { "map.key": "us_states", "map.region": "state", "map.metric": "rev" },
      },
    }];
    cards = [chartCard("m")];
    chartsChRunSpec.mockResolvedValue({
      data: { columns: [{ name: "state", type: "varchar" }, { name: "rev", type: "double" }], rows: [["CA", 10]] },
    });

    renderView();
    await openVizPanel();

    // Vacuity guard: the map's existing display control, the one thing this panel already offered.
    expect(screen.getByLabelText("Color")).toBeInTheDocument();

    const mapSelect = await screen.findByLabelText("Map");
    // The two query inputs stay out. Region and metric decide what the card ASKS FOR, and a
    // dashboard card may restyle its chart without re-pointing it.
    expect(screen.queryByLabelText("Region")).toBeNull();
    expect(screen.queryByLabelText("Metric")).toBeNull();

    await user.click(mapSelect);
    await user.click(await screen.findByRole("option", { name: "Vietnam provinces" }));

    await vi.waitFor(() => {
      const stored = JSON.parse(screen.getByTestId("disp-c1").getAttribute("data-viz-override")!);
      expect(stored["map.key"]).toBe("vietnam");
      /**
       * REGION AND METRIC RIDE ALONG UNCHANGED, and asserting they were ABSENT here is what this
       * test tried first — it failed, and the brief's framing ("nothing about the query rode
       * along") is what was wrong, not the code.
       *
       * An override is a full settings SNAPSHOT for the descriptor's prefix, not a diff:
       * `mergeVizSettings` spreads `descriptor.toSettings(effectiveConfig)` over what was stored,
       * so every key the viz owns is written on every edit. That predates this change and is
       * shared with the Display path — touching the map's Color has always stamped `map.region`
       * and `map.metric` into the override too. Nothing here makes it truer or falser.
       *
       * So the invariant worth pinning is not absence but IMMOVABILITY: the values written are the
       * chart's own, and — with no Region or Metric control on this panel, asserted above — there
       * is no path by which they could be anything else. `applyVizOverride` is the backstop that
       * makes this hold for the query rather than merely for the drawing: it destructures `spec`
       * out of the override and throws it away (`params.ts`), so a card's query spec is not
       * reachable from an override at all, whatever settings it carries.
       */
      expect(stored["map.region"]).toBe("state");
      expect(stored["map.metric"]).toBe("rev");
    });
  });

  /**
   * F3, THE THIRD INSTANCE — found by sweeping the descriptors while here, and kept because it is
   * MEASURED rather than merely plausible. Metabase's own source settles it without touching the
   * parity dashboard: `Funnel.tsx` gives all three of `funnel.dimension`, `funnel.metric` and
   * `funnel.rows` `getSection: () => t`Data``, and marks only the first two `dashboard: false` —
   * the flag whose contract is spelled out in `settings/typed-utils.ts` ("Settings with
   * `dashboard: false` are hidden from dashboard UI"). So Metabase puts the step list on the Data
   * tab AND offers it on a dashcard, which is exactly the combination egret could not express.
   *
   * egret's own test agrees: the funnel's `buildSpec` is `h.dimsAndMetrics([c.dimension],
   * [c.metric], s)`, and `stepOrder`/`hiddenSteps` appear nowhere in it — the reorder and the hide
   * are applied when the funnel is drawn.
   *
   * THAT METABASE MODELS THIS WITH TWO INDEPENDENT FIELDS IS THE POINT, and it is the strongest
   * evidence for the mechanism this PR chose: `getSection` and `dashboard` are separate in
   * Metabase because a tab and an eligibility are separate questions. egret had one field doing
   * both jobs, which is why all three of these defects exist.
   */
  it("a funnel card can reorder and hide its steps, and the change lands on the card", async () => {
    const user = userEvent.setup();
    charts = [{
      id: "f", name: "Funnel", dataset_id: "d", viz_type: "funnel",
      params: {
        version: 2,
        spec: { metrics: ["rev"], dimensions: ["step"], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [] },
        settings: { "funnel.dimension": "step", "funnel.metric": "rev" },
      },
    }];
    cards = [chartCard("f")];
    chartsChRunSpec.mockResolvedValue({
      data: {
        columns: [{ name: "step", type: "varchar" }, { name: "rev", type: "double" }],
        rows: [["Visit", 100], ["Cart", 60], ["Buy", 20]],
      },
    });

    renderView();
    await openVizPanel();

    // Vacuity guard: the funnel's DISPLAY control, which this panel already offered.
    expect(screen.getByLabelText("Funnel type")).toBeInTheDocument();

    expect(await screen.findByLabelText("Show Visit")).toBeInTheDocument();
    expect(screen.getByLabelText("Show Cart")).toBeInTheDocument();
    expect(screen.getByLabelText("Show Buy")).toBeInTheDocument();

    await user.click(screen.getByLabelText("Show Cart"));

    await vi.waitFor(() => {
      const stored = JSON.parse(screen.getByTestId("disp-c1").getAttribute("data-viz-override")!);
      expect(stored["funnel.hiddenSteps"]).toStrictEqual(["Cart"]);
    });
    // The step column itself stays out — it IS the query, exactly as Metabase's `dashboard: false`
    // on `funnel.dimension` says.
    expect(screen.queryByLabelText("Stage (dimension)")).toBeNull();
  });
});
