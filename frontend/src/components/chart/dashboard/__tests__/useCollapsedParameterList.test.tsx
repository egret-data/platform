import { describe, it, expect, beforeAll, afterAll } from "vitest";
import { render, screen } from "@testing-library/react";
import { useCollapsedParameterList } from "../useCollapsedParameterList";

/**
 * jsdom lays nothing out — every width is 0 — so this file FAKES the geometry rather than
 * pretending to measure it, and it fakes the two numbers that must not be confused:
 *
 *   `getBoundingClientRect().width` — the box as LAID OUT. A `min-w-0 truncate` title shrinks to
 *   whatever is left, so this number is bounded by the container and the collision sum can never
 *   exceed it.
 *   `scrollWidth` — how wide the content would LIKE to be. Unbounded by the flex row.
 *
 * The whole collapse is a silent no-op if the hook reads the first, and no test that measures only
 * one of them can tell. So the reserved node is given a laid-out width of 50 and a scrollWidth of
 * 300, and the two answers are on opposite sides of the threshold.
 */
const rects = new Map<string, number>();
let origRect: typeof Element.prototype.getBoundingClientRect;

beforeAll(() => {
  origRect = Element.prototype.getBoundingClientRect;
  Element.prototype.getBoundingClientRect = function () {
    const w = rects.get((this as HTMLElement).dataset?.probe ?? "") ?? 0;
    return { width: w, height: 0, top: 0, left: 0, right: w, bottom: 0, x: 0, y: 0, toJSON: () => ({}) } as DOMRect;
  };
  // No ResizeObserver in jsdom. A no-op stub proves the hook's mount-time `measure()` is what
  // produces the first answer, and that the observer path is guarded rather than assumed.
  (globalThis as { ResizeObserver?: unknown }).ResizeObserver = class {
    observe() {} unobserve() {} disconnect() {}
  };
});

afterAll(() => {
  Element.prototype.getBoundingClientRect = origRect;
  delete (globalThis as { ResizeObserver?: unknown }).ResizeObserver;
});

function Probe({ bufferPx }: { bufferPx: number }) {
  const { collapsed, containerRef, reservedRef, listRef } = useCollapsedParameterList(bufferPx);
  return (
    <div ref={containerRef} data-probe="container">
      <div ref={reservedRef} data-probe="reserved">title</div>
      <div ref={listRef} data-probe="list" />
      <span data-testid="answer">{String(collapsed)}</span>
    </div>
  );
}

function setup(reservedScrollWidth: number, bufferPx = 100) {
  rects.set("container", 300);
  rects.set("reserved", 50);     // laid out: shrunk by `min-w-0 truncate`
  rects.set("list", 100);
  // jsdom never computes scrollWidth, so it is declared on the prototype for the duration.
  Object.defineProperty(HTMLElement.prototype, "scrollWidth", {
    configurable: true,
    get() { return (this as HTMLElement).dataset?.probe === "reserved" ? reservedScrollWidth : 0; },
  });
  render(<Probe bufferPx={bufferPx} />);
  return screen.getByTestId("answer").textContent;
}

describe("useCollapsedParameterList", () => {
  it("collapses on the title's INTRINSIC width, which its laid-out box hides", () => {
    // scrollWidth 300 + list 100 + buffer 100 = 500 > container 300 → collapse.
    // Reading getBoundingClientRect().width instead gives 50 + 100 + 100 = 250, which is NOT
    // greater than 300 — so the mutant answers "there is room" and the collapse never fires.
    expect(setup(300)).toBe("true");
  });

  it("stays expanded when even the intrinsic title leaves room", () => {
    // 50 + 100 + 100 = 250 <= 300.
    expect(setup(50)).toBe("false");
  });
});
