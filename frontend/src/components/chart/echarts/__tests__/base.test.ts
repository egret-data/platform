import { describe, it, expect } from "vitest";
import { baseOption, itemTooltip, axisTooltip, baseTextStyle } from "../base";
import { ARIA_A11Y, SERIES_PALETTE, CHART_FONT_FALLBACK } from "../palette";

describe("baseOption", () => {
  it("carries the accessibility config and the shared palette", () => {
    const o = baseOption();
    expect(o.aria).toBe(ARIA_A11Y);
    expect(o.color).toBe(SERIES_PALETTE);
    expect(o.textStyle).toEqual(baseTextStyle());
  });

  // Stated concretely, not just as "the same as baseTextStyle" — which would hold just as well if
  // neither named a font, which is the state this replaced (every chart drew in the browser's
  // default sans while the app around it is Inter).
  it("names the product's own typeface, at the option root", () => {
    expect(baseOption().textStyle.fontFamily).toBe(CHART_FONT_FALLBACK);
    expect(itemTooltip((v) => String(v)).textStyle.fontFamily).toBe(CHART_FONT_FALLBACK);
    expect(axisTooltip().textStyle.fontFamily).toBe(CHART_FONT_FALLBACK);
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
    expect(itemTooltip(fmt).textStyle).toEqual(baseTextStyle());
  });

  // Every pie slice states its own share on the datum (`data.slicePercent`) because the pie is a
  // `sunburst` series, and a sunburst supplies no `percent` at all — only `PieSeriesModel` adds
  // one. When a datum carries one it wins; charts that never set it (funnel, map, boxplot, sankey)
  // are unaffected — `percent` still comes straight from ECharts.
  it("prefers a per-datum slicePercent over ECharts' own percent, when the datum carries one", () => {
    expect(
      itemTooltip(fmt, { percent: true }).formatter({ name: "North", value: 10, percent: 4.17, data: { slicePercent: 50 } }),
    ).toBe("North: $10 (50%)");
  });
});

describe("axisTooltip", () => {
  it("uses the shadow pointer and the shared text style", () => {
    const t = axisTooltip();
    expect(t.trigger).toBe("axis");
    expect(t.axisPointer).toEqual({ type: "shadow" });
    expect(t.textStyle).toEqual(baseTextStyle());
  });
});

