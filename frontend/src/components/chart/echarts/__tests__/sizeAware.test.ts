import { describe, it, expect } from "vitest";
import type { EChartsCoreOption } from "echarts/core";
import type { EChartsSize } from "../EChartsRenderer";
import { buildCartesianOption } from "../options/cartesian";
import { buildGaugeOption } from "../options/gauge";
import { buildSankeyOption } from "../options/sankey";

/**
 * These three builders grew a fourth, optional `_size` parameter so the size-dependent label work —
 * x-label rotation, gauge graduations, sankey node labels; see each builder's own comment — has a
 * seam to attach to. Gauge and sankey still do not read it, and this file is what says so: build
 * each chart twice, once with a size and once without, and the two options must be
 * indistinguishable. Cartesian's seam is live as of PR 02, so its case is inverted below rather
 * than deleted — the point is unchanged, that a size changes exactly what it is supposed to and
 * nothing else.
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
   * two builds must differ, and differ ONLY in the x-axis label decision. The narrow assertion —
   * which rotation, which `interval` — is `cartesianXLabels.test.ts`; what this one adds is that
   * the size did not leak anywhere else. The obvious way to make a builder size-aware is
   * `formatter: (v) => truncate(v, widthFromSize)`, and this file's fingerprint probes closures
   * precisely so that a formatter quietly becoming size-dependent shows up as a diff.
   *
   * Two labels across 300px stay on the ladder's upright rung, so `grid.bottom` is the same in
   * both builds and the equality below is exact rather than approximate.
   */
  it("cartesian reads the size, and reads it ONLY for the x-axis labels", () => {
    const cfg = { kind: "bar" as const, xAxis: "k", yMetrics: ["v"] };
    const sized = fingerprint(buildCartesianOption(cols, rows, cfg, SMALL)) as Record<string, never>;
    const unsized = fingerprint(buildCartesianOption(cols, rows, cfg)) as Record<string, never>;
    // tooltip, category-axis label, value-axis label — pinned exactly, for the reason
    // `expectSizeIsUnread` gives.
    expect(functionsIn(sized)).toBe(3);

    const axisLabelOf = (fp: Record<string, never>) =>
      (fp as unknown as { xAxis: { axisLabel: unknown } }).xAxis.axisLabel;
    expect(axisLabelOf(sized)).not.toStrictEqual(axisLabelOf(unsized));

    delete (sized as unknown as { xAxis: { axisLabel?: unknown } }).xAxis.axisLabel;
    delete (unsized as unknown as { xAxis: { axisLabel?: unknown } }).xAxis.axisLabel;
    expect(sized).toStrictEqual(unsized);
  });

  it("gauge builds identically with and without a size", () => {
    // tooltip, arc axis label, centre detail.
    expectSizeIsUnread((size) => buildGaugeOption(cols, rows, { metric: "v" }, size), 3);
  });

  it("sankey builds identically with and without a size", () => {
    const scols = [{ name: "s", type: "varchar" }, { name: "t", type: "varchar" }, { name: "v", type: "double" }];
    const srows: unknown[][] = [["A", "X", 10], ["B", "X", 3]];
    const cfg = { source: "s", target: "t", metric: "v" };
    // tooltip, edge label.
    expectSizeIsUnread((size) => buildSankeyOption(scols, srows, cfg, size), 2);
  });
});
