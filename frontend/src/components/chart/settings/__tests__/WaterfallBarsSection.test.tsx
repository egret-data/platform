import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { WaterfallBarsSection } from "../WaterfallDisplaySection";
import { buildWaterfallOption } from "@/components/chart/echarts/options/waterfall";

/** A colour as the CSSOM normalises it, so a hex and its `rgb()` spelling compare equal. */
const css = (color: string) => {
  const el = document.createElement("span");
  el.style.backgroundColor = color;
  return el.style.backgroundColor;
};

describe("WaterfallBarsSection", () => {
  // KI-057: the Total swatch showed slate (#4C5773) while the bar is drawn in the page's ink.
  it("shows the colour the Total bar is actually drawn in when none is chosen", () => {
    const option = buildWaterfallOption(
      [{ name: "step", type: "varchar" }, { name: "delta", type: "double" }],
      [["a", 10], ["b", -4]],
      { xAxis: "step", metric: "delta" },
    ) as { series: { data: { itemStyle?: { color?: string } }[] }[] };
    const drawn = option.series.flatMap((s) => s.data).at(-1)?.itemStyle?.color;
    expect(drawn).toBeTruthy();

    render(<WaterfallBarsSection config={{}} onChange={() => {}} />);
    expect(screen.getByRole("button", { name: "Total color" }).style.backgroundColor).toBe(css(drawn!));
  });
});
