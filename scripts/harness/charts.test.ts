/**
 * What this file defends is the harness's SHAPE — that the 19 entries still describe the same
 * dashboard, and that the four defects PR 07 found stay fixed.
 *
 * Three of those defects (`MB · Scalar`, `MB · Gauge`, `MB · Progress`) were invisible for six
 * PRs because the charts rendered a plausible-looking number. The number was one arbitrary
 * category's revenue, and which category it was changed between runs. So the assertions here are
 * about the SPEC those configs produce — `dimensions: []`, `row_limit: 1` — not about the config
 * fields, which never looked wrong.
 *
 * `describe`/`it`/`expect` come from vitest's globals; see `params.test.ts` for why a bare
 * `from "vitest"` cannot resolve from outside `frontend/`.
 */
import { CHARTS, DASHBOARD, chartByName, chartParams, metricUses } from "./charts";
import { DATASETS, assertMetricsDeclared } from "./datasets";

/**
 * WHY THIS SAYS 22 AND NOT 19. The dashboard was nineteen cards, one per viz type, and that second
 * clause was the invariant this block asserted. It is no longer true and could not be kept: the
 * three breakout cards added for PR #436's blind spot are a `bar`, an `area` and a second `bar`,
 * because a breakout is a property of a chart's QUERY rather than a viz type of its own and there
 * is no twentieth type to give it. What survives, and is asserted below instead, is the direction
 * that actually mattered — EVERY VIZ TYPE STILL APPEARS. A type dropped from the harness is a hole
 * in the instrument; a type appearing twice is deliberate coverage.
 */
describe("the 22 charts", () => {
  it("covers all 19 viz types, and no card is on a type the harness dropped", () => {
    expect(CHARTS).toHaveLength(22);
    expect(new Set(CHARTS.map((c) => c.viz)).size).toBe(19);
  });

  it("has unique names", () => {
    expect(new Set(CHARTS.map((c) => c.name)).size).toBe(CHARTS.length);
  });

  it("only names datasets that exist", () => {
    const known = new Set(DATASETS.map((d) => d.name));
    for (const c of CHARTS) expect(known, `${c.name} → ${c.dataset}`).toContain(c.dataset);
  });

  it("builds params for every one without throwing", () => {
    // `harnessParams` throws when a config produces no settings in its viz's own namespace —
    // the shape that made three table/object charts open on "Set up your Table" in PR 06.
    for (const c of CHARTS) expect(() => chartParams(c), c.name).not.toThrow();
  });

  it("writes at least one setting in each viz's own namespace", () => {
    for (const c of CHARTS) {
      const own = Object.keys(chartParams(c).settings)
        .filter((k) => k !== "column_settings" && k !== "conditional_formatting");
      expect(own.length, `${c.name} wrote no viz settings`).toBeGreaterThan(0);
    }
  });
});

describe("the grid geometry PRs 01–06 measured against", () => {
  it("is a uniform 4×5 on a 12-column grid, three per row", () => {
    for (const c of CHARTS) {
      expect(c.sizeX, c.name).toBe(4);
      expect(c.sizeY, c.name).toBe(5);
      expect([0, 4, 8], c.name).toContain(c.col);
      expect(c.row % 5, c.name).toBe(0);
    }
  });

  it("has no two cards in the same cell", () => {
    const cells = CHARTS.map((c) => `${c.col},${c.row}`);
    expect(new Set(cells).size).toBe(cells.length);
  });

  it("keeps the dashboard's identity", () => {
    expect(DASHBOARD.name).toBe("ZZ Chart Parity");
    expect(DASHBOARD.width).toBe("full");
  });
});

describe("the three charts PR 07 corrected", () => {
  // The defect: a stored spec of `dimensions: ["category"], row_limit: 1000` on a viz whose
  // renderer reads `rows[0]`. Five identical runs returned Widget, Widget, Gizmo, Gizmo, Widget.
  //
  // HONEST ABOUT WHAT THIS CATCHES: `dimensions: []` and `row_limit: 1` are guaranteed by the
  // three descriptors' `oneRow` — no config this file could write makes them come out otherwise.
  // These assertions therefore guard the `viz` field and a descriptor regression, NOT the config,
  // and they are here because the old shape is not reachable rather than because it is guarded.
  // The assertions with teeth are the bounds below, which a config edit can and did get wrong.
  for (const name of ["MB · Scalar", "MB · Gauge", "MB · Progress"]) {
    it(`${name} asks for one ungrouped row`, () => {
      const spec = chartParams(chartByName(name)).spec;
      expect(spec.dimensions, `${name} must not group`).toEqual([]);
      expect(spec.row_limit, `${name} must ask for one row`).toBe(1);
      expect(spec.metrics).toEqual(["revenue"]);
    });
  }

  it("moves the gauge and progress bounds with the metric they measure", () => {
    // 500,000 was chosen against one category's ~423K. Against the real 1,510,621.69 total it
    // would peg at 300%, which is how a wrong bound looks when the metric underneath changes.
    expect(chartParams(chartByName("MB · Gauge")).settings["gauge.max"]).toBe(2_000_000);
    expect(chartParams(chartByName("MB · Gauge")).settings["gauge.min"]).toBe(0);
    expect(chartParams(chartByName("MB · Progress")).settings["progress.goal"]).toBe(2_000_000);
  });

  it("MB · Trend still groups by month — it is a trend, not a scalar", () => {
    const spec = chartParams(chartByName("MB · Trend")).spec;
    expect(spec.dimensions).toEqual(["order_month"]);
    expect(spec.temporal_units).toEqual({ order_month: "month" });
  });
});

describe("MB · Table no longer stores settings: {}", () => {
  // Trap 4. PR 06 made the app recover from it; this makes the harness stop relying on that.
  it("names its ten columns in its own settings, not only in the spec", () => {
    const p = chartParams(chartByName("MB · Table"));
    expect(p.settings["table.columns"]).toEqual([
      "id", "created_at", "title", "category", "vendor",
      "subtotal", "discount", "total", "quantity", "state",
    ]);
    expect(p.spec.columns).toEqual(p.settings["table.columns"]);
    expect(p.spec.row_limit).toBe(60);
  });

  it("the object chart shows the same ten, from the same source", () => {
    expect(chartParams(chartByName("MB · Object")).settings["object.columns"])
      .toEqual(chartParams(chartByName("MB · Table")).settings["table.columns"]);
  });
});

describe("the charts the earlier PRs repaired stay repaired", () => {
  it("the pivot carries real rollup_groups, not [] — PR 05b", () => {
    const spec = chartParams(chartByName("MB · Pivot")).spec;
    expect(spec.rollup_groups).toEqual([["order_month"], ["category"]]);
    expect(spec.temporal_units).toEqual({ order_month: "month" });
  });

  it("every temporal x-axis defaults to monthly grouping — PR 03", () => {
    for (const name of ["MB · Line", "MB · Area", "MB · Combo", "MB · Waterfall"]) {
      expect(chartParams(chartByName(name)).spec.temporal_units, name)
        .toEqual({ order_month: "month" });
    }
  });

  it("the pie's settings key is plural", () => {
    const s = chartParams(chartByName("MB · Pie")).settings;
    expect(s).toHaveProperty("pie.dimensions", ["category"]);
    expect(s).not.toHaveProperty("pie.dimension");
  });

  it("the funnel keeps its pipeline order rather than sorting by value — PR 05b", () => {
    expect(chartParams(chartByName("MB · Funnel")).settings["funnel.stepOrder"])
      .toEqual(["Visited", "Added to cart", "Checkout", "Purchased"]);
  });
});

/**
 * WHAT THE SEED EMITS MOVED, ON PURPOSE. Every cartesian chart's spec used to carry `orderby: []`,
 * which let the engine return the categories in whatever order it liked — measured at 7/10 one
 * ordering and 3/10 another on `MB · Bar`. `cartesianDescriptor`'s `buildSpec` now derives an
 * ascending order on the x-axis, so these specs are no longer the ones currently stored in `demo`:
 * the harness and `demo` diverge until the seed is re-run.
 *
 * Ascending by the DIMENSION because that is Metabase's default — a fresh summarise-by carries no
 * `order-by` at all, and its query processor adds `:asc` on each breakout column
 * (`add-implicit-breakout-order-by`). Descending by the aggregation is only what an author writes,
 * as this harness's own `MB By Category` question does with `ORDER BY revenue DESC`.
 */
describe("the seed asks for the order each chart draws in", () => {
  it("MB · Bar orders by its category, ascending", () => {
    expect(chartParams(chartByName("MB · Bar")).spec.orderby).toEqual([{ field: "category", desc: false }]);
  });

  // D7 / KI-045: the row chart is the one cartesian that ranks — largest first, as egret's gallery
  // promises — with the category kept as the tie-breaker. A deliberate divergence from Metabase's
  // implicit ascending default.
  it("MB · Row orders by its metric, descending, then by its category", () => {
    expect(chartParams(chartByName("MB · Row")).spec.orderby)
      .toEqual([{ field: "revenue", desc: true }, { field: "category", desc: false }]);
  });

  it("every un-broken-out cartesian chart but the row orders by its own x-axis, and by nothing else", () => {
    // The single-clause shape is the point: with one breakout there is one implicit order-by. The
    // broken-out cards have two, which is `add-implicit-breakout-order-by` reducing over every
    // breakout — asserted in `charts.breakout.test.ts` rather than here.
    const expected: Record<string, string> = {
      "MB · Bar": "category", "MB · Line": "order_month",
      "MB · Area": "order_month", "MB · Combo": "order_month", "MB · Scatter": "price",
    };
    for (const [name, field] of Object.entries(expected)) {
      expect(chartParams(chartByName(name)).spec.orderby, name).toEqual([{ field, desc: false }]);
    }
  });

  // The non-goal, pinned: the other grouped charts sort themselves client-side and were left alone,
  // because adding an `orderby` to eleven more `buildSpec`s changes which rows survive `row_limit`
  // on eleven chart types at once and needs its own comparison.
  it("leaves the charts that sort themselves untouched", () => {
    for (const name of ["MB · Pie", "MB · Funnel", "MB · Waterfall", "MB · Sankey", "MB · Map", "MB · Pivot"]) {
      expect(chartParams(chartByName(name)).spec.orderby, name).toEqual([]);
    }
    // The trend already derived its own, and still does.
    expect(chartParams(chartByName("MB · Trend")).spec.orderby)
      .toEqual([{ field: "order_month", desc: false }]);
  });
});

describe("metricUses feeds the undeclared-metric invariant", () => {
  it("derives the metric list from the same buildSpec that writes the saved spec", () => {
    const uses = metricUses();
    expect(uses).toHaveLength(CHARTS.length);
    const combo = uses.find((u) => u.chart === "MB · Combo")!;
    expect(combo.metrics).toEqual(["orders", "revenue"]);
    // The table and object charts aggregate nothing, and MB Raw Orders declares no metrics.
    expect(uses.find((u) => u.chart === "MB · Table")!.metrics).toEqual([]);
    expect(uses.find((u) => u.chart === "MB · Boxplot")!.metrics).toEqual([]);
  });

  it("every metric all 19 charts name is declared by its dataset", () => {
    // This is the check `seed.ts` runs before it opens a connection. A chart naming a bare
    // column saves with a 200 and renders "unknown metric" only when someone opens it.
    expect(() => assertMetricsDeclared(metricUses())).not.toThrow();
  });
});

describe("chartByName", () => {
  it("throws for a name that is not in the harness", () => {
    expect(() => chartByName("MB · Nope")).toThrow(/no harness chart named "MB · Nope"/);
  });
});
