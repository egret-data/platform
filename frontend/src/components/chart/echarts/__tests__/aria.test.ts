import { describe, it, expect } from "vitest";
import { ariaDescription } from "../base";
import { buildPieOption } from "../options/pie";
import { buildCartesianOption } from "../options/cartesian";
import { buildWaterfallOption } from "../options/waterfall";
import { buildBoxplotOption } from "../options/boxplot";
import { registerEChartsModules } from "../register";

const fmt = (v: unknown) => `${Number(v).toLocaleString()}`;

describe("ariaDescription", () => {
  it("says what the chart is and reads the values with the chart's own formatting", () => {
    const a = ariaDescription({
      kind: "Bar chart", metric: "total_revenue", dimension: "region", format: fmt,
      points: [{ name: "North", value: 27880 }, { name: "South", value: 30137 }],
    });
    expect(a.label.description).toBe(
      "Bar chart of total_revenue by region. 2 values: North 27,880, South 30,137.",
    );
  });

  it("keeps the screen-reader description on, and the decal patterns off", () => {
    // The two halves of `aria` are separable and were separated in chart 05: the
    // description is the part that earns its keep, while the patterns covered the
    // fill and matched neither Metabase nor Superset.
    const a = ariaDescription({ kind: "Pie chart", format: fmt, points: [] });
    expect(a.enabled).toBe(true);
    expect(a.decal).toEqual({ show: false });
  });

  it("mentions the series count when more than one is plotted", () => {
    const a = ariaDescription({
      kind: "Line chart", format: fmt, seriesCount: 3,
      points: [{ name: "Jan", value: 1 }],
    });
    expect(a.label.description).toContain("3 series;");
  });

  it("caps the enumeration instead of reading hundreds of values", () => {
    const points = Array.from({ length: 25 }, (_, i) => ({ name: `c${i}`, value: i }));
    const a = ariaDescription({ kind: "Bar chart", format: fmt, points });
    expect(a.label.description).toContain("and 5 more");
  });

  it("says so when there is nothing to describe", () => {
    expect(ariaDescription({ kind: "Bar chart", format: fmt, points: [] }).label.description)
      .toContain("No data.");
  });
});

describe("charts carry a description rather than ECharts' generated one", () => {
  const columns = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }];
  const rows = [["North", 10], ["South", 20]];

  it("pie", () => {
    const o = buildPieOption(columns, rows, { dimensions: ["region"], metric: "rev" }) as {
      aria: { label: { description: string } };
    };
    expect(o.aria.label.description).toContain("Pie chart of rev by region");
  });

  it("cartesian", () => {
    const o = buildCartesianOption(columns, rows, { kind: "bar", xAxis: "region", yMetrics: ["rev"] }) as {
      aria: { label: { description: string } };
    };
    expect(o.aria.label.description).toContain("Bar chart");
    // ECharts' own text reads "the data for North is 0, 10" — index first. Ours does not.
    expect(o.aria.label.description).not.toMatch(/is \d+, \d+/);
  });
});

/**
 * A SCREEN READER HEARS WHAT THE AXIS DRAWS — the period name, not the bucket it was truncated to.
 *
 * `ariaDescription`'s whole premise, stated in its own header, is that it "says what a sighted user
 * sees". On a month-bucketed axis that premise was false: the axis drew "January 2026" through
 * `drawLabel` while the description enumerated `String(c)` — the raw `2026-01-01` the query
 * returned. Same for the dimension it names, which read the COLUMN NAME while the axis title beside
 * it read the column's `column_title`.
 *
 * FIXTURE BLINDNESS is the trap in every case below, and it is why each one names both strings:
 * a fixture whose raw value and drawn label coincide ("Jan", "North") cannot fail for this defect
 * at all. `2026-01-01` and `January 2026` share no substring, so `toContain` and `not.toContain`
 * pin the two ends of the same assertion and neither passes by accident.
 *
 * WHAT IS DELIBERATELY NOT ASSERTED HERE: the tooltip. Both this family and the waterfall still
 * head their tooltip with the raw bucket, and that is a standing decision recorded at
 * `options/waterfall.ts`'s tooltip formatter — a tooltip sits next to a drill, and what a click
 * FILTERS by is the raw bucket. The `aria` description has no click behind it, which is why it
 * moves and the tooltip does not. Measured, not assumed: ECharts builds `params.axisValueLabel`
 * from `axis.scale.getLabel` (`component/axisPointer/viewHelper.js`), which on an OrdinalScale
 * returns the `xAxis.data` entry verbatim and never consults `axisLabel.formatter`.
 */
describe("a bucketed temporal axis is described the way it is drawn", () => {
  const dateCols = [{ name: "order_date", type: "date" }, { name: "rev", type: "double" }];
  const dateRows = [["2026-01-01", 10], ["2026-02-01", 20]];
  type Described = {
    aria: { label: { description: string } };
    xAxis: { name?: string; data?: unknown[]; axisLabel?: { formatter?: (v: string) => string } };
    series: { name?: string; __raw?: string }[];
  };

  it("names the period the bucket covers, on the cartesian family", () => {
    const o = buildCartesianOption(dateCols, dateRows, {
      kind: "bar", xAxis: "order_date", yMetrics: ["rev"], xAxisUnit: "month", xAxisType: "date",
    }) as Described;
    // The string the axis itself puts on the card, taken from the axis rather than restated —
    // so the two cannot drift apart without this failing.
    expect(o.xAxis.axisLabel?.formatter?.("2026-01-01")).toBe("January 2026");
    expect(o.aria.label.description).toContain("January 2026 10");
    expect(o.aria.label.description).toContain("February 2026 20");
    expect(o.aria.label.description).not.toContain("2026-01-01");
  });

  it("names the dimension the way its axis title does", () => {
    const o = buildCartesianOption(dateCols, dateRows, {
      kind: "bar", xAxis: "order_date", yMetrics: ["rev"], xAxisUnit: "month", xAxisType: "date",
      columnSettings: { order_date: { column_title: "Month" } },
    }) as Described;
    expect(o.xAxis.name).toBe("Month");
    expect(o.aria.label.description).toContain("by Month.");
    expect(o.aria.label.description).not.toContain("by order_date");
    // The rename is a title, not a date style, so it must NOT outrank the period name — the
    // priority `drawLabel` documents. Pinned here because this fixture sets both at once.
    expect(o.aria.label.description).toContain("January 2026 10");
  });

  it("names a breakout's bands the way the legend does", () => {
    // The legend and the aria's metric are ONE string — `first.name`, the series' display name —
    // so a band named for the raw bucket is heard as one too. Both ends pinned.
    const o = buildCartesianOption(
      [{ name: "region", type: "varchar" }, { name: "order_date", type: "date" }, { name: "rev", type: "double" }],
      [["EU", "2026-01-01", 10], ["EU", "2026-02-01", 4]],
      { kind: "bar", xAxis: "region", yMetrics: ["rev"], series: "order_date", seriesUnit: "month", seriesType: "date" },
    ) as Described;
    expect(o.series.map((s) => s.name)).toEqual(["January 2026", "February 2026"]);
    // …and the raw value still rides along, because that is what a drill filters by.
    expect(o.series.map((s) => s.__raw)).toEqual(["2026-01-01", "2026-02-01"]);
    expect(o.aria.label.description).toContain("of January 2026 by region");
  });

  it("names the period the bucket covers, on the waterfall", () => {
    const o = buildWaterfallOption(
      [{ name: "order_date", type: "date" }, { name: "delta", type: "double" }],
      [["2026-01-01", 100], ["2026-02-01", -30]],
      { xAxis: "order_date", metric: "delta", xAxisUnit: "month", showTotal: false },
    ) as Described;
    expect(o.xAxis.axisLabel?.formatter?.("2026-01-01")).toBe("January 2026");
    expect(o.aria.label.description).toContain("January 2026");
    expect(o.aria.label.description).toContain("February 2026");
    expect(o.aria.label.description).not.toContain("2026-01-01");
  });

  // THE BOX PLOT IS NOT AFFECTED, and this is the check that keeps it that way rather than an
  // assumption: it takes no `xAxisUnit`, has no `drawLabel`, and its category axis carries no
  // `axisLabel.formatter` — so the group key IS the drawn label, and feeding the aria the raw
  // categories is already correct. The day a formatter is added there, this fails.
  it("box plot: the drawn label is the group key, so its description already matches", () => {
    const o = buildBoxplotOption(
      [{ name: "order_date", type: "date" }, { name: "v", type: "double" }],
      [["2026-01-01", 1], ["2026-01-01", 3], ["2026-02-01", 10]],
      { category: "order_date", value: "v" },
    ) as Described;
    expect(o.xAxis.axisLabel?.formatter).toBeUndefined();
    for (const c of o.xAxis.data ?? []) expect(o.aria.label.description).toContain(String(c));
  });
});

describe("the aria component is registered", () => {
  // Without it ECharts silently ignores every `aria` option: no description, no decals.
  // That was the state of the app until this audit.
  it("registers without throwing and is idempotent", () => {
    expect(() => { registerEChartsModules(); registerEChartsModules(); }).not.toThrow();
  });
});
