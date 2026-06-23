import { describe, it, expect } from "vitest";
import { ROW_PX, ROW_SCALE, GRID_MARGIN_X, GRID_MARGIN_Y, toLayoutRows, fromLayoutRows, contentRowsFor } from "../dashboard-grid";

// The old grid was rowHeight=90, margin=[12,12]. RGL positions item top at y*(rowHeight+marginY)
// and sizes height at n*rowHeight + (n-1)*marginY. To keep existing dashboards in place after the
// ×3 render-scale, the VERTICAL margin must scale by ROW_SCALE too (12→4); otherwise the internal
// margins triple and every card inflates (+96px for a 4-row card). This block guards that.
const OLD_ROW_PX = 90;
const OLD_MARGIN = 12;

describe("dashboard-grid render scale", () => {
  it("renders a stored row as ROW_SCALE layout rows", () => {
    expect(toLayoutRows(4)).toBe(12);
    expect(toLayoutRows(0)).toBe(0);
  });
  it("round-trips stored↔layout for untouched values", () => {
    for (const r of [0, 1, 2, 4, 6, 10]) expect(fromLayoutRows(toLayoutRows(r))).toBe(r);
  });
  it("keeps the same pixel height as the old 90px grid", () => {
    expect(ROW_PX * ROW_SCALE).toBe(90); // 4 stored rows → 12 × 30px = 360px (was 4 × 90px)
  });
  it("scales the vertical margin so card POSITIONS are pixel-identical to the old grid", () => {
    // top(old) = y*(90+12); top(new) = (3y)*(ROW_PX+GRID_MARGIN_Y). Pitch must match exactly.
    expect(GRID_MARGIN_Y).toBe(4);
    expect(ROW_SCALE * (ROW_PX + GRID_MARGIN_Y)).toBe(OLD_ROW_PX + OLD_MARGIN); // 3*34 === 102
    expect(GRID_MARGIN_X).toBe(OLD_MARGIN); // horizontal unchanged
  });
  it("card height differs from the old grid by only a constant +8px (non-distorting), any size", () => {
    const oldH = (n: number) => n * OLD_ROW_PX + (n - 1) * OLD_MARGIN;
    const newH = (n: number) => toLayoutRows(n) * ROW_PX + (toLayoutRows(n) - 1) * GRID_MARGIN_Y;
    for (const n of [1, 2, 4, 6, 12]) expect(newH(n) - oldH(n)).toBe(8);
  });
  it("contentRowsFor rounds content px up to whole stored rows (min 1)", () => {
    expect(contentRowsFor(89)).toBe(1);
    expect(contentRowsFor(90)).toBe(1);
    expect(contentRowsFor(91)).toBe(2);
    expect(contentRowsFor(0)).toBe(1);
  });
});
