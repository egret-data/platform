import { describe, it, expect } from "vitest";
import { EMPTY_SPEC } from "@/api/charts";
import { getDescriptor } from "../registry";
import { mergeVizSettings } from "../renderConfig";

/**
 * "Auto" on a display control is the absence of a stored setting: the control emits
 * `undefined`, the chart falls back to its own rule, and the key must leave `settings`.
 *
 * Both save paths merge the descriptor's settings OVER the previously loaded ones
 * (ChartExplore's `paramsForSave`, CardVizPanel's `patch`), and a spread cannot express a
 * removal — so a `toSettings` that simply omits an undefined key let the old explicit value
 * survive the save. The control showed Auto, the chart rendered as Auto, and saving wrote
 * `true` straight back: an Auto state reachable but unsavable.
 *
 * These pin the merge itself, not the control's `onChange` — that boundary was always fine.
 */
describe("mergeVizSettings", () => {
  it("clearing a pie setting to Auto removes the previously stored key", () => {
    const pie = getDescriptor("pie")!;
    const stored = { "pie.showLegend": true, "pie.donut": true };
    const merged = mergeVizSettings(stored, pie, { showLegend: undefined, donut: true } as never);

    expect("pie.showLegend" in merged).toBe(false);
    // Only the cleared key goes; everything else this descriptor owns still saves.
    expect(merged["pie.donut"]).toBe(true);
  });

  it("clearing a cartesian setting to Auto removes it too (same codec, every viz type)", () => {
    const bar = getDescriptor("bar")!;
    const stored = { "cartesian.showLegend": false, "cartesian.xAxis": "region" };
    const merged = mergeVizSettings(stored, bar, {
      showLegend: undefined, xAxis: "region", yMetrics: ["rev"],
    } as never);

    expect("cartesian.showLegend" in merged).toBe(false);
    expect(merged["cartesian.xAxis"]).toBe("region");
  });

  it("never touches a key another descriptor or the shell owns", () => {
    const pie = getDescriptor("pie")!;
    const stored = {
      "pie.showLegend": true,
      "cartesian.showLegend": true,
      column_settings: { rev: { decimals: 0 } },
    };
    const merged = mergeVizSettings(stored, pie, { showLegend: undefined } as never);

    expect("pie.showLegend" in merged).toBe(false);
    expect(merged["cartesian.showLegend"]).toBe(true);
    expect(merged.column_settings).toEqual({ rev: { decimals: 0 } });
  });

  it("round-trips: a chart saved from Auto reads back as Auto", () => {
    const pie = getDescriptor("pie")!;
    const merged = mergeVizSettings({ "pie.showLegend": true }, pie, { showLegend: undefined } as never);
    const reloaded = pie.fromParams({ version: 2, spec: EMPTY_SPEC, settings: merged }) as { showLegend?: boolean };
    expect(reloaded.showLegend).toBeUndefined();
  });
});
