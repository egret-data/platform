import { describe, it, expect } from "vitest";
import { summarize, isBlank } from "../params";
import type { DashboardParameter } from "@/api/dashboards";

const p = (over: Partial<DashboardParameter> = {}): DashboardParameter =>
  ({ id: "p1", name: "Region", slug: "region", type: "category", operator: "is", ...over });

describe("isBlank", () => {
  it("treats undefined/null/empty-string as blank", () => {
    expect(isBlank(undefined)).toBe(true);
    expect(isBlank(null)).toBe(true);
    expect(isBlank("")).toBe(true);
    expect(isBlank("x")).toBe(false);
    expect(isBlank(0)).toBe(false);
  });
});

describe("summarize", () => {
  it("blank → empty string", () => {
    expect(summarize(p(), undefined)).toBe("");
    expect(summarize(p(), [])).toBe("");
  });
  it("category single vs multi", () => {
    expect(summarize(p(), ["EU"])).toBe("EU");
    expect(summarize(p(), ["EU", "US"])).toBe("2 selections");
  });
  it("number between → a – b", () => {
    expect(summarize(p({ type: "number", operator: "between" }), [1, 5])).toBe("1 – 5");
  });
  it("date relative → label", () => {
    expect(summarize(p({ type: "date", operator: "relative" }), "last7")).toBe("Last 7 days");
  });
  it("scalar → string", () => {
    expect(summarize(p({ type: "text", operator: "is" }), "hello")).toBe("hello");
  });
});
