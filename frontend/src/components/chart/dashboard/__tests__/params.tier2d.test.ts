import { describe, it, expect } from "vitest";
import { paramToFilters, summarize } from "../params";
import type { DashboardParameter } from "@/api/dashboards";

const p = (over: Partial<DashboardParameter>): DashboardParameter =>
  ({ id: "1", name: "F", slug: "f", type: "text", operator: "is", ...over });

describe("paramToFilters — tier2d types", () => {
  it("boolean 'true' → eq true; 'false' → eq false; blank → []", () => {
    const param = p({ type: "boolean", operator: "is" });
    expect(paramToFilters(param, "true", "active")).toEqual([{ col: "active", op: "eq", value: true }]);
    expect(paramToFilters(param, "false", "active")).toEqual([{ col: "active", op: "eq", value: false }]);
    expect(paramToFilters(param, "", "active")).toEqual([]);
    expect(paramToFilters(param, false, "active")).toEqual([{ col: "active", op: "eq", value: false }]);
  });
  it("location is → in array; is-not → not_in array; blank → []", () => {
    expect(paramToFilters(p({ type: "location", operator: "is" }), ["US", "CA"], "country"))
      .toEqual([{ col: "country", op: "in", value: ["US", "CA"] }]);
    expect(paramToFilters(p({ type: "location", operator: "is-not" }), ["US"], "country"))
      .toEqual([{ col: "country", op: "not_in", value: ["US"] }]);
    expect(paramToFilters(p({ type: "location", operator: "is" }), [], "country")).toEqual([]);
  });
  it("month-year 'YYYY-MM' → ge first-of-month + le last-of-month", () => {
    expect(paramToFilters(p({ type: "date", operator: "month-year" }), "2024-03", "d"))
      .toEqual([{ col: "d", op: "ge", value: "2024-03-01" }, { col: "d", op: "le", value: "2024-03-31" }]);
    expect(paramToFilters(p({ type: "date", operator: "month-year" }), "", "d")).toEqual([]);
  });
  it("quarter-year 'YYYY-Q' → ge quarter-start + le quarter-end", () => {
    expect(paramToFilters(p({ type: "date", operator: "quarter-year" }), "2024-2", "d"))
      .toEqual([{ col: "d", op: "ge", value: "2024-04-01" }, { col: "d", op: "le", value: "2024-06-30" }]);
    expect(paramToFilters(p({ type: "date", operator: "quarter-year" }), "2024-1", "d"))
      .toEqual([{ col: "d", op: "ge", value: "2024-01-01" }, { col: "d", op: "le", value: "2024-03-31" }]);
    expect(paramToFilters(p({ type: "date", operator: "quarter-year" }), "2024", "d")).toEqual([]);
  });
});

describe("summarize — tier2d types", () => {
  it("boolean → True/False", () => {
    expect(summarize(p({ type: "boolean", operator: "is" }), "true")).toBe("True");
    expect(summarize(p({ type: "boolean", operator: "is" }), false)).toBe("False");
    expect(summarize(p({ type: "boolean", operator: "is" }), "")).toBe("");
  });
  it("quarter-year → 'Q2 2024'", () => {
    expect(summarize(p({ type: "date", operator: "quarter-year" }), "2024-2")).toBe("Q2 2024");
  });
  it("month-year → the value string", () => {
    expect(summarize(p({ type: "date", operator: "month-year" }), "2024-03")).toBe("2024-03");
  });
  it("location multi → 'N selections'", () => {
    expect(summarize(p({ type: "location", operator: "is" }), ["US", "CA"])).toBe("2 selections");
  });
});
