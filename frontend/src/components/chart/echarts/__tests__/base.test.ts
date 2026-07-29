import { describe, it, expect } from "vitest";
import { baseOption, itemTooltip, axisTooltip, bottomLegend, baseTextStyle } from "../base";
import { ARIA_A11Y, SERIES_PALETTE } from "../palette";

describe("baseOption", () => {
  it("carries the accessibility config and the shared palette", () => {
    const o = baseOption();
    expect(o.aria).toBe(ARIA_A11Y);
    expect(o.color).toBe(SERIES_PALETTE);
    expect(o.textStyle).toEqual(baseTextStyle);
  });
});

describe("itemTooltip", () => {
  const fmt = (v: unknown) => `$${v}`;

  it("formats name and value through the chart's formatter", () => {
    expect(itemTooltip(fmt).formatter({ name: "North", value: 10 })).toBe("North: $10");
  });

  it("adds the share when asked", () => {
    expect(itemTooltip(fmt, { percent: true }).formatter({ name: "North", value: 10, percent: 25 }))
      .toBe("North: $10 (25%)");
  });

  it("shows the bare name when there is no value", () => {
    expect(itemTooltip(fmt).formatter({ name: "North" })).toBe("North");
    expect(itemTooltip(fmt).formatter({ name: "North", value: NaN })).toBe("North");
  });

  it("uses the shared text style", () => {
    expect(itemTooltip(fmt).textStyle).toEqual(baseTextStyle);
  });
});

describe("axisTooltip", () => {
  it("uses the shadow pointer and the shared text style", () => {
    const t = axisTooltip();
    expect(t.trigger).toBe("axis");
    expect(t.axisPointer).toEqual({ type: "shadow" });
    expect(t.textStyle).toEqual(baseTextStyle);
  });
});

describe("bottomLegend", () => {
  it("is a scrollable bottom legend when shown, and nothing when not", () => {
    expect(bottomLegend(true)).toEqual({ bottom: 0, type: "scroll", textStyle: baseTextStyle });
    expect(bottomLegend(false)).toBeUndefined();
    expect(bottomLegend(undefined)).toBeUndefined();
  });

  // A caller (pie's nested rings) needs the legend restricted to a specific set of names
  // rather than ECharts' default of every series' data — otherwise a chart with several
  // pie series lists every ring's slices in one flat legend.
  it("restricts entries to an explicit data list when given one", () => {
    expect(bottomLegend(true, ["North", "South"])).toEqual({
      bottom: 0, type: "scroll", textStyle: baseTextStyle, data: ["North", "South"],
    });
  });
  it("omits the data key entirely when not given one, unchanged from before", () => {
    expect(bottomLegend(true)).not.toHaveProperty("data");
  });
});
