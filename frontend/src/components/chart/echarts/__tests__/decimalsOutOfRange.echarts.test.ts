import { describe, it, expect } from "vitest";
import * as echarts from "echarts/core";
import { registerEChartsModules } from "../register";
import { buildCartesianOption, type CartesianBuildConfig } from "../options/cartesian";
import { formatValue } from "../../format";

/**
 * A `decimals` OUT OF INTL'S RANGE TAKES THE WHOLE CHART DOWN — it is a crash, not a wrong label.
 *
 * WHAT WAS MEASURED, because the first diagnosis of this was wrong and worth recording:
 *
 *   decimals   Intl.NumberFormat(min = max = decimals).format(1234.5678)
 *   2.5        "1,234.57"   — Intl TRUNCATES a fractional count. Harmless; no defect here.
 *   0          "1,235"      — a legal setting, not "unset".
 *   100        "1,234.5678000…" — legal; Intl's ceiling is 100, not 10.
 *   101        RangeError: minimumFractionDigits value is out of range.
 *   -1         RangeError: minimumFractionDigits value is out of range.
 *
 * So the fatal values are the OUT-OF-RANGE ones, and `FormatSection`'s box could produce them:
 * `min={0}` on an `<input type="number">` only sets a `validity.rangeUnderflow` flag at form
 * submission — PR #398 established in a real Chrome probe that it blocks no character — and there
 * was no `max` at all. Typing `-1` writes `-1` straight into `column_settings`.
 *
 * AND IT THROWS DURING THE BUILD, not inside a lazy ECharts formatter. The stack, taken today:
 *
 *   at formatNumber (format.ts:56)
 *   at formatValue (format.ts:139)
 *   at cartesian.ts (the value axis' own formatter)
 *   at widestTickLabel (axisLabelFit.ts:241)
 *   at estimatePlotWidth (axisLabelFit.ts:321)
 *   at baseOption (cartesian.ts)
 *   at buildCartesianOption
 *
 * `baseOption` formats the widest tick to measure the axis gutter, so the throw happens before any
 * option exists. In the app that lands in React's render pass and `ChartErrorBoundary` swallows it
 * into "Couldn't render this chart: minimumFractionDigits value is out of range." — the card loses
 * its chart entirely, and a SAVED chart keeps losing it on every load.
 *
 * THE FIX IS IN TWO PLACES ON PURPOSE. `FormatSection` clamps what a user can store (matching the
 * pie's Decimal places, which has had `min/max/step` AND a clamping handler since it was written).
 * `formatNumber` clamps what it will hand to Intl, because a chart saved before that control
 * existed — or written by any other writer of `column_settings` — is otherwise un-renderable for
 * good, and no panel fix can reach it.
 */

const COLS = [{ name: "c", type: "varchar" }, { name: "rev", type: "double" }];
const ROWS: unknown[][] = [["a", 1234.5678], ["b", 2345.6789]];
const SIZE = { width: 640, height: 400 };

const build = (cfg: CartesianBuildConfig) => buildCartesianOption(COLS, ROWS, cfg, SIZE);

function renderSVG(option: unknown): string {
  registerEChartsModules();
  const chart = echarts.init(null, null, { renderer: "svg", ssr: true, ...SIZE });
  chart.setOption({ ...(option as object), animation: false });
  const svg = chart.renderToSVGString();
  chart.dispose();
  return svg;
}

describe("formatValue survives a decimals Intl would reject", () => {
  // Each of `formatNumber`'s three branches hands `decimals` to a different API, and all three
  // have the same 0..100 range: `Intl.NumberFormat` twice (plain and compact) and
  // `Number#toExponential` for scientific. One clamp covers all three; a per-branch guard would
  // have to be remembered three times.
  it.each([-1, 101, -0.4, Number.NaN])("does not throw on decimals %s", (decimals) => {
    expect(() => formatValue(1234.5678, "double", { decimals })).not.toThrow();
    expect(() => formatValue(1234.5678, "double", { decimals, compact: true })).not.toThrow();
    expect(() => formatValue(1234.5678, "double", { decimals, number_style: "scientific" })).not.toThrow();
  });

  it("reads a negative decimals as 0 — the nearest thing the user could have meant", () => {
    expect(formatValue(1234.5678, "double", { decimals: -1 })).toBe("1,235");
  });

  it("caps an over-large decimals at Intl's own ceiling rather than refusing to render", () => {
    expect(formatValue(1.5, "double", { decimals: 101 })).toBe(`1.5${"0".repeat(99)}`);
  });

  it("still honours every decimals Intl accepts, untouched", () => {
    expect(formatValue(1234.5678, "double", { decimals: 0 })).toBe("1,235");
    expect(formatValue(1234.5678, "double", { decimals: 2 })).toBe("1,234.57");
    // Intl truncates a fractional count itself; the clamp must not start rounding it up instead.
    expect(formatValue(1234.5678, "double", { decimals: 2.5 })).toBe("1,234.57");
    expect(formatValue(1234.5678, "double", { decimals: 20 })).toBe("1,234.56780000000000000000");
  });
});

describe("a chart whose column carries an out-of-range decimals", () => {
  it("builds at all — this used to throw out of baseOption's axis measurement", () => {
    expect(() =>
      build({
        kind: "bar", xAxis: "c", yMetrics: ["rev"], metricTypes: { rev: "double" },
        columnSettings: { rev: { decimals: -1 } }, showLabels: true,
      }),
    ).not.toThrow();
  });

  it("draws its value labels, rather than the error boundary's message", () => {
    const opt = build({
      kind: "bar", xAxis: "c", yMetrics: ["rev"], metricTypes: { rev: "double" },
      columnSettings: { rev: { decimals: -1 } }, showLabels: true,
    });
    const svg = renderSVG(opt);
    expect(svg).toContain("1,235");
    expect(svg).toContain("2,346");
  });
});
