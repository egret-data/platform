import { describe, it, expect } from "vitest";
import { baseOption, itemTooltip, axisTooltip, bottomLegend, sideLegend, baseTextStyle } from "../base";
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

  // Pie's outer rings (see pie.ts buildPieOption) stash a parent-relative percentage on the
  // datum itself (`data.parentPercent`) because ECharts' own per-series `percent` is always
  // share-of-that-series (share of the ring), with no per-datum override. When a datum carries
  // one, it wins over ECharts' own percent; charts that never set it (every non-pie series, and
  // pie's own ring 0) are unaffected — `percent` still comes straight from ECharts.
  it("prefers a per-datum parentPercent over ECharts' own percent, when the datum carries one", () => {
    expect(
      itemTooltip(fmt, { percent: true }).formatter({ name: "North", value: 10, percent: 4.17, data: { parentPercent: 50 } }),
    ).toBe("North: $10 (50%)");
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

  // A caller (pie, showing each slice's share beside its name) needs to rewrite the entry
  // text from a number the legend itself knows nothing about, so this takes a callback rather
  // than ECharts' template-string form.
  it("passes a formatter through when given one, and omits the key when not", () => {
    const formatter = (name: string) => `${name} 50%`;
    expect(bottomLegend(true, ["North"], formatter)).toEqual({
      bottom: 0, type: "scroll", textStyle: baseTextStyle, data: ["North"], formatter,
    });
    expect(bottomLegend(true, ["North"])).not.toHaveProperty("formatter");
  });
});

// The same legend as a left-hand COLUMN, for a chart whose host is wide enough that a column
// costs it less than a row does (pie — see options/pie.ts, the only caller of either).
describe("sideLegend", () => {
  it("is a scrollable left column when shown, and nothing when not", () => {
    expect(sideLegend(true)).toEqual({
      orient: "vertical", left: 0, top: "middle", type: "scroll", textStyle: baseTextStyle,
    });
    expect(sideLegend(false)).toBeUndefined();
    expect(sideLegend(undefined)).toBeUndefined();
  });

  it("takes the same explicit data list and formatter as the bottom row", () => {
    const formatter = (name: string) => `${name} 50%`;
    expect(sideLegend(true, ["North", "South"], formatter)).toMatchObject({
      data: ["North", "South"], formatter,
    });
    expect(sideLegend(true)).not.toHaveProperty("data");
    expect(sideLegend(true)).not.toHaveProperty("formatter");
  });

  // A column can align a name left and a percentage right, which a single run of text cannot:
  // ECharts parses a legend formatter's output as rich text once the textStyle declares `rich`
  // styles, and each fragment carries its own width/align.
  it("merges rich-text fragment styles into the shared text style when given them", () => {
    const rich = { n: { width: 97, align: "left" as const }, p: { width: 44, align: "right" as const } };
    expect(sideLegend(true, ["North"], undefined, rich)?.textStyle).toEqual({ ...baseTextStyle, rich });
    expect(sideLegend(true, ["North"])?.textStyle).toEqual(baseTextStyle);
  });
});
