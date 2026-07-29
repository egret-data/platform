import { describe, it, expect } from "vitest";
import { buildGaugeOption } from "../options/gauge";

const cols = [{ name: "rev", type: "double" }];

describe("buildGaugeOption", () => {
  // Task 1 shim: min/max are no longer read from cfg (segments replace them; task 3 derives
  // the scale from cfg.segments). Until then the scale is always the same auto-range.
  it("maps the metric value onto the gauge's auto-ranged scale", () => {
    const opt = buildGaugeOption(cols, [[60]], { metric: "rev" }) as any;
    expect(opt.series[0].type).toBe("gauge");
    expect(opt.series[0].min).toBe(0);
    expect(opt.series[0].max).toBe(120);
    expect(opt.series[0].data[0].value).toBe(60);
  });
  it("defaults max above the value when unset", () => {
    const opt = buildGaugeOption(cols, [[50]], { metric: "rev" }) as any;
    expect(opt.series[0].max).toBeGreaterThan(50);
  });
  it("formats the centre detail via column settings", () => {
    const opt = buildGaugeOption(cols, [[1000]], { metric: "rev", metricTypes: { rev: "double" }, columnSettings: { rev: { prefix: "$", decimals: 0 } } }) as any;
    expect(opt.series[0].detail.formatter(1000)).toBe("$1,000");
  });
  it("falls back to a zero value when there is no metric/row", () => {
    expect((buildGaugeOption(cols, [], {}) as any).series[0].data[0].value).toBe(0);
  });
  it("ignores a min and max left over from before segments existed", () => {
    // Stored settings are not whitelisted server-side, so these keys can still arrive at
    // runtime on an old chart even though GaugeBuildConfig no longer declares them.
    const opt = buildGaugeOption(cols, [[60]], { metric: "rev", min: 999, max: 5000 } as any) as any;
    expect(opt.series[0].min).toBe(0);
    expect(opt.series[0].max).toBe(120);
  });
});
