import { describe, it, expect, vi } from "vitest";

// jsdom has no canvas, so every string measures 0 and nothing could ever overhang. A fixed advance
// per character stands in for the browser's measurement — the wiring is what is under test here;
// the arithmetic is `labelOverhangRight`'s own test in boxplot.test.ts.
vi.mock("../../measure-text", () => ({
  measureText: (text: string) => ({ width: text.length * 8, height: 12 }),
}));

import { buildBoxplotOption } from "../options/boxplot";
import { CHART_THEME } from "../palette";

const cols = [{ name: "mode", type: "varchar" }, { name: "price", type: "double" }];
const rows: unknown[][] = ["AIR", "MAIL", "RAIL", "SHIP", "TRUCK", "FOB", "REG AIR"]
  .flatMap((m, i) => [10, 20, 30, 40, 50].map((v) => [m, v * 1000 + i * 137]));
const base = { category: "mode", value: "price" };
const right = (o: unknown) => (o as { grid: { right: number } }).grid.right;

describe("box plot value labels get room at the right edge (KI-165)", () => {
  it("widens the right inset when the last box's label would run off a measured card", () => {
    const size = { width: 330, height: 560 };
    expect(right(buildBoxplotOption(cols, rows, base, size))).toBe(CHART_THEME.grid.right);
    expect(right(buildBoxplotOption(cols, rows, { ...base, showLabels: true }, size))).toBeGreaterThan(CHART_THEME.grid.right as number);
  });

  it("reserves nothing on a card wide enough to hold the label", () => {
    expect(right(buildBoxplotOption(cols, rows, { ...base, showLabels: true }, { width: 4000, height: 560 })))
      .toBe(CHART_THEME.grid.right);
  });
});
