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

  /**
   * THE BOX PLOT STILL DESCRIBES WHAT IT DRAWS — the same claim, now checked against a formatter
   * instead of against its absence.
   *
   * This test used to assert `axisLabel.formatter` was UNDEFINED and said "the day a formatter is
   * added there, this fails". It did, on the PR that gave the category axis its column format
   * (`options/boxplot.ts`'s `catLabel`), and failing was the correct behaviour: the assertion was a
   * proxy for "the description matches the ticks", and the proxy stopped standing for the claim the
   * moment a formatter existed. What replaces it is the claim itself, asserted directly and in both
   * states — the ticks and the description are read from the same option and compared to each
   * other, so no future formatter can move one without the other.
   *
   * The box plot still takes no `xAxisUnit` and names no period, which is why the UNFORMATTED case
   * below draws the raw group key and is not a regression from what this used to pin.
   */
  it("box plot: the description says whatever the ticks say, formatted or not", () => {
    const build = (columnSettings?: Record<string, { date_style: string }>) => buildBoxplotOption(
      [{ name: "order_date", type: "date" }, { name: "v", type: "double" }],
      [["2026-01-01", 1], ["2026-01-01", 3], ["2026-02-01", 10]],
      { category: "order_date", value: "v", columnSettings },
    ) as Described;

    // No stored format: the tick is the raw group key, and so is the description.
    const plain = build();
    const plainTicks = (plain.xAxis.data ?? []).map((c) => plain.xAxis.axisLabel?.formatter?.(String(c)));
    expect(plainTicks).toEqual(["2026-01-01", "2026-02-01"]);
    for (const t of plainTicks) expect(plain.aria.label.description).toContain(String(t));

    // A date style: both move together, and the raw key is heard nowhere.
    const styled = build({ order_date: { date_style: "MMMM YYYY" } });
    const styledTicks = (styled.xAxis.data ?? []).map((c) => styled.xAxis.axisLabel?.formatter?.(String(c)));
    expect(styledTicks).toEqual(["January 2026", "February 2026"]);
    for (const t of styledTicks) expect(styled.aria.label.description).toContain(String(t));
    expect(styled.aria.label.description).not.toContain("2026-01-01");
  });
});

/**
 * THE PIE DESCRIBES THE SLICES IT IS DRAWING, and until this commit it did not.
 *
 * The description was built from `effectiveRings[0]` while the arcs came from `pieRootSlices(...)`,
 * which filters `toggledOffSlices` — the EPHEMERAL legend click, the one thing `buildPieRings` has
 * deliberately not already applied (the saved `hiddenSlices` curation is applied before a ring is
 * aggregated, so that every total and threshold re-bases; the click is applied to the finished
 * rings, so they do not). With "US" clicked off, ring 0 drew ["APAC", "EU"] and a screen reader
 * heard "3 values: US 30, APAC 20, EU 10". Both the count and the enumeration were false.
 *
 * The fixture's values are all distinct and none of the names is a substring of another, so no
 * assertion below can pass on a coincidence.
 */
describe("the pie describes the slices it is drawing", () => {
  const cols = [{ name: "region", type: "varchar" }, { name: "m", type: "double" }];
  const rows = [["US", 30], ["APAC", 20], ["EU", 10]];
  const cfg = { dimensions: ["region"], metric: "m", sliceThreshold: 0 };
  const descriptionOf = (c: Record<string, unknown>) =>
    (buildPieOption(cols, rows, { ...cfg, ...c }) as { aria: { label: { description: string } } })
      .aria.label.description;

  it("counts and enumerates only what ring 0 draws once a legend entry is toggled off", () => {
    const d = descriptionOf({ toggledOffSlices: ["US"] });
    expect(d).toContain("2 values:");
    expect(d).not.toContain("3 values:");
    expect(d).toContain("APAC 20");
    expect(d).toContain("EU 10");
    expect(d).not.toContain("US 30");
  });

  it("is unchanged when nothing is toggled off", () => {
    expect(descriptionOf({})).toBe(descriptionOf({ toggledOffSlices: [] }));
    expect(descriptionOf({})).toContain("3 values: US 30, APAC 20, EU 10.");
  });

  // The divergence note is appended AFTER the description body and is computed from the full
  // `effectiveRings`, not from what the legend leaves showing — the same rule the hole's total and
  // the legend percentages follow. Sourcing the body from `rootSlices` must not disturb that.
  it("still carries a ring's divergence note, and does not re-base it on the toggle", () => {
    const ringCols = [
      { name: "region", type: "varchar" }, { name: "category", type: "varchar" }, { name: "m", type: "double" },
    ];
    // The inner ring totals 40; the outer drops the -5 row by the zero/negative filter and totals 35.
    const ringRows = [["US", "Widgets", 30], ["EU", "Gadgets", 10], ["EU", "Doohickeys", -5]];
    const withNote = (c: Record<string, unknown>) =>
      (buildPieOption(ringCols, ringRows, {
        dimensions: ["region", "category"], metric: "m", sliceThreshold: 0, ...c,
      }) as { aria: { label: { description: string } } }).aria.label.description;
    expect(withNote({})).toContain("Note: the category ring totals");
    const off = withNote({ toggledOffSlices: ["US"] });
    expect(off).toContain("1 values:");
    // `EU 5`, not `EU 10`: ring 0 aggregates the region, and EU's -5 row is part of that sum even
    // though the OUTER ring's zero/negative filter refuses to draw it. That gap is the whole
    // subject of the note below, and the description reads the value the wedge is drawn at.
    expect(off).toContain("EU 5");
    expect(off).not.toContain("US 30");
    // Same note, same numbers — the note reads the rings, and a legend click does not touch them.
    expect(off).toContain(withNote({}).slice(withNote({}).indexOf("Note:")));
  });
});

/**
 * THE VALUE-AXIS FAMILY READS THE Y OF ITS OWN DATUM.
 *
 * Measured live during #411: a scatter's description came out `"15.69 NaN, 19.87 NaN, …"` — every
 * value unreadable. The aria builder zipped `drawnCategories` against `first.data[i]` and passed
 * the datum straight to `Number()`, and since #387 a datum on a value/time axis is a
 * `{value: [x, y], name}` item object, which `Number()` answers `NaN` to. `datumY` was introduced
 * in #397 to unwrap exactly that, for `widestOf`; the aria path never picked it up.
 *
 * THE SECOND HALF WAS NOT IN THE BRIEF and is the more dangerous of the two: `applyXScale` FILTERS
 * — a datum whose x will not parse is dropped from the series — so `first.data` is no longer
 * index-aligned with `drawnCategories` at all. That does not read as `NaN`; it reads as a plausible
 * sentence pairing the wrong number with the wrong label, which is worse.
 *
 * FIXTURE DISCIPLINE, the trap #397 recorded: every x and every y below is distinct and no y equals
 * any x, so the mutation that reads `v[0]` instead of `v[1]` cannot pass.
 */
describe("a value/time x-axis is described with the y it plots", () => {
  type Described = { aria: { label: { description: string } } };

  it("reads a scatter's y rather than NaN", () => {
    const o = buildCartesianOption(
      [{ name: "weight", type: "double" }, { name: "score", type: "double" }],
      [[100, 7], [200, 8], [300, 9]],
      { kind: "scatter", xAxis: "weight", yMetrics: ["score"] },
    ) as Described;
    const d = o.aria.label.description;
    expect(d).not.toContain("NaN");
    // Both halves of every pair named: `100 100` is what reading `v[0]` produces.
    expect(d).toContain("100 7");
    expect(d).toContain("200 8");
    expect(d).toContain("300 9");
    expect(d).not.toContain("100 100");
  });

  it("does not slide the pairings when a point's x cannot be placed", () => {
    // `Date.parse` REFUSES A REGION ZONE ID — `"… 00:00:00.000 Asia/Ho_Chi_Minh"` is `NaN`, while
    // the same string with `UTC` parses — and that is precisely how Trino renders a
    // `timestamp with time zone`. So this is a reachable production shape, not a contrived one, and
    // it is the only way a point reaches `applyXScale` and is dropped: a NULL x loses its whole row
    // further upstream, before `categories` is built, which keeps the two lists aligned by luck.
    //
    // The middle point is therefore not drawn and must not be described. What must NOT happen is
    // the survivors sliding up into its slot — which is what zipping `drawnCategories` against a
    // FILTERED `series.data` by index does, and it reads as a plausible sentence rather than as
    // `NaN`, so nothing else would have caught it.
    const o = buildCartesianOption(
      [{ name: "order_ts", type: "timestamp" }, { name: "rev", type: "double" }],
      [
        ["2026-01-01 00:00:00.000 UTC", 11],
        ["2026-02-01 00:00:00.000 Asia/Ho_Chi_Minh", 22],
        ["2026-03-01 00:00:00.000 UTC", 33],
      ],
      { kind: "line", xAxis: "order_ts", yMetrics: ["rev"], xScale: "timeseries" },
    ) as Described;
    const d = o.aria.label.description;
    expect(d).not.toContain("NaN");
    expect(d).toContain("2 values:");
    expect(d).toContain("2026-01-01 00:00:00.000 UTC 11");
    expect(d).toContain("2026-03-01 00:00:00.000 UTC 33");
    // The two shapes the old index-zip produced: the third point wearing the second's value, and
    // the dropped point named at all.
    expect(d).not.toContain("2026-03-01 00:00:00.000 UTC 22");
    expect(d).not.toContain("Asia/Ho_Chi_Minh");
  });

  it("names the period on a TIME axis too, not the raw bucket the datum carries", () => {
    // #400 decided that the description reads the period the axis draws rather than the truncated
    // bucket, and pinned it on the ordinal path. This is the other path, and it needs saying
    // separately: a value/time datum carries the RAW category (`ScaledPoint.name`, which exists so
    // a drill can filter by it), so reading the datum's name straight out would have quietly undone
    // #400 for every timeseries chart. A mutation replacing the `drawnByRaw` lookup with the raw
    // name survived the whole suite until this case existed.
    //
    // `2026-01-01` and `January 2026` share no substring, so neither direction passes by accident.
    const o = buildCartesianOption(
      [{ name: "order_date", type: "date" }, { name: "rev", type: "double" }],
      [["2026-01-01", 61], ["2026-02-01", 72]],
      {
        kind: "line", xAxis: "order_date", yMetrics: ["rev"],
        xScale: "timeseries", xAxisUnit: "month", xAxisType: "date",
      },
    ) as Described;
    const d = o.aria.label.description;
    expect(d).toContain("January 2026 61");
    expect(d).toContain("February 2026 72");
    expect(d).not.toContain("2026-01-01");
  });

  it("still describes an ordinal axis positionally", () => {
    // The other branch, which has no per-datum name to read and must keep zipping by index.
    const o = buildCartesianOption(
      [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }],
      [["North", 41], ["South", 52]],
      { kind: "bar", xAxis: "region", yMetrics: ["rev"] },
    ) as Described;
    expect(o.aria.label.description).toContain("2 values: North 41, South 52.");
  });
});

describe("the aria component is registered", () => {
  // Without it ECharts silently ignores every `aria` option: no description, no decals.
  // That was the state of the app until this audit.
  it("registers without throwing and is idempotent", () => {
    expect(() => { registerEChartsModules(); registerEChartsModules(); }).not.toThrow();
  });
});
