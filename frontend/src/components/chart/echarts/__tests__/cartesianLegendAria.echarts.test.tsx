import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { render } from "@testing-library/react";
import * as echarts from "echarts/core";
import { EChartsRenderer } from "../EChartsRenderer";
import { buildCartesianOption } from "../options/cartesian";

/**
 * THE CARTESIAN DESCRIPTION FOLLOWS THE LEGEND.
 *
 * CONFIRMED IN A BROWSER FIRST, because the audit could only infer it. On the deployed "MB · Combo"
 * chart (`/explore/demo/charts/be2dce76…`), clicking the `orders` legend entry left the chart
 * drawing revenue alone — the 0-400 orders axis disappeared — while the container's `aria-label`
 * still read, verbatim and unchanged:
 *
 *   "Combo chart of orders by order_month. 12 values: 2 series; January 2026 205, February 2026
 *    206, …, December 2026 418."
 *
 * Two series, named for the hidden one, enumerating the hidden one's values. Same shape as the
 * pie's `toggledOffSlices` defect, one family over.
 *
 * WHY RE-DERIVE RATHER THAN STOP ENUMERATING. The other candidate fix was to drop the values from
 * any chart that HAS a legend, on the grounds that a legend can hide them. That pays for a state
 * most readers never enter with a permanent loss for every reader who does not: the enumeration is
 * the only thing in the description carrying actual numbers, and a multi-series chart is exactly
 * where a screen-reader user has the least other recourse. Re-deriving costs one event subscription
 * and keeps the description true in every state, including "everything hidden", which now says
 * "No data." rather than naming a series nobody can see.
 *
 * WHY THIS FILE RENDERS REAL ECHARTS. The description is set by ECharts' own `aria` component onto
 * the container element; a mock can see that `setOption` was called and nothing about whether the
 * attribute moved. The event is ECharts' too — `legendselectchanged` fires from inside the legend
 * component, so nothing short of a real instance proves the subscription is on the right event.
 */

const COLS = [
  { name: "month", type: "varchar" },
  { name: "rev", type: "double" },
  { name: "cnt", type: "double" },
];
// Every number distinct, and no value of one series appears in the other, so an assertion that the
// description switched series cannot pass on a shared digit.
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

/**
 * `legendToggleSelect` IS WHAT A CLICK ON A LEGEND ENTRY DISPATCHES, and it is the only legend
 * action that emits `legendselectchanged`. The first draft of this file used `legendUnSelect`,
 * whose event is `legendunselected` — so the specs failed against the finished code and would have
 * been "fixed" by widening the subscription to an event no click can produce. The others
 * (`legendAllSelect`, `legendInverseSelect`) come from the legend's `selector` UI, which no chart
 * here turns on.
 */
const toggle = (inst: echarts.ECharts, name: string) =>
  inst.dispatchAction({ type: "legendToggleSelect", name });

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

describe("the description follows the legend rather than the option it was built with", () => {
  const option = () =>
    buildCartesianOption(COLS, ROWS, { kind: "bar", xAxis: "month", yMetrics: ["rev", "cnt"] }, { width: SIZE, height: SIZE });

  it("describes the first series before anything is toggled", async () => {
    const { container } = render(<EChartsRenderer option={option()} />);
    await settle();
    const { host } = chartOf(container);
    const d = host.getAttribute("aria-label") ?? "";
    expect(d).toContain("of rev by month");
    expect(d).toContain("2 series;");
    expect(d).toContain("2026-01 11");
  });

  it("moves to the surviving series when the described one is hidden", async () => {
    const { container } = render(<EChartsRenderer option={option()} />);
    await settle();
    const { host, inst } = chartOf(container);
    toggle(inst, "rev");
    await settle();
    const d = host.getAttribute("aria-label") ?? "";
    expect(d).toContain("of cnt by month");
    expect(d).not.toContain("of rev by month");
    expect(d).toContain("2026-01 71");
    // The hidden series' values must be gone, and the count must be the count of what is drawn.
    expect(d).not.toContain("2026-01 11");
    expect(d).not.toContain("2 series;");
  });

  it("says there is nothing to describe once every series is hidden", async () => {
    const { container } = render(<EChartsRenderer option={option()} />);
    await settle();
    const { host, inst } = chartOf(container);
    toggle(inst, "rev");
    toggle(inst, "cnt");
    await settle();
    expect(host.getAttribute("aria-label") ?? "").toContain("No data.");
  });

  it("comes back when the entry is switched on again", async () => {
    const { container } = render(<EChartsRenderer option={option()} />);
    await settle();
    const { host, inst } = chartOf(container);
    const before = host.getAttribute("aria-label");
    toggle(inst, "rev");
    await settle();
    toggle(inst, "rev");
    await settle();
    expect(host.getAttribute("aria-label")).toBe(before);
  });
});
