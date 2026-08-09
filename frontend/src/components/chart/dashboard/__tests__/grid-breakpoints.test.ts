import { describe, it, expect } from "vitest";
import {
  GRID_BREAKPOINTS,
  MOBILE_DEFAULT_CARD_HEIGHT,
  breakpointFor,
  mobileHeightFor,
  toMobileLayout,
} from "../grid-breakpoints";

describe("breakpointFor", () => {
  it("is mobile at and below 752 and desktop at 753", () => {
    // Ported from Metabase utils/dashboard_grid.ts:11-16 — the two numbers are adjacent
    // on purpose, so this pins the boundary rather than a value near it.
    expect(breakpointFor(752)).toBe("mobile");
    expect(breakpointFor(GRID_BREAKPOINTS.desktop)).toBe("desktop");
    expect(breakpointFor(0)).toBe("mobile");
  });
});

describe("mobileHeightFor", () => {
  it("uses the per-kind heights, and text keeps its desktop height when taller", () => {
    expect(mobileHeightFor("link", 1)).toBe(1);
    expect(mobileHeightFor("heading", 1)).toBe(2);
    expect(mobileHeightFor("scalar", 3)).toBe(4);
    expect(mobileHeightFor("smartscalar", 3)).toBe(4);
    expect(mobileHeightFor("text", 1)).toBe(2);
    expect(mobileHeightFor("text", 7)).toBe(7);
    expect(mobileHeightFor("bar", 6)).toBe(MOBILE_DEFAULT_CARD_HEIGHT);
    expect(mobileHeightFor("anything-unknown", 99)).toBe(MOBILE_DEFAULT_CARD_HEIGHT);
  });
});

describe("toMobileLayout", () => {
  const desktop = [
    { i: "b", x: 12, y: 0, w: 12, h: 6 },
    { i: "a", x: 0, y: 0, w: 12, h: 6 },
    { i: "c", x: 0, y: 6, w: 24, h: 3 },
  ];
  const kinds = { a: "bar", b: "scalar", c: "heading" };

  it("stacks every card at x:0 w:1 in reading order, not array order", () => {
    const out = toMobileLayout(desktop, (id) => kinds[id as keyof typeof kinds]);
    expect(out.map((l) => l.i)).toEqual(["a", "b", "c"]);
    expect(out.every((l) => l.x === 0 && l.w === 1)).toBe(true);
  });

  it("assigns heights by kind and stacks y with no gaps", () => {
    const out = toMobileLayout(desktop, (id) => kinds[id as keyof typeof kinds]);
    expect(out.map((l) => [l.h, l.y])).toEqual([
      [6, 0],
      [4, 6],
      [2, 10],
    ]);
  });

  it("never emits minW/maxH that would pin a card wider than the single column", () => {
    // The desktop layout carries minW from per-viz minimums (DashboardView.tsx:342-352).
    // Carrying those into a 1-column grid makes RGL warn and clamp every card back to
    // multi-column widths, which is exactly the bug this projection exists to prevent.
    const withMins = [{ i: "a", x: 0, y: 0, w: 12, h: 6, minW: 4, minH: 3, maxH: 6 }];
    const out = toMobileLayout(withMins, () => "bar");
    expect(out[0].minW).toBe(1);
    expect(out[0].maxH).toBeUndefined();
  });
});
