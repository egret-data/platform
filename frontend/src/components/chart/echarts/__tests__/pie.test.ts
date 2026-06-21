import { describe, it, expect } from "vitest";
import { buildPieOption } from "../options/pie";
import { SERIES_PALETTE } from "../palette";

const cols = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }];
const rows = [["EU", 10], ["US", 30]];

describe("buildPieOption", () => {
  it("maps rows to {name, value} slices", () => {
    const opt = buildPieOption(cols, rows, { dimension: "region", metric: "rev" }) as any;
    expect(opt.series[0].type).toBe("pie");
    expect(opt.series[0].data).toEqual([{ name: "EU", value: 10 }, { name: "US", value: 30 }]);
    expect(opt.series[0].radius).toEqual(["0%", "70%"]);
  });
  it("donut sets an inner radius", () => {
    const opt = buildPieOption(cols, rows, { dimension: "region", metric: "rev", donut: true }) as any;
    expect(opt.series[0].radius).toEqual(["45%", "70%"]);
  });
  it("empty when dimension or metric missing", () => {
    expect((buildPieOption(cols, rows, {}) as any).series[0].data).toEqual([]);
  });
});

// Behaviors ported from Metabase's pie unit specs (visualizations/echarts/pie/*),
// scoped to what egret's flat buildPieOption supports: palette, percent tooltip,
// legend + label toggles. (Metabase-only: "Other" grouping, negative handling,
// donut centre total, percent-visibility modes — tracked as fidelity gaps.)
describe("buildPieOption — Metabase-parity", () => {
  const cols = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }];
  const rows = [["EU", 10], ["US", 30]];
  const base = { dimension: "region", metric: "rev" };

  it("uses the categorical palette", () => {
    expect((buildPieOption(cols, rows, base) as any).color).toEqual(SERIES_PALETTE);
  });
  it("item tooltip carries a percent formatter", () => {
    const opt = buildPieOption(cols, rows, base) as any;
    expect(opt.tooltip.trigger).toBe("item");
    expect(opt.tooltip.formatter({ name: "EU", value: 10, percent: 25 })).toContain("25%");
  });
  it("showLegend toggles the legend", () => {
    expect((buildPieOption(cols, rows, { ...base, showLegend: true }) as any).legend).toMatchObject({ type: "scroll" });
    expect((buildPieOption(cols, rows, base) as any).legend).toBeUndefined();
  });
  it("showLabels toggles slice labels", () => {
    expect((buildPieOption(cols, rows, { ...base, showLabels: true }) as any).series[0].label.show).toBe(true);
    expect((buildPieOption(cols, rows, base) as any).series[0].label.show).toBe(false);
  });
});
