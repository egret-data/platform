// frontend/src/components/chart/echarts/__tests__/fold.test.ts
import { describe, it, expect } from "vitest";
import { foldTail } from "../fold";

const points = [
  { name: "a", value: 10 },
  { name: "b", value: 8 },
  { name: "c", value: 6 },
  { name: "d", value: 4 },
  { name: "e", value: 2 },
];

describe("foldTail", () => {
  it("keeps everything when no maximum is set", () => {
    expect(foldTail(points, undefined)).toEqual(points);
  });

  it("keeps the largest n-1 and sums the rest into Other", () => {
    expect(foldTail(points, 3)).toEqual([
      { name: "a", value: 10 },
      { name: "b", value: 8 },
      { name: "Other", value: 12 },
    ]);
  });

  it("does nothing when the data already fits", () => {
    expect(foldTail(points, 5)).toEqual(points);
    expect(foldTail(points, 9)).toEqual(points);
  });

  it("treats a maximum below 2 as no folding — one bar labelled Other says nothing", () => {
    expect(foldTail(points, 1)).toEqual(points);
    expect(foldTail(points, 0)).toEqual(points);
  });

  it("ignores non-numeric values when summing", () => {
    const mixed = [{ name: "a", value: 5 }, { name: "b", value: null }, { name: "c", value: 3 }];
    expect(foldTail(mixed, 2)).toEqual([{ name: "a", value: 5 }, { name: "Other", value: 3 }]);
  });
});
