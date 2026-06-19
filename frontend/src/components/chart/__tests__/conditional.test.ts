import { describe, it, expect } from "vitest";
import { buildConditional } from "../conditional";
import type { Rule } from "../format-types";

const rows = [
  ["EU", 10],
  ["US", 50],
  ["JP", 90],
];
const colIndex = { region: 0, revenue: 1 };

describe("buildConditional single-color", () => {
  it("colors cells matching a > operator", () => {
    const rules: Rule[] = [{ columns: ["revenue"], type: "single", operator: ">", value: 40, color: "#ff0000" }];
    const c = buildConditional(rows, colIndex, rules);
    expect(c.bg(10, "revenue")).toBeNull();
    expect(c.bg(50, "revenue")).toMatch(/^rgba\(255, 0, 0,/);
    expect(c.bg(50, "region")).toBeNull();
  });
  it("string contains", () => {
    const rules: Rule[] = [{ columns: ["region"], type: "single", operator: "contains", value: "U", color: "#00ff00" }];
    const c = buildConditional(rows, colIndex, rules);
    expect(c.bg("EU", "region")).toMatch(/^rgba\(0, 255, 0,/);
    expect(c.bg("JP", "region")).toBeNull();
  });
  it("is-null", () => {
    const rules: Rule[] = [{ columns: ["revenue"], type: "single", operator: "is-null", color: "#0000ff" }];
    const c = buildConditional(rows, colIndex, rules);
    expect(c.bg(null, "revenue")).toMatch(/^rgba\(0, 0, 255,/);
    expect(c.bg(5, "revenue")).toBeNull();
  });
  it("first matching rule wins", () => {
    const rules: Rule[] = [
      { columns: ["revenue"], type: "single", operator: ">", value: 0, color: "#111111" },
      { columns: ["revenue"], type: "single", operator: ">", value: 0, color: "#222222" },
    ];
    const c = buildConditional(rows, colIndex, rules);
    expect(c.bg(50, "revenue")).toMatch(/17, 17, 17/);
  });
});

describe("buildConditional color-range", () => {
  it("interpolates a 2-color gradient across the column extent", () => {
    const rules: Rule[] = [
      { columns: ["revenue"], type: "range", colors: ["#000000", "#ffffff"], min_type: null, max_type: null },
    ];
    const c = buildConditional(rows, colIndex, rules);
    expect(c.bg(10, "revenue")).toMatch(/^rgba\(0, 0, 0,/);
    expect(c.bg(90, "revenue")).toMatch(/^rgba\(255, 255, 255,/);
    expect(c.bg("x", "revenue")).toBeNull();
  });
  it("custom extent halfway = mid grey", () => {
    const rules: Rule[] = [
      { columns: ["revenue"], type: "range", colors: ["#000000", "#ffffff"], min_type: "custom", max_type: "custom", min_value: 0, max_value: 100 },
    ];
    const c = buildConditional(rows, colIndex, rules);
    expect(c.bg(50, "revenue")).toMatch(/^rgba\(128, 128, 128,/);
  });
});

describe("buildConditional row highlight", () => {
  it("rowBg colors a row when a highlight_row rule matches", () => {
    const rules: Rule[] = [{ columns: ["revenue"], type: "single", operator: ">", value: 80, color: "#abcdef", highlight_row: true }];
    const c = buildConditional(rows, colIndex, rules);
    expect(c.rowBg(2)).toMatch(/^rgba\(171, 205, 239, 0.2/);
    expect(c.rowBg(0)).toBeNull();
  });
  it("isPivot disables row highlight", () => {
    const rules: Rule[] = [{ columns: ["revenue"], type: "single", operator: ">", value: 0, color: "#abcdef", highlight_row: true }];
    const c = buildConditional(rows, colIndex, rules, { isPivot: true });
    expect(c.rowBg(0)).toBeNull();
  });
});
