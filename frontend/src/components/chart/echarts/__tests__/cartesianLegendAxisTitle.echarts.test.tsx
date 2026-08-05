import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { render } from "@testing-library/react";
import * as echarts from "echarts/core";
import { EChartsRenderer } from "../EChartsRenderer";
import { buildCartesianOption } from "../options/cartesian";

/**
 * THE VALUE AXIS'S TITLE FOLLOWS THE LEGEND, for the same reason #412's description does.
 *
 * CONFIRMED IN A BROWSER BEFORE ANY OF THIS WAS WRITTEN, because the finding this comes from was
 * INFERRED rather than measured — it was carried on #412's "found, not fixed" list, and five items
 * from equally careful lists have since evaporated on re-inspection. On the deployed "MB · Combo"
 * (`/explore/demo/charts/be2dce76…`), the built chart draws `orders` against a left axis titled
 * "orders" and `revenue` against a right one. Clicking the `orders` legend entry off leaves:
 *
 *   - the left axis's tick labels gone (0/100/200/300/400/500 all removed by ECharts, which has no
 *     extent left to scale) — the axis line is drawn but says nothing;
 *   - the rotated title "orders" STILL PAINTED beside it, and moved 24px left as the plot expanded
 *     into the space the labels vacated.
 *
 * So a chart drawing revenue alone carries a title naming a series nobody can see, next to an axis
 * carrying no numbers. Measured, not inferred: the `<text>` node's own box moved from x=573 to
 * x=549 while its content stayed "orders".
 *
 * THE MECHANISM IS #412'S, NOT A SECOND ONE. ECharts' legend hides a series without the option
 * being rebuilt, so anything the builder decided FROM the series list goes stale on a legend click —
 * the description was the first case, the axis title is the second, and they are the same bug. So
 * this rides the `legendselectchanged` subscription `EChartsRenderer` already has: the builder
 * offers `__axisNamesFor(selected)` beside `__ariaFor(selected)`, and the one handler merges both.
 *
 * WHY THIS FILE RENDERS REAL ECHARTS: identical to `cartesianLegendAria.echarts.test.tsx`'s reason.
 * `legendselectchanged` fires from inside ECharts' legend component and the axis name is read back
 * out of ECharts' own model, so a mocked instance would prove neither end.
 */

const COLS = [
  { name: "month", type: "varchar" },
  { name: "rev", type: "double" },
  { name: "cnt", type: "double" },
];
const ROWS: unknown[][] = [["2026-01", 11, 71], ["2026-02", 22, 82]];

const SIZE = 600;

const chartOf = (c: HTMLElement) => {
  const host = c.querySelector<HTMLElement>("[_echarts_instance_]") ?? (c.firstElementChild as HTMLElement);
  const inst = echarts.getInstanceByDom(host);
  if (!inst) throw new Error("no ECharts instance on the rendered host");
  return { host, inst };
};

/** `dispatchAction` does not repaint synchronously — zrender flushes on its next frame. */
const settle = () => new Promise((r) => setTimeout(r, 20));

/** The action a real click on a legend entry dispatches, and the only one that emits
 *  `legendselectchanged` — see the note in `cartesianLegendAria.echarts.test.tsx`, which had to
 *  learn this the hard way. */
const toggle = (inst: echarts.ECharts, name: string) =>
  inst.dispatchAction({ type: "legendToggleSelect", name });

/** The name ECharts' own model reports for a value axis. `getOption()` normalises a single axis to
 *  a one-element array, so both shapes are read the same way. An axis with no name reports `""`. */
const axisName = (inst: echarts.ECharts, key: "xAxis" | "yAxis" = "yAxis", i = 0) =>
  ((inst.getOption() as Record<string, { name?: string }[]>)[key][i].name ?? "");

beforeEach(() => {
  Object.defineProperty(HTMLElement.prototype, "getBoundingClientRect", {
    configurable: true,
    value: () => ({ width: SIZE, height: SIZE, top: 0, left: 0, right: SIZE, bottom: SIZE, x: 0, y: 0, toJSON() {} }),
  });
  for (const prop of ["clientWidth", "clientHeight"]) {
    Object.defineProperty(HTMLElement.prototype, prop, { configurable: true, value: SIZE });
  }
});

afterEach(() => {
  for (const prop of ["getBoundingClientRect", "clientWidth", "clientHeight"]) {
    delete (HTMLElement.prototype as unknown as Record<string, unknown>)[prop];
  }
});

describe("the value axis's title follows the legend", () => {
  /** The reproduction's shape: two metrics, one banished to the right axis, so the left axis has
   *  exactly one metric and therefore a title. */
  const dualOption = () =>
    buildCartesianOption(
      COLS, ROWS,
      { kind: "combo", xAxis: "month", yMetrics: ["rev", "cnt"], rightAxisMetrics: ["cnt"] },
      { width: SIZE, height: SIZE },
    );

  it("titles the left axis with its only metric before anything is toggled", async () => {
    const { container } = render(<EChartsRenderer option={dualOption()} />);
    await settle();
    expect(axisName(chartOf(container).inst)).toBe("rev");
  });

  it("drops the title when the left axis's only series is hidden", async () => {
    // THE DEFECT. The axis has no series left, so ECharts stops drawing its numbers; a title left
    // standing there names a series that is not on the chart.
    const { container } = render(<EChartsRenderer option={dualOption()} />);
    await settle();
    const { inst } = chartOf(container);
    toggle(inst, "rev");
    await settle();
    expect(axisName(inst)).toBe("");
  });

  it("does not disturb the axis the legend click did not touch", async () => {
    // Hiding the RIGHT axis's series must leave the left axis titled as built. The rule is keyed on
    // `yAxisIndex`, exactly as the build-time one is, so a fix that recomputed from the config
    // rather than from the surviving series would fail here.
    const { container } = render(<EChartsRenderer option={dualOption()} />);
    await settle();
    const { inst } = chartOf(container);
    toggle(inst, "cnt");
    await settle();
    expect(axisName(inst)).toBe("rev");
  });

  it("restores the built title when the entry is switched back on", async () => {
    const { container } = render(<EChartsRenderer option={dualOption()} />);
    await settle();
    const { inst } = chartOf(container);
    toggle(inst, "rev");
    await settle();
    toggle(inst, "rev");
    await settle();
    expect(axisName(inst)).toBe("rev");
  });

  /**
   * The other direction, and the reason this is a rule rather than a patch on one case: an axis
   * carrying TWO metrics has no single name, so the builder deliberately leaves it untitled and
   * lets the legend name them. Hide one and the axis IS a single metric's axis — it should now say
   * whose numbers those are. Same one-line rule (`metricTitle` over the surviving series), read the
   * other way.
   */
  it("gains a title when hiding leaves one metric on a shared axis", async () => {
    const option = () =>
      buildCartesianOption(COLS, ROWS, { kind: "bar", xAxis: "month", yMetrics: ["rev", "cnt"] }, { width: SIZE, height: SIZE });
    const { container } = render(<EChartsRenderer option={option()} />);
    await settle();
    const { inst } = chartOf(container);
    expect(axisName(inst), "two metrics share the axis, so it is built untitled").toBe("");
    toggle(inst, "cnt");
    await settle();
    expect(axisName(inst)).toBe("rev");
  });

  /**
   * An axis title the AUTHOR typed is not a derived label and must not follow anything. `yLabel` is
   * the one branch of the title ladder that outranks the metric name at build time, and it has to
   * keep outranking it here — otherwise a legend click would silently delete text the user wrote.
   */
  it("never touches an explicit yLabel", async () => {
    const option = () =>
      buildCartesianOption(
        COLS, ROWS,
        { kind: "combo", xAxis: "month", yMetrics: ["rev", "cnt"], rightAxisMetrics: ["cnt"], yLabel: "Money" },
        { width: SIZE, height: SIZE },
      );
    const { container } = render(<EChartsRenderer option={option()} />);
    await settle();
    const { inst } = chartOf(container);
    toggle(inst, "rev");
    await settle();
    expect(axisName(inst)).toBe("Money");
  });

  /** …and `showYLabel: false` means no title in any legend state, not "no title until you click". */
  it("stays untitled when the author turned the label off", async () => {
    const option = () =>
      buildCartesianOption(
        COLS, ROWS,
        { kind: "bar", xAxis: "month", yMetrics: ["rev", "cnt"], showYLabel: false },
        { width: SIZE, height: SIZE },
      );
    const { container } = render(<EChartsRenderer option={option()} />);
    await settle();
    const { inst } = chartOf(container);
    toggle(inst, "cnt");
    await settle();
    expect(axisName(inst)).toBe("");
  });

  /**
   * The surviving metric's DISPLAY name, not its column name — the same rename ladder `titleOf`
   * applies at build time. A fix that re-derived from `cfg.yMetrics` would put the raw column name
   * back on the axis after a legend click and undo the rename only in that state.
   */
  it("uses the surviving series' renamed label", async () => {
    const option = () =>
      buildCartesianOption(
        COLS, ROWS,
        { kind: "bar", xAxis: "month", yMetrics: ["rev", "cnt"], seriesLabels: { rev: "Revenue" } },
        { width: SIZE, height: SIZE },
      );
    const { container } = render(<EChartsRenderer option={option()} />);
    await settle();
    const { inst } = chartOf(container);
    toggle(inst, "cnt");
    await settle();
    expect(axisName(inst)).toBe("Revenue");
  });

  /**
   * A ROW CHART'S VALUE AXIS IS ITS `xAxis`. The builder swaps the two when `horizontal`, so a fix
   * that always merged `yAxis` would write the metric name over the CATEGORY axis's title here —
   * visibly wrong, and invisible to every test above.
   */
  it("follows the legend on a row chart, where the value axis is the x one", async () => {
    const option = () =>
      buildCartesianOption(
        COLS, ROWS,
        { kind: "bar", horizontal: true, xAxis: "month", yMetrics: ["rev", "cnt"] },
        { width: SIZE, height: SIZE },
      );
    const { container } = render(<EChartsRenderer option={option()} />);
    await settle();
    const { inst } = chartOf(container);
    toggle(inst, "cnt");
    await settle();
    expect(axisName(inst, "xAxis")).toBe("rev");
    // …and the category axis, which is the `yAxis` on this chart, keeps its own title.
    expect(axisName(inst, "yAxis")).toBe("month");
  });

  /**
   * THE PATCH MUST RE-TITLE AN AXIS, NEVER ADD ONE. `axisNamesFor` addresses the value axis by the
   * `id` `mkValueAxis` stamps on it, and ECharts' component matching treats an id that matches
   * nothing as a request to CREATE that component — so dropping the id from the builder while the
   * patch still names it does not degrade to position-matching, it grows a third y-axis. Measured
   * before this was written: `ids: [undefined, undefined, "yValue"]`, the phantom carrying the new
   * title while the real left axis kept the stale one. Nothing else in this file would notice,
   * because the phantom's title reads correctly when you ask it by index.
   */
  it("re-titles the existing axis rather than adding one", async () => {
    const { container } = render(<EChartsRenderer option={dualOption()} />);
    await settle();
    const { inst } = chartOf(container);
    const before = (inst.getOption() as { yAxis: unknown[] }).yAxis.length;
    expect(before).toBe(2);
    toggle(inst, "rev");
    await settle();
    expect((inst.getOption() as { yAxis: unknown[] }).yAxis.length).toBe(before);
  });

  /** The legend selection itself must survive the merge. A `notMerge` apply would reset it, which
   *  is the state being described — the same trap #412's handler comment names. */
  it("leaves the hidden series hidden", async () => {
    const { container } = render(<EChartsRenderer option={dualOption()} />);
    await settle();
    const { inst } = chartOf(container);
    toggle(inst, "rev");
    await settle();
    const legend = (inst.getOption() as { legend: { selected?: Record<string, boolean> }[] }).legend[0];
    expect(legend.selected?.rev).toBe(false);
  });
});
