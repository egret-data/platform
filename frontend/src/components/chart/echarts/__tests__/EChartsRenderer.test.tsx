import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render } from "@testing-library/react";
import type { EChartsCoreOption } from "echarts/core";

const handlers: Record<string, (p: unknown) => void> = {};
const instance = vi.hoisted(() => ({
  setOption: vi.fn(),
  resize: vi.fn(),
  dispose: vi.fn(),
  isDisposed: vi.fn(() => false),
  // `on` takes either (event, handler) or ECharts' (event, query, handler) form — the hover
  // listeners use the query form so they only hear about SERIES elements.
  on: vi.fn((evt: string, a: unknown, b?: unknown) => {
    handlers[evt] = (typeof a === "function" ? a : b) as (p: unknown) => void;
  }),
  dispatchAction: vi.fn(),
}));
const init = vi.hoisted(() => vi.fn(() => instance));

vi.mock("echarts/core", () => ({ init, use: vi.fn() }));
vi.mock("../register", () => ({ registerEChartsModules: vi.fn() }));

// ResizeObserver is not in jsdom
beforeEach(() => {
  instance.setOption.mockClear(); instance.resize.mockClear(); instance.dispose.mockClear(); init.mockClear(); instance.on.mockClear();
  instance.dispatchAction.mockClear();
  // clear captured handlers between tests
  for (const key of Object.keys(handlers)) delete handlers[key];
  globalThis.ResizeObserver = class { observe() {} unobserve() {} disconnect() {} } as unknown as typeof ResizeObserver;
});

import { EChartsRenderer } from "../EChartsRenderer";
import { chartImageSvg } from "../../export-chart";

const opt = { series: [] } as EChartsCoreOption;

describe("EChartsRenderer", () => {
  it("inits and sets the option on mount", () => {
    render(<EChartsRenderer option={opt} />);
    expect(init).toHaveBeenCalledTimes(1);
    expect(instance.setOption).toHaveBeenCalledWith(opt, true);
  });
  // KI-043: `.png` exports the <svg> inside the surface this marks, and only that one.
  it("marks its container as the chart image the PNG export reads", () => {
    const { container } = render(<EChartsRenderer option={opt} />);
    // ECharts is mocked, so stand in for the <svg> it would draw into the container.
    const el = container.firstElementChild as HTMLElement;
    el.appendChild(document.createElementNS("http://www.w3.org/2000/svg", "svg"));
    expect(chartImageSvg(container)).toBe(el.querySelector("svg"));
  });
  it("disposes on unmount", () => {
    const { unmount } = render(<EChartsRenderer option={opt} />);
    unmount();
    expect(instance.dispose).toHaveBeenCalledTimes(1);
  });
});

// A theme toggle changes what every var()-backed colour resolves to, but ThemeProvider's
// `children` element is referentially stable, so React never re-renders the chart tree for it —
// the chart has to re-apply its own option, or it keeps the previous theme's colours until a
// reload. Applies to every ECharts chart type, not just pie.
describe("EChartsRenderer — theme changes", () => {
  afterEach(() => {
    document.documentElement.classList.remove("dark");
  });

  const flipTheme = async () => {
    document.documentElement.classList.toggle("dark");
    await new Promise((r) => setTimeout(r, 0));
  };

  it("re-applies the option when the theme changes", async () => {
    render(<EChartsRenderer option={opt} />);
    instance.setOption.mockClear();
    await flipTheme();
    expect(instance.setOption).toHaveBeenCalledWith(opt, true);
  });

  it("stops re-applying once unmounted", async () => {
    const { unmount } = render(<EChartsRenderer option={opt} />);
    unmount();
    instance.setOption.mockClear();
    await flipTheme();
    expect(instance.setOption).not.toHaveBeenCalled();
  });
});

describe("EChartsRenderer onPointClick", () => {
  it("forwards a click as { name, seriesName, value, x, y }", () => {
    const onPointClick = vi.fn();
    render(<EChartsRenderer option={{}} onPointClick={onPointClick} />);
    handlers["click"]?.({ name: "EU", seriesName: "rev", value: 10, event: { event: { clientX: 5, clientY: 7 } } });
    expect(onPointClick).toHaveBeenCalledWith({ name: "EU", seriesName: "rev", value: 10, x: 5, y: 7 });
  });
});

// Hover has to reach React for the pie's hole readout and its legend highlight, and this is the
// only component that talks to the ECharts instance. `mouseover`/`mouseout` rather than
// `mousemove` (which is what Metabase listens to, because its tooltip needs the cursor position):
// these fire once per element the pointer ENTERS, not once per pixel it travels, so nothing
// downstream has to defend against pointer-rate updates.
describe("EChartsRenderer onPointHover", () => {
  it("forwards a series mouseover, and reports a mouseout as no hover at all", () => {
    const onPointHover = vi.fn();
    render(<EChartsRenderer option={{}} onPointHover={onPointHover} />);
    handlers["mouseover"]?.({
      name: "EU", seriesName: "rev", value: 10,
      treePathInfo: [{ name: "" }, { name: "EU" }],
    });
    expect(onPointHover).toHaveBeenCalledWith({
      name: "EU", seriesName: "rev", value: 10, treePathInfo: [{ name: "" }, { name: "EU" }],
    });

    handlers["mouseout"]?.({});
    expect(onPointHover).toHaveBeenLastCalledWith(null);
  });

  // The listeners are registered against ECharts' `series` query, so the hole's own `graphic`
  // text — which sits in the middle of the chart, right where a pointer travelling to the centre
  // crosses — can never be mistaken for a slice.
  it("listens for hovers on series elements only", () => {
    render(<EChartsRenderer option={{}} onPointHover={vi.fn()} />);
    const queries = instance.on.mock.calls
      .filter(([evt]) => evt === "mouseover" || evt === "mouseout")
      .map(([, query]) => query);
    expect(queries).toEqual(["series", "series"]);
  });
});

// The pie's hole is an ECharts `graphic`, and it has to change on every hover — but the option it
// lives in is the expensive one (a per-slice text-measurement pass over every ring), so it cannot
// be rebuilt to say so. `overlay` is the escape hatch: a small partial option applied in MERGE
// mode, which updates the graphic in place and leaves the series untouched.
describe("EChartsRenderer overlay", () => {
  const ovl = { graphic: [{ id: "t", style: { text: "50" } }] };

  it("applies an overlay as a merge, never as a replacement", () => {
    render(<EChartsRenderer option={opt} overlay={ovl} />);
    // One argument — no `true`. A notMerge apply would throw the series away and re-init it.
    expect(instance.setOption).toHaveBeenCalledWith(ovl);
    expect(instance.setOption).toHaveBeenCalledWith(opt, true);
  });

  it("re-states the overlay after the whole option is re-applied", async () => {
    render(<EChartsRenderer option={opt} overlay={ovl} />);
    instance.setOption.mockClear();
    document.documentElement.classList.toggle("dark");
    await new Promise((r) => setTimeout(r, 0));
    document.documentElement.classList.remove("dark");
    // A notMerge apply resets `graphic` to whatever the option says, so the overlay has to follow
    // it — and in that order, or the chart snaps back to its resting hole mid-hover.
    const calls = instance.setOption.mock.calls;
    expect(calls[0]).toEqual([opt, true]);
    expect(calls[1]).toEqual([ovl]);
  });

  it("applies a new overlay on its own, without touching the option", () => {
    const { rerender } = render(<EChartsRenderer option={opt} overlay={ovl} />);
    instance.setOption.mockClear();
    const next = { graphic: [{ id: "t", style: { text: "40" } }] };
    rerender(<EChartsRenderer option={opt} overlay={next} />);
    expect(instance.setOption).toHaveBeenCalledTimes(1);
    expect(instance.setOption).toHaveBeenCalledWith(next);
  });
});

// Metabase highlights the hovered legend entry's slice with `dispatchAction`, which needs the
// ECharts instance — and this component owns it. A prop rather than an imperative handle keeps
// that ownership where it is.
describe("EChartsRenderer highlightDataIndex", () => {
  it("highlights the named datum, and downplays it again when it changes", () => {
    const { rerender } = render(<EChartsRenderer option={opt} highlightDataIndex={3} />);
    expect(instance.dispatchAction).toHaveBeenCalledWith({ type: "highlight", seriesIndex: 0, dataIndex: 3 });

    instance.dispatchAction.mockClear();
    rerender(<EChartsRenderer option={opt} highlightDataIndex={undefined} />);
    expect(instance.dispatchAction).toHaveBeenCalledWith({ type: "downplay", seriesIndex: 0, dataIndex: 3 });
  });

  it("dispatches nothing at all when nothing is highlighted", () => {
    render(<EChartsRenderer option={opt} />);
    expect(instance.dispatchAction).not.toHaveBeenCalled();
  });
});

// A function-valued `option` (pie was the first — see options/pie.ts's `containerSize`; the
// cartesian family, the gauge and the sankey take the same path now, and the list grows — see
// `EChartsOptionInput`, which deliberately does not track it) needs the chart's
// REAL rendered pixel size, which this component is the one place with actual DOM access to
// measure (via ResizeObserver, already used here for `chart.resize()`). These tests drive a
// controllable ResizeObserver mock directly — jsdom has no ResizeObserver at all, and the plain
// no-op stub in this file's own `beforeEach` (and the global one in vitest.setup.ts) never
// invokes its callback, so a test that needs to simulate an actual resize tick has to supply its
// own mock that captures and can invoke that callback on demand, the same way `instance.on`
// above is already captured to simulate a click.
describe("EChartsRenderer — function-valued option (real size plumbing)", () => {
  let roCallback: ResizeObserverCallback | null = null;
  let rectWidth = 530;
  let rectHeight = 420;

  beforeEach(() => {
    roCallback = null;
    rectWidth = 530;
    rectHeight = 420;
    class MockResizeObserver {
      constructor(cb: ResizeObserverCallback) { roCallback = cb; }
      observe() {}
      unobserve() {}
      disconnect() {}
    }
    globalThis.ResizeObserver = MockResizeObserver as unknown as typeof ResizeObserver;
    vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(
      () => ({ width: rectWidth, height: rectHeight, top: 0, left: 0, right: rectWidth, bottom: rectHeight, x: 0, y: 0, toJSON() {} }) as DOMRect,
    );
    // Real rAF is timer-driven (~16ms) in jsdom; the resize-thrash guard below coalesces
    // rebuilds to "the next animation frame", which these tests want to observe synchronously
    // rather than await a real timer.
    vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => { cb(0); return 1; });
    vi.stubGlobal("cancelAnimationFrame", () => {});
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("calls a function-valued option with the measured size on init", () => {
    const optionFn = vi.fn((size: { width: number; height: number }) => ({ size }));
    render(<EChartsRenderer option={optionFn} />);
    expect(optionFn).toHaveBeenCalledWith({ width: 530, height: 420 });
    expect(instance.setOption).toHaveBeenCalledWith({ size: { width: 530, height: 420 } }, true);
  });

  it("calls the function again with the new size when the observed size changes", () => {
    const optionFn = vi.fn((size: { width: number; height: number }) => ({ size }));
    render(<EChartsRenderer option={optionFn} />);
    // Mount already ran optionFn (asserted by the previous test) — clear so this test is only
    // about the DELTA a resize tick adds, independent of exactly how many times mount itself
    // calls it (the pre-existing init-effect + option-effect pair, unrelated to this fix).
    optionFn.mockClear();
    instance.setOption.mockClear();

    rectWidth = 800;
    rectHeight = 600;
    roCallback?.([{ contentRect: { width: 800, height: 600 } }] as unknown as ResizeObserverEntry[], {} as ResizeObserver);

    expect(optionFn).toHaveBeenCalledWith({ width: 800, height: 600 });
    expect(instance.setOption).toHaveBeenLastCalledWith({ size: { width: 800, height: 600 } }, true);
    // chart.resize() still runs every tick regardless — the cheap DOM-size-read/repaint this
    // already did before function-valued options existed.
    expect(instance.resize).toHaveBeenCalled();
  });

  it("does not rebuild the option when a resize tick reports the SAME size (thrash guard)", () => {
    const optionFn = vi.fn((size: { width: number; height: number }) => ({ size }));
    render(<EChartsRenderer option={optionFn} />);
    optionFn.mockClear();
    instance.setOption.mockClear();

    roCallback?.([{ contentRect: { width: 530, height: 420 } }] as unknown as ResizeObserverEntry[], {} as ResizeObserver);

    expect(optionFn).not.toHaveBeenCalled();
    expect(instance.setOption).not.toHaveBeenCalled();
    // resize() itself is harmless and still runs — only the (expensive) option rebuild is guarded.
    expect(instance.resize).toHaveBeenCalled();
  });

  it("still accepts a plain object option and never calls it as a function", () => {
    const opt = { series: [] } as EChartsCoreOption;
    render(<EChartsRenderer option={opt} />);
    expect(instance.setOption).toHaveBeenCalledWith(opt, true);
    instance.setOption.mockClear();

    rectWidth = 999;
    roCallback?.([{ contentRect: { width: 999, height: 420 } }] as unknown as ResizeObserverEntry[], {} as ResizeObserver);
    // A plain-object option never depends on size — no extra setOption call on resize.
    expect(instance.setOption).not.toHaveBeenCalled();
    expect(instance.resize).toHaveBeenCalled();
  });
});
