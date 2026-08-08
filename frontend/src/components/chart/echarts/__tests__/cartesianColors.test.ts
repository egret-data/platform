import { describe, it, expect } from "vitest";
import { buildCartesianOption } from "../options/cartesian";
import { colorsForKeys } from "../keyColors";
import { PREFERRED_COLORS } from "../keyColors";

/**
 * Which colour a cartesian series is drawn in.
 *
 * The rule is Metabase's: a series' colour is derived from a hash of its NAME, so a column keeps
 * the same colour on every card of a dashboard that plots it. It is NOT derived from the series'
 * position, and there is no special case for a chart that happens to have exactly one series.
 *
 * That last clause is why this file exists. The builder used to hand a lone series `#689735` and
 * two comments in it asserted that this was Metabase's own behaviour. Measured on identical data
 * on the "ZZ Chart Parity" dashboards, both sides: it is not. Metabase draws a single-metric bar,
 * row, line or area card in `#f9d45c` — which is exactly what "revenue" hashes to, and what
 * `colorsForKeys(["revenue"])` returns below. `#689735` is Metabase's `success` colour, handed out
 * by name to a series actually CALLED "profit" or "completed"; it was mistaken for a lone-series
 * rule and five egret charts rendered entirely in it.
 *
 * WHICH CASES CARRY THE BY-NAME GUARANTEE. A chart with ONE series cannot distinguish a by-name
 * rule from a by-position one — both hand out exactly one colour — so the single-metric cases below
 * pin WHICH colour (not the green, the hashed one) and nothing about the mechanism. The mechanism
 * is pinned by the multi-series cases, and only because their fixtures are unsorted; see `rows`.
 */

const cols = [
  { name: "month", type: "varchar" },
  { name: "category", type: "varchar" },
  { name: "revenue", type: "double" },
  { name: "orders", type: "bigint" },
];

/**
 * The four Sample-Database product categories. Two of them — "Gadget" and "Widget" — hash onto the
 * SAME palette slot, so this data also exercises `colorsForKeys`' collision walk rather than only
 * its happy path.
 *
 * THE EMITTED ORDER IS LOAD-BEARING AND MUST NOT BE ALPHABETICAL. `hashMapping` assigns in sorted
 * key order, so its returned Record is ALSO in sorted order — and a chart whose bands come out
 * sorted makes a by-name lookup and a by-position one produce the identical sequence. The two
 * breakout tests below were written against `Doohickey, Gadget, Gizmo, Widget` and both passed with
 * `colors[s.name]` mutated to `Object.values(colors)[colorIdx]`: the exact bug this file exists to
 * pin, surviving the tests named for it.
 *
 * IT USED TO COME FOR FREE FROM THE ROW ORDER, and no longer does: the builder now emits the bands
 * in ascending order of their raw value (`sortBands`), because first-seen order was the same card
 * drawing a different legend on two page loads. That fix would have re-blinded both tests silently
 * — sorted emission, sorted Record, identical sequences — so each states the non-alphabetical order
 * it needs through `seriesOrder`, which is the panel's own control and outranks the default sort.
 * The rows below stay unsorted anyway, so a regression to arrival order still shows up elsewhere.
 */
const rows: unknown[][] = [
  ["2026-01", "Widget", 10, 1],
  ["2026-01", "Doohickey", 20, 2],
  ["2026-01", "Gizmo", 30, 3],
  ["2026-01", "Gadget", 40, 4],
  ["2026-02", "Widget", 50, 5],
  ["2026-02", "Doohickey", 60, 6],
  ["2026-02", "Gizmo", 70, 7],
  ["2026-02", "Gadget", 80, 8],
];
/** Deliberately NOT sorted; see `rows` for why, and for why the tests using it must ask for it
 *  through `seriesOrder`. `colorsForKeys` sorts internally, so the EXPECTED colours are unaffected
 *  by this order, which is precisely what makes the comparison meaningful. */
const GROUPS = ["Widget", "Doohickey", "Gizmo", "Gadget"];

const baseCfg = { kind: "bar" as const, xAxis: "month", yMetrics: ["revenue"] };

type Cfg = Parameters<typeof buildCartesianOption>[2];
interface SeriesShape { name: string; itemStyle?: { color?: string }; lineStyle?: { color?: string } }

const buildCartesian = (cfg: Cfg) =>
  buildCartesianOption(cols, rows, cfg) as unknown as { series: SeriesShape[] };
const seriesOf = (opt: { series: SeriesShape[] }) => opt.series;
const colorOf = (s: SeriesShape) => s.itemStyle?.color ?? s.lineStyle?.color;
const seriesColorsOf = (opt: { series: SeriesShape[] }) => seriesOf(opt).map(colorOf);

describe("cartesian series colours", () => {
  it("does not give a lone series the summarize green", () => {
    // Metabase, measured on identical data, draws this card #f9d45c. The green was egret's.
    const opt = buildCartesian({ ...baseCfg, yMetrics: ["revenue"] });
    expect(seriesColorsOf(opt)).not.toContain(PREFERRED_COLORS.success);
  });

  it("gives a lone series the colour its NAME hashes to", () => {
    const opt = buildCartesian({ ...baseCfg, yMetrics: ["revenue"] });
    expect(seriesColorsOf(opt)).toEqual([colorsForKeys(["revenue"]).revenue]);
  });

  it("gives the same column the same colour in two different charts", () => {
    const a = buildCartesian({ ...baseCfg, yMetrics: ["revenue", "orders"] });
    const b = buildCartesian({ ...baseCfg, yMetrics: ["orders", "revenue"] });
    const byName = (opt: { series: SeriesShape[] }) =>
      Object.fromEntries(seriesOf(opt).map((s) => [s.name, colorOf(s)]));
    expect(byName(a).revenue).toBe(byName(b).revenue);
    expect(byName(a).orders).toBe(byName(b).orders);
  });

  it("colours a breakout by its group name", () => {
    const opt = buildCartesian({ ...baseCfg, series: "category", seriesOrder: GROUPS });
    expect(seriesOf(opt).map((s) => s.name)).toEqual(GROUPS); // the emission order this pins against
    expect(seriesColorsOf(opt)).toEqual(GROUPS.map((g) => colorsForKeys(GROUPS)[g]));
  });

  it("a series NAMED for a meaning still takes that meaning's colour", () => {
    // The rule the lone-series green was mistaken for. `profit` is one of Metabase's preferred
    // names, so it keeps the success green here — because of what it is called, not because it is
    // alone.
    const c = [{ name: "month", type: "varchar" }, { name: "profit", type: "double" }];
    const r: unknown[][] = [["2026-01", 10], ["2026-02", 20]];
    const opt = buildCartesianOption(c, r, { kind: "bar", xAxis: "month", yMetrics: ["profit"] }) as unknown as { series: SeriesShape[] };
    expect(seriesColorsOf(opt)).toEqual([PREFERRED_COLORS.success]);
  });

  it("keys a scatter's lone series by its y-metric, not by the empty group name", () => {
    // A metric-vs-metric scatter has no breakout, so its one group is named "". Hashing THAT would
    // give every un-broken-out scatter in the product the same colour whatever it plots — and it is
    // also the key `cartesianSeriesNames` does NOT offer the settings panel, so a pinned colour
    // could never match.
    const opt = buildCartesian({ kind: "scatter", xMetric: "orders", yMetrics: ["revenue"] });
    expect(seriesColorsOf(opt)).toEqual([colorsForKeys(["revenue"]).revenue]);
    expect(seriesColorsOf(opt)).not.toEqual([colorsForKeys([""])[""]]);
  });

  it("colours a scatter's breakout bands by their group names", () => {
    const opt = buildCartesian({
      kind: "scatter", xMetric: "orders", yMetrics: ["revenue"], series: "category", seriesOrder: GROUPS,
    });
    expect(seriesOf(opt).map((s) => s.name)).toEqual(GROUPS);
    expect(seriesColorsOf(opt)).toEqual(GROUPS.map((g) => colorsForKeys(GROUPS)[g]));
  });

  it("an explicit seriesColors entry consumes its colour rather than shadowing it", () => {
    // `colorsForKeys` folds the override in, so the series that WOULD have hashed onto the pinned
    // ink moves along instead of being drawn identically to the pinned one.
    const metrics = ["revenue", "orders"];
    // The ink `revenue` would have hashed to, pinned onto `orders` instead.
    const pinned = colorsForKeys(metrics).revenue;
    const opt = buildCartesian({ ...baseCfg, yMetrics: metrics, seriesColors: { orders: pinned } });
    expect(seriesColorsOf(opt)).toEqual(
      metrics.map((m) => colorsForKeys(metrics, { orders: pinned })[m]),
    );
    // …which is to say: `orders` gets it, and `revenue` moves along rather than being drawn the same.
    expect(seriesColorsOf(opt)[1]).toBe(pinned);
    expect(seriesColorsOf(opt)[0]).not.toBe(pinned);
  });
});

/**
 * THE RENAME'S HALF OF THE SAME KEY, which the colour above has had since it was written and the
 * LABEL did not.
 *
 * `scatterXY` keys its colours through `groupKey(g) = g === "" ? yMetric : g` — added precisely so an
 * un-broken-out scatter's one group, whose raw name is `""`, is looked up under the name the panel
 * actually stores against. Seven lines below it the series' `name` did its own lookup on the RAW `g`,
 * so `seriesColors` matched and `seriesLabels` could not: `SeriesListSection` writes the rename under
 * whatever `cartesianSeriesNames` handed it, and that function returns the Y-METRIC for a chart with
 * no breakout. The rename was therefore stored under `revenue` and read under `""` — typed, saved,
 * round-tripped, and never drawn.
 *
 * `settingsKey`'s own comment in `options/cartesian.ts` had already written down the fact that
 * settles this — "an un-broken-out scatter's one group is named `""` and is labelled with its
 * y-metric, which is the key both the builder and this panel use" — while the line it describes was
 * using the other key.
 *
 * THE BREAKOUT CASE IS PINNED TOO, and with FOUR bands renaming the SECOND. `groupKey` is the
 * identity on any non-empty group, so a fix that reached only the empty one would be no fix at all;
 * and a two-band fixture cannot see a name-keyed bug, because dropping one name and re-appending it
 * yields the identical list. Renaming a middle band means position and name disagree.
 */
describe("cartesian series renames", () => {
  it("renames a scatter's lone series under its y-metric, the key the panel stores", () => {
    const opt = buildCartesian({
      kind: "scatter", xMetric: "orders", yMetrics: ["revenue"],
      seriesLabels: { revenue: "Revenue (USD)" },
    });
    expect(seriesOf(opt).map((s) => s.name)).toEqual(["Revenue (USD)"]);
  });

  it("does not read a scatter's rename under the raw empty group name", () => {
    // The mirror of the assertion above: `""` is not a key the panel can write, so a config
    // carrying one must not be what makes the rename appear.
    const opt = buildCartesian({
      kind: "scatter", xMetric: "orders", yMetrics: ["revenue"],
      seriesLabels: { "": "Should not appear" },
    });
    expect(seriesOf(opt).map((s) => s.name)).toEqual(["revenue"]);
  });

  it("renames one of a scatter's breakout bands and leaves the other three alone", () => {
    const opt = buildCartesian({
      kind: "scatter", xMetric: "orders", yMetrics: ["revenue"], series: "category",
      seriesLabels: { Doohickey: "Doohickeys" },
    });
    // Ascending by the RAW band value, so the renamed one stays where `Doohickey` belongs rather
    // than moving to where `Doohickeys` would sort — the rename is a label, not a key.
    expect(seriesOf(opt).map((s) => s.name)).toEqual(["Doohickeys", "Gadget", "Gizmo", "Widget"]);
  });

  it("a renamed scatter band keeps the colour its RAW name hashes to", () => {
    // The two settings are keyed alike and must stay so: renaming a band must not move its ink.
    const plain = buildCartesian({ kind: "scatter", xMetric: "orders", yMetrics: ["revenue"], series: "category" });
    const renamed = buildCartesian({
      kind: "scatter", xMetric: "orders", yMetrics: ["revenue"], series: "category",
      seriesLabels: { Doohickey: "Doohickeys" },
    });
    expect(seriesColorsOf(renamed)).toEqual(seriesColorsOf(plain));
  });

  it("a renamed lone scatter series keeps the colour its Y-METRIC hashes to", () => {
    const renamed = buildCartesian({
      kind: "scatter", xMetric: "orders", yMetrics: ["revenue"],
      seriesLabels: { revenue: "Revenue (USD)" },
    });
    expect(seriesColorsOf(renamed)).toEqual([colorsForKeys(["revenue"]).revenue]);
  });
});
