import { describe, it, expect } from "vitest";
import type { EChartsCoreOption } from "echarts/core";
import type { EChartsSize } from "../EChartsRenderer";
import { buildBoxplotOption } from "../options/boxplot";
import { buildCartesianOption } from "../options/cartesian";
import { buildGaugeOption } from "../options/gauge";
import { buildSankeyOption } from "../options/sankey";
import { buildWaterfallOption } from "../options/waterfall";

/**
 * These five builders grew a fourth, optional size parameter so the size-dependent label work —
 * x-label rotation, gauge graduations, sankey node labels; see each builder's own comment — has a
 * seam to attach to. Only the GAUGE still does not read it, and keeps the `_size` name to say so;
 * cartesian, the box plot, the waterfall and the sankey each call it `size` and read it. The gauge
 * is what this file is now written for: build the chart twice, once with a size and once without,
 * and the two options must be indistinguishable. The four live seams have their cases inverted
 * below rather than deleted — the point is unchanged, that a size changes exactly what it is
 * supposed to and nothing else.
 *
 * Naively that is `expect(build(cols, rows, cfg, SMALL)).toEqual(build(cols, rows, cfg))`, and that
 * does not work. Every one of these options carries FUNCTIONS — axis-label formatters, tooltip
 * formatters — and `toEqual` compares functions by reference. Two builder calls close over two
 * distinct closures, so the naive assertion failed all three builders before a line of production
 * code had changed. A test that red for that reason proves nothing about the size parameter.
 *
 * So both options are reduced to a `fingerprint` first, in which each function is replaced by
 * something comparable: its SOURCE, plus what it RETURNS for a handful of probe arguments. Source
 * alone would be too weak in exactly the direction the follow-up PRs push — the obvious way to make
 * a builder size-aware is `formatter: (v) => truncate(v, widthFromSize)`, whose source text is
 * identical whatever the captured width is. Probing the closure is what notices that.
 */

const SMALL: EChartsSize = { width: 300, height: 200 };

// A label deliberately LONGER than any axis here can draw. The size-dependent change these probes
// exist to catch is truncation, and truncation leaves a short label untouched — a probe set of
// short strings goes green against a formatter that truncates at 25 chars when sized and 40 when
// not. That is not hypothetical: this file shipped with `"Widget"` as its longest probe, and
// exactly that mutant passed it (re-verified both ways when the long probe was added).
const LONG_LABEL = "Enterprise Widget Assembly, North-Western Distribution Region";

/**
 * Arguments to call every function-valued option field with. ECharts hands formatters several
 * different shapes — a bare value on an axis label, a params object on an item tooltip, an array of
 * them on an axis tooltip — and one object carrying every field this directory's formatters read
 * (`base.ts`, and the tooltip/axis/label formatters in the three files under test) covers all of
 * them. `LONG_LABEL` goes in the name fields as well as standing alone, so tooltip and node-label
 * formatters come under the same pressure as axis labels. A formatter that throws on a shape it
 * never sees in production is fine: `probe` records the throw, and a throw is as good a fingerprint
 * as a return value so long as it is the same one.
 */
const PROBE_PARAMS = {
  name: LONG_LABEL, axisValueLabel: LONG_LABEL, seriesName: "v", value: 1234.5,
  marker: "<i></i>", dataType: "edge", percent: 12.5, dataIndex: 0,
  data: { name: LONG_LABEL, value: 1234.5 },
};
const PROBES: unknown[] = ["Widget", LONG_LABEL, 1234.5, PROBE_PARAMS, [PROBE_PARAMS]];

function probe(fn: (arg: never) => unknown): string[] {
  return PROBES.map((arg) => {
    try {
      return `= ${String(fn(arg as never))}`;
    } catch (e) {
      return `! ${e instanceof Error ? e.message : String(e)}`;
    }
  });
}

/** A deep-comparable stand-in for an option: same shape, functions swapped for source + probes. */
function fingerprint(v: unknown): unknown {
  if (typeof v === "function") return { __fn: String(v), __probes: probe(v as (a: never) => unknown) };
  // A Date would otherwise fall into the object branch and fingerprint as `{}`, which would make
  // every temporal category compare equal to every other one.
  if (v instanceof Date) return v.toISOString();
  if (Array.isArray(v)) return v.map(fingerprint);
  if (v !== null && typeof v === "object") {
    return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, fingerprint(x)]));
  }
  return v;
}

/** How many functions a fingerprint captured. */
function functionsIn(fp: unknown): number {
  return JSON.stringify(fp)?.match(/"__fn":/g)?.length ?? 0;
}

/**
 * @param expectedFunctions how many formatters this builder emits. Pinned EXACTLY rather than
 * merely non-zero: cartesian emits three, and if the axis-label formatter — the very one the size
 * seam exists for — collapsed to a plain string while the tooltip formatter stayed, a `> 0` guard
 * would still be satisfied and the seam would go unprobed. A deliberate change here is one number.
 */
function expectSizeIsUnread(
  build: (size?: EChartsSize) => EChartsCoreOption,
  expectedFunctions: number,
): void {
  // Both builds happen before either is fingerprinted: fingerprinting CALLS the formatters, and a
  // probe that touched shared state would otherwise land between the two builds and be blamed on
  // the size parameter.
  const sized = build(SMALL);
  const unsized = build();
  const fp = fingerprint(sized);
  expect(functionsIn(fp)).toBe(expectedFunctions);
  // toStrictEqual, not toEqual: a fingerprint is plain data by construction, so there is no false
  // failure to fear, and it tells `{ rotate: undefined }` apart from `{}` — exactly the shape a
  // size-conditional like `rotate: size ? 45 : undefined` produces.
  expect(fp).toStrictEqual(fingerprint(unsized));
}

const cols = [{ name: "k", type: "varchar" }, { name: "v", type: "double" }];
const rows: unknown[][] = [["Widget", 10], ["Gadget", 20]];

describe("builders accept a container size", () => {
  /**
   * Cartesian is the one that now READS it (`fitXLabels`, PR 02), so its entry here inverts: the
   * two builds must differ, and differ only where the fit is entitled to make them differ. The
   * narrow assertion — which rotation, which `interval` — is `cartesianXLabels.test.ts`; what this
   * one adds is that the size did not leak anywhere else.
   *
   * ONLY THE THREE KEYS THE FIT WRITES are exempted, not the whole `axisLabel` subtree. Exempting
   * the subtree would take `xAxis.axisLabel.formatter` — one of the three probed closures — out of
   * the comparison, and a size-dependent formatter (`(v) => truncate(v, widthFromSize)`) is
   * precisely the leak the fingerprint exists to catch, per this file's header. It would have been
   * a net loss of coverage dressed as a gain.
   *
   * Two labels across 300px stay on the ladder's upright rung, so `grid.bottom` is the same in
   * both builds and the equality below is exact rather than approximate.
   */
  it("cartesian reads the size, and reads it ONLY for the x-axis label decision", () => {
    const cfg = { kind: "bar" as const, xAxis: "k", yMetrics: ["v"] };
    const sized = fingerprint(buildCartesianOption(cols, rows, cfg, SMALL)) as Record<string, never>;
    const unsized = fingerprint(buildCartesianOption(cols, rows, cfg)) as Record<string, never>;
    // tooltip, category-axis label, value-axis label — pinned exactly, for the reason
    // `expectSizeIsUnread` gives.
    expect(functionsIn(sized)).toBe(3);

    const axisLabelOf = (fp: Record<string, never>) =>
      (fp as unknown as { xAxis: { axisLabel: Record<string, unknown> } }).xAxis.axisLabel;
    expect(axisLabelOf(sized)).not.toStrictEqual(axisLabelOf(unsized));

    for (const key of ["interval", "rotate", "show"]) {
      delete axisLabelOf(sized)[key];
      delete axisLabelOf(unsized)[key];
    }
    // `formatter` is still in here, and so is every other field of every other axis.
    expect(sized).toStrictEqual(unsized);
  });

  /**
   * The box plot and the waterfall read the size for exactly the same one decision cartesian does,
   * and are checked the same inverted way: the two builds must differ, and differ ONLY in the three
   * `axisLabel` keys the fit writes. Their whole reason for being here is the leak this file's
   * header describes — a size that reaches a formatter, a grid inset, a series, anything other than
   * the x-axis label decision it was threaded in for.
   *
   * `grid.bottom` is deliberately left IN the comparison rather than exempted alongside the three
   * keys: no builder reserves room below the axis for a rotated label any more (`containLabel`
   * does it — see `cartesianXLabels.test.ts`), so the sized and unsized grids agree exactly. A fit
   * that started padding the grid again would fail here as well, which is the point of not
   * exempting it.
   */
  it.each([
    ["box plot", (size?: EChartsSize) => buildBoxplotOption(
      cols, [["Widget", 10], ["Widget", 20], ["Gadget", 30], ["Gadget", 40]], { category: "k", value: "v" }, size,
    // Two: the tooltip and the value-axis label. The box plot's CATEGORY axis has no formatter of
    // its own — it draws the raw group key — where the waterfall's four include one.
    ), 2],
    ["waterfall", (size?: EChartsSize) => buildWaterfallOption(cols, rows, { xAxis: "k", metric: "v" }, size), 4],
  ] as const)("%s reads the size, and reads it ONLY for the x-axis label decision", (_what, build, fns) => {
    const sized = fingerprint(build(SMALL)) as Record<string, never>;
    const unsized = fingerprint(build()) as Record<string, never>;
    // Pinned exactly, for the reason `expectSizeIsUnread` gives: a formatter that collapsed to a
    // plain string would otherwise take itself out of the comparison unnoticed.
    expect(functionsIn(sized)).toBe(fns);

    const axisLabelOf = (fp: Record<string, never>) =>
      (fp as unknown as { xAxis: { axisLabel: Record<string, unknown> } }).xAxis.axisLabel;
    expect(axisLabelOf(sized)).not.toStrictEqual(axisLabelOf(unsized));

    for (const key of ["interval", "rotate", "show"]) {
      delete axisLabelOf(sized)[key];
      delete axisLabelOf(unsized)[key];
    }
    expect(sized).toStrictEqual(unsized);
  });

  it("gauge builds identically with and without a size", () => {
    // tooltip, arc axis label, centre detail.
    expectSizeIsUnread((size) => buildGaugeOption(cols, rows, { metric: "v" }, size), 3);
  });

  /**
   * The sankey reads it too now (PR 05), for one decision and one option key: how much of the width
   * the LAST column's labels get, which is `series[0].right`. Inverted the same way the four above
   * are — the two builds must differ, and differ ONLY there.
   *
   * The `right` gutter is the whole seam by design: on this fixture the single last-column node is
   * `"X"`, whose label is far inside the quarter-width cap, so nothing is truncated and no label
   * formatter appears. That is what makes the exact-function-count assertion below still worth
   * having: it stays at two, and a fit that started emitting a truncating formatter on a chart with
   * nothing to truncate would fail here rather than pass quietly.
   */
  it("sankey reads the size, and reads it ONLY for the node-label gutter", () => {
    const scols = [{ name: "s", type: "varchar" }, { name: "t", type: "varchar" }, { name: "v", type: "double" }];
    const srows: unknown[][] = [["A", "X", 10], ["B", "X", 3]];
    const cfg = { source: "s", target: "t", metric: "v" };
    const sized = fingerprint(buildSankeyOption(scols, srows, cfg, SMALL)) as Record<string, never>;
    const unsized = fingerprint(buildSankeyOption(scols, srows, cfg)) as Record<string, never>;
    // tooltip, edge label — pinned exactly, for the reason `expectSizeIsUnread` gives.
    expect(functionsIn(sized)).toBe(2);

    const seriesOf = (fp: Record<string, never>) =>
      (fp as unknown as { series: Record<string, unknown>[] }).series[0];
    expect(seriesOf(sized).right).not.toStrictEqual(seriesOf(unsized).right);

    delete seriesOf(sized).right;
    delete seriesOf(unsized).right;
    expect(sized).toStrictEqual(unsized);
  });
});
