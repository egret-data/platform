import { describe, it, expect } from "vitest";
import { buildGaugeOption } from "../options/gauge";

const cols = [{ name: "rev", type: "double" }];

describe("buildGaugeOption", () => {
  it("maps the metric value onto a gauge with the configured min/max", () => {
    const opt = buildGaugeOption(cols, [[60]], { metric: "rev", min: 0, max: 100 }) as any;
    expect(opt.series[0].type).toBe("gauge");
    expect(opt.series[0].min).toBe(0);
    expect(opt.series[0].max).toBe(100);
    expect(opt.series[0].data[0].value).toBe(60);
  });
  it("defaults max above the value when unset", () => {
    const opt = buildGaugeOption(cols, [[50]], { metric: "rev" }) as any;
    expect(opt.series[0].max).toBeGreaterThan(50);
  });
  it("formats the centre detail via column settings", () => {
    const opt = buildGaugeOption(cols, [[1000]], { metric: "rev", max: 2000, metricTypes: { rev: "double" }, columnSettings: { rev: { prefix: "$", decimals: 0 } } }) as any;
    expect(opt.series[0].detail.formatter(1000)).toBe("$1,000");
  });
  it("falls back to a zero value when there is no metric/row", () => {
    expect((buildGaugeOption(cols, [], {}) as any).series[0].data[0].value).toBe(0);
  });
});
