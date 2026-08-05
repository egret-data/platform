import { describe, it, expect, vi } from "vitest";
import { render } from "@testing-library/react";
import { EChartsRenderer } from "../EChartsRenderer";
import { buildGaugeOption } from "../options/gauge";

/**
 * THE GAUGE'S DRILL IS A DOM CLICK THAT HAS TO GET OUT OF ECHARTS, and this file is the only thing
 * that can say whether it does — against REAL ECharts, no `vi.mock`.
 *
 * Why the gauge does not use `chart.on("click")` like every other ECharts chart here: a gauge's arc
 * is AXIS-LINE geometry, not data items, so there is no per-segment series element to subscribe to.
 * And it would not help if there were — every segment of a gauge produces the identical drill
 * (`{ dims: [] }`, see `GaugeChart`), so there is no positional meaning to recover. The click target
 * is therefore the whole chart, expressed as a `<button>` wrapping the container.
 *
 * That rests on ONE assumption, and it is exactly the kind this programme has shipped over twice:
 * that ZRender, which installs its own pointer handling on the container, lets a click bubble past
 * it to an ancestor. `ChartRenderer.test.tsx` cannot see that — it mocks `EChartsRenderer`, so its
 * gauge test would pass just as happily over a click ECharts swallowed, which is a test over a dead
 * code path. Hence this file.
 *
 * jsdom does no layout, so the click is dispatched on a node from the rendered SVG rather than at
 * coordinates. That is the only fiction here: the handler chain being exercised — ZRender's own
 * listeners on the container, then DOM bubbling to the button — is the real one.
 */
describe("a click inside an ECharts container reaches a wrapping button", () => {
  const columns = [{ name: "rev", type: "double" }];
  const rows = [[42000]];

  /** Renders the real gauge inside a button, the way `GaugeChart` does when it is drillable. */
  function renderWrapped(onClick: () => void) {
    return render(
      <button type="button" data-testid="wrap" onClick={onClick}>
        <EChartsRenderer option={(size) => buildGaugeOption(columns, rows, { metric: "rev" }, size)} />
      </button>,
    );
  }

  it("draws a real gauge", () => {
    const { container } = renderWrapped(() => {});
    // Not an empty box: ECharts really rendered, so the click below lands on its output rather than
    // on a bare div that would bubble trivially.
    expect(container.querySelectorAll("svg path").length).toBeGreaterThan(0);
  });

  it("bubbles a click on the gauge's own SVG geometry out to the button", () => {
    const onClick = vi.fn();
    const { container } = renderWrapped(onClick);
    const path = container.querySelector("svg path");
    expect(path).not.toBeNull();
    path!.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  // …and from the container ECharts installed its own handlers on, which is the node that would
  // stop the event if anything were going to.
  it("bubbles a click on the ECharts container itself", () => {
    const onClick = vi.fn();
    const { container } = renderWrapped(onClick);
    const host = container.querySelector("[data-testid='wrap'] > div");
    expect(host).not.toBeNull();
    host!.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    expect(onClick).toHaveBeenCalledTimes(1);
  });
});
