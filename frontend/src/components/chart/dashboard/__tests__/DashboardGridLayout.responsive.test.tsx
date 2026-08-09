import { describe, it, expect, vi, beforeEach } from "vitest";
import { render } from "@testing-library/react";
import { DashboardGridLayout } from "../DashboardGridLayout";

// jsdom gives every element clientWidth 0, so the component would always see width 0 and
// always render mobile. Stub the measurement instead of the component under test.
function withContainerWidth(px: number) {
  Object.defineProperty(HTMLElement.prototype, "clientWidth", {
    configurable: true,
    get() {
      return px;
    },
  });
}

const layout = [
  { i: "a", x: 0, y: 0, w: 12, h: 6 },
  { i: "b", x: 12, y: 0, w: 12, h: 6 },
];

beforeEach(() => {
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
});

describe("DashboardGridLayout breakpoint", () => {
  it("reports desktop and keeps 24 columns at 1136px", () => {
    withContainerWidth(1136);
    const onBreakpoint = vi.fn();
    render(
      <DashboardGridLayout
        editing
        layout={layout}
        kindOf={() => "bar"}
        onBreakpointChange={onBreakpoint}
        onLayoutChange={() => {}}
        onResizeStart={() => {}}
        onResizeStop={() => {}}
        onDragStart={() => {}}
        onDragStop={() => {}}
      >
        {() => <div key="a" data-testid="card-a" />}
      </DashboardGridLayout>,
    );
    expect(onBreakpoint).toHaveBeenCalledWith("desktop");
  });

  it("reports mobile at 390px", () => {
    withContainerWidth(390);
    const onBreakpoint = vi.fn();
    render(
      <DashboardGridLayout
        editing
        layout={layout}
        kindOf={() => "bar"}
        onBreakpointChange={onBreakpoint}
        onLayoutChange={() => {}}
        onResizeStart={() => {}}
        onResizeStop={() => {}}
        onDragStart={() => {}}
        onDragStop={() => {}}
      >
        {() => <div key="a" data-testid="card-a" />}
      </DashboardGridLayout>,
    );
    expect(onBreakpoint).toHaveBeenCalledWith("mobile");
  });

  it("draws no edit-mode grid background on mobile", () => {
    // The guide grid describes a 24-column canvas. On a one-column grid it describes nothing
    // real, and it is the loudest thing on a small screen.
    withContainerWidth(390);
    const { container } = render(
      <DashboardGridLayout
        editing
        layout={layout}
        kindOf={() => "bar"}
        onBreakpointChange={() => {}}
        onLayoutChange={() => {}}
        onResizeStart={() => {}}
        onResizeStop={() => {}}
        onDragStart={() => {}}
        onDragStop={() => {}}
      >
        {() => <div key="a" data-testid="card-a" />}
      </DashboardGridLayout>,
    );
    expect(container.querySelector("[aria-hidden].pointer-events-none")).toBeNull();
  });

  it("renders every card at one column on mobile, and side by side on desktop", () => {
    // The breakpoint callback alone would pass even if the projection were never applied to
    // the grid. This asserts what RGL actually received.
    withContainerWidth(390);
    const mobile = render(
      <DashboardGridLayout
        editing
        layout={layout}
        kindOf={() => "bar"}
        onBreakpointChange={() => {}}
        onLayoutChange={() => {}}
        onResizeStart={() => {}}
        onResizeStop={() => {}}
        onDragStart={() => {}}
        onDragStop={() => {}}
      >
        {() => [<div key="a" data-testid="card-a" />, <div key="b" data-testid="card-b" />]}
      </DashboardGridLayout>,
    );
    const mobileItems = [...mobile.container.querySelectorAll<HTMLElement>(".react-grid-item")];
    expect(mobileItems).toHaveLength(2);
    // One column of a 390px container: every item is the full width and starts at x=0.
    const widths = mobileItems.map((e) => e.style.width);
    expect(new Set(widths).size).toBe(1);
    expect(mobileItems.every((e) => /translate\(0px,/.test(e.style.transform))).toBe(true);

    withContainerWidth(1136);
    const desktop = render(
      <DashboardGridLayout
        editing
        layout={layout}
        kindOf={() => "bar"}
        onBreakpointChange={() => {}}
        onLayoutChange={() => {}}
        onResizeStart={() => {}}
        onResizeStop={() => {}}
        onDragStart={() => {}}
        onDragStop={() => {}}
      >
        {() => [<div key="a" data-testid="card-a" />, <div key="b" data-testid="card-b" />]}
      </DashboardGridLayout>,
    );
    const desktopItems = [...desktop.container.querySelectorAll<HTMLElement>(".react-grid-item")];
    const xs = desktopItems.map((e) => /translate\((-?\d+)px,/.exec(e.style.transform)?.[1]);
    expect(xs[0]).toBe("0");
    expect(xs[1]).not.toBe("0");
  });

  it("disables drag and resize on mobile even while editing", () => {
    // react-grid-layout 1.5.3 always MOUNTS the resize handle and only marks it
    // `react-resizable-hide` when isResizable is false (GridItem.js:451-453), and only adds
    // `react-draggable` when isDraggable is true (:542). So the classes are the observable
    // signal, not the presence of the handle element.
    const props = {
      editing: true as const,
      layout,
      kindOf: () => "bar",
      onBreakpointChange: () => {},
      onLayoutChange: () => {},
      onResizeStart: () => {},
      onResizeStop: () => {},
      onDragStart: () => {},
      onDragStop: () => {},
    };
    withContainerWidth(390);
    const mobile = render(
      <DashboardGridLayout {...props}>{() => <div key="a" data-testid="card-a" />}</DashboardGridLayout>,
    );
    const mobileItem = mobile.container.querySelector(".react-grid-item")!;
    expect(mobileItem.classList.contains("react-draggable")).toBe(false);
    expect(mobile.container.querySelector(".react-resizable-hide")).not.toBeNull();

    withContainerWidth(1136);
    const desktop = render(
      <DashboardGridLayout {...props}>{() => <div key="a" data-testid="card-a" />}</DashboardGridLayout>,
    );
    const desktopItem = desktop.container.querySelector(".react-grid-item")!;
    expect(desktopItem.classList.contains("react-draggable")).toBe(true);
    expect(desktop.container.querySelector(".react-resizable-hide")).toBeNull();
  });
});
