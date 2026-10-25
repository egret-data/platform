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

  it("never forwards a layout change while it is rendering one column", () => {
    // Measured against react-grid-layout 1.5.3: RGL emits onLayoutChange for the layout it
    // synchronised with its children, and at a mobile width that layout is the one-column
    // projection — [["a",0,1],["b",0,1]]. Forwarding it lets the route write x:0 w:1 into every
    // card. The refusal must live here, in the same closure that produced the layout, because
    // RGL's componentDidUpdate runs a whole commit before this component's effects.
    withContainerWidth(390);
    const onLayoutChange = vi.fn();
    render(
      <DashboardGridLayout
        editing
        layout={layout}
        kindOf={() => "bar"}
        onBreakpointChange={() => {}}
        onLayoutChange={onLayoutChange}
        onResizeStart={() => {}}
        onResizeStop={() => {}}
        onDragStart={() => {}}
        onDragStop={() => {}}
      >
        {() => [<div key="a" data-testid="card-a" />, <div key="b" data-testid="card-b" />]}
      </DashboardGridLayout>,
    );
    expect(onLayoutChange).not.toHaveBeenCalled();
  });

  it("forwards the desktop layout once the container has been measured", () => {
    // The mirror of the test above: the guard must not become a blanket refusal. The first
    // emission of any mount is still the width-0 one-column projection and is dropped; what
    // arrives is the 24-column layout.
    withContainerWidth(1136);
    const onLayoutChange = vi.fn();
    render(
      <DashboardGridLayout
        editing
        layout={layout}
        kindOf={() => "bar"}
        onBreakpointChange={() => {}}
        onLayoutChange={onLayoutChange}
        onResizeStart={() => {}}
        onResizeStop={() => {}}
        onDragStart={() => {}}
        onDragStop={() => {}}
      >
        {() => [<div key="a" data-testid="card-a" />, <div key="b" data-testid="card-b" />]}
      </DashboardGridLayout>,
    );
    expect(onLayoutChange).toHaveBeenCalled();
    for (const call of onLayoutChange.mock.calls) {
      const emitted = call[0] as { i: string; x: number; w: number }[];
      expect(emitted.map((l) => [l.i, l.x, l.w])).toEqual([
        ["a", 0, 12],
        ["b", 12, 12],
      ]);
    }
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

// KI-282: the 340px edit panel (plus its 16px gap) narrowed a 1366px window's canvas to 706px,
// under the 753px breakpoint, and the cards dropped to the one-column layout while the author was
// editing them. The breakpoint is decided as if the canvas had the panel's width back.
describe("DashboardGridLayout breakpoint beside the edit panel (KI-282)", () => {
  const renderAt = (px: number, besidePanelPx?: number) => {
    withContainerWidth(px);
    const onBreakpoint = vi.fn();
    render(
      <DashboardGridLayout
        editing
        layout={layout}
        kindOf={() => "bar"}
        besidePanelPx={besidePanelPx}
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
    return onBreakpoint;
  };

  it("keeps the desktop grid on a 706px canvas while the panel is open", () => {
    const onBreakpoint = renderAt(706, 356);
    // The first report is the unmeasured width 0, as on any canvas; the measured one is desktop.
    expect(onBreakpoint).toHaveBeenLastCalledWith("desktop");
  });

  it("still drops to one column on a phone, panel or not", () => {
    expect(renderAt(390, 356)).toHaveBeenCalledWith("mobile");
  });

  it("drops to one column on a 706px canvas with no panel", () => {
    expect(renderAt(706)).toHaveBeenCalledWith("mobile");
  });
});
