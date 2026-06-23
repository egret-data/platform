import { describe, it, expect } from "vitest";
import {
  GRID_COLS, GRID_MARGIN, gridRowHeight, cellWidth, contentRowsFor,
  getPositionForNewDashCard, sizeForViz,
} from "../dashboard-grid";

describe("gridRowHeight (Metabase aspect formula)", () => {
  it("is (width/24)/(10/9), floored", () => {
    expect(gridRowHeight(1200)).toBe(45); // floor(1200/24/(10/9)) = floor(45)
  });
  it("clamps to the 40px minimum for small widths", () => {
    expect(gridRowHeight(0)).toBe(40);
    expect(gridRowHeight(500)).toBe(40); // floor(500/24/(10/9)) ≈ 18 → 40
  });
});

describe("cellWidth", () => {
  it("subtracts the inter-column margins then divides by cols", () => {
    expect(cellWidth(1200)).toBeCloseTo((1200 - (GRID_COLS - 1) * GRID_MARGIN[0]) / GRID_COLS); // (1200-138)/24
  });
});

describe("contentRowsFor(px, rowHeight)", () => {
  it("counts whole rows of (rowHeight + verticalMargin), min 1", () => {
    // rowHeight 45, marginY 6 → row pitch 51
    expect(contentRowsFor(0, 45)).toBe(1);
    expect(contentRowsFor(51, 45)).toBe(1);
    expect(contentRowsFor(52, 45)).toBe(2);
    expect(contentRowsFor(102, 45)).toBe(2);
  });
});

describe("getPositionForNewDashCard (first free slot, L→R T→B)", () => {
  it("places the first card at the origin", () => {
    expect(getPositionForNewDashCard([], 4, 4)).toEqual({ grid_col: 0, grid_row: 0 });
  });
  it("places the next card to the right when there is room", () => {
    const cards = [{ grid_col: 0, grid_row: 0, size_x: 4, size_y: 4 }];
    expect(getPositionForNewDashCard(cards, 4, 4)).toEqual({ grid_col: 4, grid_row: 0 });
  });
  it("wraps to the next row when the row is full", () => {
    const cards = [{ grid_col: 0, grid_row: 0, size_x: 24, size_y: 2 }];
    expect(getPositionForNewDashCard(cards, 6, 4)).toEqual({ grid_col: 0, grid_row: 2 });
  });
});

describe("sizeForViz", () => {
  it("returns Metabase sizes for known viz", () => {
    expect(sizeForViz("table")).toEqual({ default: { w: 12, h: 9 }, min: { w: 4, h: 3 } });
    expect(sizeForViz("scalar")).toEqual({ default: { w: 6, h: 3 }, min: { w: 2, h: 2 } });
    expect(sizeForViz("heading")).toEqual({ default: { w: 24, h: 1 }, min: { w: 1, h: 1 } });
  });
  it("maps egret-specific keys", () => {
    expect(sizeForViz("pivot_table").default).toEqual({ w: 12, h: 9 });
    expect(sizeForViz("smartscalar").min).toEqual({ w: 2, h: 2 });
  });
  it("falls back to a generic chart size for unknown viz", () => {
    expect(sizeForViz("boxplot")).toEqual({ default: { w: 12, h: 6 }, min: { w: 4, h: 3 } });
    expect(sizeForViz("totally-unknown")).toEqual({ default: { w: 12, h: 6 }, min: { w: 4, h: 3 } });
  });
});
