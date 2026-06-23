import { describe, it, expect } from "vitest";
import { paramToFilters } from "../params";
import type { DashboardParameter } from "@/api/dashboards";

const p = (over: Partial<DashboardParameter>): DashboardParameter =>
  ({ id: "1", name: "F", slug: "f", type: "text", operator: "does-not-contain", ...over });

describe("paramToFilters does-not-contain", () => {
  it("maps to not_like with %v% wildcards", () => {
    expect(paramToFilters(p({}), "spam", "title")).toEqual([{ col: "title", op: "not_like", value: "%spam%" }]);
  });
  it("blank → []", () => {
    expect(paramToFilters(p({}), "", "title")).toEqual([]);
  });
});
