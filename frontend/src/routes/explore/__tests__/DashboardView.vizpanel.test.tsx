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
vi.mock("@/components/chart/dashboard/DashboardCardView", () => ({
  DashboardCardView: ({ card }: { card: { id: string } }) => <div data-testid={`disp-${card.id}`} />,
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
