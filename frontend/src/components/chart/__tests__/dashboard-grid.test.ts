import { describe, it, expect } from "vitest";
import { ROW_PX, ROW_SCALE, toLayoutRows, fromLayoutRows, contentRowsFor } from "../dashboard-grid";

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
  it("contentRowsFor rounds content px up to whole stored rows (min 1)", () => {
    expect(contentRowsFor(89)).toBe(1);
    expect(contentRowsFor(90)).toBe(1);
    expect(contentRowsFor(91)).toBe(2);
    expect(contentRowsFor(0)).toBe(1);
  });
});
