import { describe, it, expect, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement } from "react";
import { cartesianDescriptor } from "../cartesian";
import { EMPTY_SPEC } from "@/api/charts";
import type { CartesianConfig } from "@/api/charts";
import type { VizControlProps } from "../types";

const SHARED = { filters: [], rowLimit: 1000, temporalUnits: {}, orderby: [] };

/**
 * THE PANEL AND THE QUERY HALF of the additional tooltip metrics. The builder half is
 * `echarts/__tests__/cartesian.tooltipMetrics.test.ts`; the two are separate because the seam
 * between them is the whole feature — a metric reaches the tooltip only if `buildSpec` asked the
 * engine for it, and a control that writes a key `buildSpec` does not read is the "saves and does
 * not paint" defect with an extra step.
 */

describe("buildSpec fetches the tooltip metrics without plotting them", () => {
  it("appends them to spec.metrics, after the plotted ones", () => {
    const spec = cartesianDescriptor("bar").buildSpec(
      { xAxis: "region", yMetrics: ["total_revenue"], tooltipMetrics: ["avg_price"] },
      SHARED,
    );
    expect(spec.metrics).toEqual(["total_revenue", "avg_price"]);
  });

  /** The grouping is the whole reason this is possible: an extra METRIC is another aggregate over
   *  the same groups, where Metabase's extra COLUMN would have had to join the `GROUP BY` and make
   *  a different chart. `chartquery.rs` takes the grouping from `spec.dimensions` alone. */
  it("leaves the dimensions — and so the grouping — untouched", () => {
    const spec = cartesianDescriptor("bar").buildSpec(
      { xAxis: "region", yMetrics: ["total_revenue"], tooltipMetrics: ["avg_price"] },
      SHARED,
    );
    expect(spec.dimensions).toEqual(["region"]);
    expect(spec.columns).toEqual([]);
  });

  /** `find_metric` errors with `unknown metric: x` on a duplicate-free list either way, but a name
   *  sent twice would be SELECTed twice under one alias — the engine's own duplicate-column error,
   *  from a config the picker can reach (a metric can sit in both keys if one is added after the
   *  other). */
  it("sends a metric that is also plotted exactly once", () => {
    const spec = cartesianDescriptor("bar").buildSpec(
      { xAxis: "region", yMetrics: ["total_revenue"], tooltipMetrics: ["total_revenue", "avg_price"] },
      SHARED,
    );
    expect(spec.metrics).toEqual(["total_revenue", "avg_price"]);
  });

  /** The list dedupes against ITSELF as well as against the plotted metrics, and the two are
   *  separate clauses that can be lost separately — removing only `all.indexOf(m) === i` left every
   *  assertion here passing. A stored list is JSON and a hand-written dashboard-card override is
   *  under no obligation to be a set; the same alias twice is the engine's own duplicate-column
   *  error, from a state nothing in the UI prevents. */
  it("sends a metric named twice in the tooltip list exactly once", () => {
    const spec = cartesianDescriptor("bar").buildSpec(
      { xAxis: "region", yMetrics: ["total_revenue"], tooltipMetrics: ["avg_price", "avg_price"] },
      SHARED,
    );
    expect(spec.metrics).toEqual(["total_revenue", "avg_price"]);
  });

  it("does not duplicate a scatter's x metric or bubble metric", () => {
    const spec = cartesianDescriptor("scatter").buildSpec(
      { xAxis: "region", xMetric: "order_count", bubbleMetric: "avg_price",
        yMetrics: ["total_revenue"], tooltipMetrics: ["order_count", "avg_price", "margin"] },
      SHARED,
    );
    expect(spec.metrics).toEqual(["order_count", "total_revenue", "avg_price", "margin"]);
  });

  /**
   * NOT FETCHED UNDER A BREAKOUT, which is the other half of the builder's decision not to draw
   * them there. Fetching a metric the tooltip will not show is exactly the defect this PR's other
   * half fixes on the scatter — paying for a column and discarding it — so the two states have to
   * agree, and this is the assertion that keeps them agreeing.
   */
  it("asks for nothing extra while a breakout names the series", () => {
    const spec = cartesianDescriptor("bar").buildSpec(
      { xAxis: "region", series: "category", yMetrics: ["total_revenue"], tooltipMetrics: ["avg_price"] },
      SHARED,
    );
    expect(spec.metrics).toEqual(["total_revenue"]);
  });

  it("changes nothing when the key is absent or empty", () => {
    const base = cartesianDescriptor("bar").buildSpec({ xAxis: "region", yMetrics: ["m"] }, SHARED);
    const empty = cartesianDescriptor("bar").buildSpec({ xAxis: "region", yMetrics: ["m"], tooltipMetrics: [] }, SHARED);
    expect(empty).toStrictEqual(base);
  });
});

describe("the key survives the codec", () => {
  /** A new stored key needs a `KEYS` entry in `cartesian-codec.ts` AND its own field on
   *  `CartesianConfig`; the codec entry alone is necessary and not sufficient, and a `KEYS` list
   *  that compiles is not the same as one that round-trips. */
  it("round-trips through settings", () => {
    const d = cartesianDescriptor("bar");
    const cfg: CartesianConfig = { xAxis: "region", yMetrics: ["m"], tooltipMetrics: ["avg_price"] };
    expect(d.toSettings(cfg)["cartesian.tooltipMetrics"]).toEqual(["avg_price"]);
    expect(d.fromParams({ version: 2, spec: EMPTY_SPEC, settings: d.toSettings(cfg) })).toEqual(cfg);
  });
});

const props = (config: CartesianConfig, over: Partial<VizControlProps<CartesianConfig>> = {})
  : VizControlProps<CartesianConfig> => ({
  config,
  onChange: () => {},
  columns: [{ column_name: "region", data_type: "varchar" }] as never,
  metrics: [
    { metric_name: "total_revenue" }, { metric_name: "avg_price" }, { metric_name: "margin" },
  ] as never,
  resultColumns: [], rows: [], temporalUnits: {}, columnSettings: {},
  unitRow: () => null,
  ...over,
});

const find = (kind: Parameters<typeof cartesianDescriptor>[0]) =>
  cartesianDescriptor(kind).controls.find((c) => c.id === "tooltipMetrics");

describe("the Additional tooltip metrics control", () => {
  /**
   * ON THE DATA TAB, WHERE METABASE PUTS IT UNDER DISPLAY, and the divergence is the point rather
   * than an oversight. Metabase's `graph.tooltip_columns` is `getSection: () => t\`Display\``
   * (`lib/settings/graph.ts:421`) and it is right there, because that setting picks among columns
   * the result ALREADY HAS and never touches the query. egret's picks a metric the query must be
   * changed to fetch, which makes it a query input — and query inputs live on the Data tab, beside
   * the Y-axis zone whose list this one is the complement of.
   *
   * IT IS ALSO WHAT KEEPS A DASHBOARD CARD OUT OF IT. `applyVizOverride` merges a card's override
   * into `settings` and states outright that "the query `spec` is never overridden" — so a card
   * that could set `cartesian.tooltipMetrics` would store a metric its own stored spec never
   * fetches, and paint nothing. `CardVizPanel` renders the Display section whole and narrows the
   * Data section to the controls flagged `cardOverridable`; declaring this one on Data and leaving
   * that flag off is what makes the card panel decline it, by the rule rather than by a special case.
   */
  it("sits on the data tab and is not offered to a dashboard card", () => {
    for (const kind of ["bar", "row", "line", "area", "combo", "scatter"] as const) {
      const c = find(kind);
      expect(c, `${kind} should declare tooltipMetrics`).toBeDefined();
      expect(c!.section).toBe("data");
      expect(c!.cardOverridable).toBeUndefined();
    }
  });

  it("lists the dataset metrics the chart does not already plot", async () => {
    render(createElement(find("bar")!.render, props({ xAxis: "region", yMetrics: ["total_revenue"] })));
    await userEvent.click(screen.getByRole("button", { name: /add tooltip metrics/i }));
    expect(screen.getByRole("button", { name: "avg_price" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "margin" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "total_revenue" })).not.toBeInTheDocument();
    cleanup();
  });

  it("writes the chosen metric to the config", async () => {
    const onChange = vi.fn();
    render(createElement(find("bar")!.render,
      props({ xAxis: "region", yMetrics: ["total_revenue"] }, { onChange })));
    await userEvent.click(screen.getByRole("button", { name: /add tooltip metrics/i }));
    await userEvent.click(screen.getByRole("button", { name: "avg_price" }));
    expect(onChange).toHaveBeenCalledWith({ tooltipMetrics: ["avg_price"] });
    cleanup();
  });

  it("keeps a scatter's x metric and bubble metric out of the list", async () => {
    render(createElement(find("scatter")!.render, props({
      xAxis: "region", xMetric: "avg_price", bubbleMetric: "margin", yMetrics: ["total_revenue"],
    })));
    expect(screen.queryByRole("button", { name: /add tooltip metrics/i })).not.toBeInTheDocument();
    cleanup();
  });

  /**
   * THE EMPTY-LIST DECISION, and it is Metabase's own rule rather than an egret invention:
   * `graph.tooltip_columns` carries `getHidden: (rawSeries, vizSettings) =>
   * getAvailableAdditionalColumns(rawSeries, vizSettings).length === 0` (`graph.ts:431`). PR #431
   * objected to controls that vanish as a neighbouring select changes, and the objection is right
   * about the case it names — a control disappearing WITH A VALUE IN IT. Metabase never has that
   * case, because `getComputedAdditionalColumnsValue` filters the stored value down to what is
   * still available before the widget ever reads it (`cartesian-chart.ts:494-498`).
   *
   * egret does not filter the stored value — a chart's settings are stored JSON and rewriting one
   * at render time is how a user's config quietly loses things. So the port is the rule AND the
   * guarantee that makes it safe: hidden only when there is nothing to offer AND nothing to show.
   */
  it("is hidden when the dataset has no other metric to offer", () => {
    const one = [{ metric_name: "total_revenue" }] as never;
    const { container } = render(createElement(find("bar")!.render,
      props({ xAxis: "region", yMetrics: ["total_revenue"] }, { metrics: one })));
    expect(container).toBeEmptyDOMElement();
    cleanup();
  });

  it("still shows a stored metric it can no longer offer, so it can be taken off", () => {
    const one = [{ metric_name: "total_revenue" }] as never;
    render(createElement(find("bar")!.render, props(
      { xAxis: "region", yMetrics: ["total_revenue"], tooltipMetrics: ["total_revenue"] },
      { metrics: one },
    )));
    expect(screen.getByRole("button", { name: /remove total_revenue/i })).toBeInTheDocument();
    cleanup();
  });

  /** Under a breakout the chips stay editable and the panel SAYS why they are not drawn, rather
   *  than withdrawing the row — the same shape `SeriesControl` already uses one control up ("With a
   *  series, only the first metric is plotted."). */
  it("says why it is inert under a breakout instead of disappearing", () => {
    render(createElement(find("bar")!.render, props({
      xAxis: "region", series: "category", yMetrics: ["total_revenue"], tooltipMetrics: ["avg_price"],
    })));
    expect(screen.getByText(/breakout/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /remove avg_price/i })).toBeInTheDocument();
    cleanup();
  });
});
