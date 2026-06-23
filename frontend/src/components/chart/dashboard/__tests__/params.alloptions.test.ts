import { describe, it, expect } from "vitest";
import { paramToFilters, summarize } from "../params";
import type { DashboardParameter } from "@/api/dashboards";

const p = (over: Partial<DashboardParameter>): DashboardParameter =>
  ({ id: "1", name: "D", slug: "d", type: "date", operator: "all-options", ...over });
const NOW = new Date("2026-06-15T00:00:00Z");

describe("paramToFilters all-options", () => {
  it("relative past 7 days → ge/le around now", () => {
    const f = paramToFilters(p({}), { mode: "relative", dir: "past", n: 7, unit: "day" }, "d", NOW);
    expect(f).toEqual([{ col: "d", op: "ge", value: "2026-06-08" }, { col: "d", op: "le", value: "2026-06-15" }]);
  });
  it("relative next 2 months → ge now / le now+2mo", () => {
    const f = paramToFilters(p({}), { mode: "relative", dir: "next", n: 2, unit: "month" }, "d", NOW);
    expect(f).toEqual([{ col: "d", op: "ge", value: "2026-06-15" }, { col: "d", op: "le", value: "2026-08-15" }]);
  });
  it("relative covers week and quarter multipliers", () => {
    // NOW = 2026-06-15
    expect(paramToFilters(p({}), { mode: "relative", dir: "past", n: 2, unit: "week" }, "d", NOW))
      .toEqual([{ col: "d", op: "ge", value: "2026-06-01" }, { col: "d", op: "le", value: "2026-06-15" }]);
    expect(paramToFilters(p({}), { mode: "relative", dir: "next", n: 1, unit: "quarter" }, "d", NOW))
      .toEqual([{ col: "d", op: "ge", value: "2026-06-15" }, { col: "d", op: "le", value: "2026-09-15" }]);
  });
  it("specific on → eq; before → lt; after → gt; between → ge+le", () => {
    expect(paramToFilters(p({}), { mode: "specific", op: "on", from: "2026-01-01" }, "d", NOW)).toEqual([{ col: "d", op: "eq", value: "2026-01-01" }]);
    expect(paramToFilters(p({}), { mode: "specific", op: "before", from: "2026-01-01" }, "d", NOW)).toEqual([{ col: "d", op: "lt", value: "2026-01-01" }]);
    expect(paramToFilters(p({}), { mode: "specific", op: "after", from: "2026-01-01" }, "d", NOW)).toEqual([{ col: "d", op: "gt", value: "2026-01-01" }]);
    expect(paramToFilters(p({}), { mode: "specific", op: "between", from: "2026-01-01", to: "2026-03-31" }, "d", NOW))
      .toEqual([{ col: "d", op: "ge", value: "2026-01-01" }, { col: "d", op: "le", value: "2026-03-31" }]);
  });
  it("blank/partial → []", () => {
    expect(paramToFilters(p({}), undefined, "d", NOW)).toEqual([]);
    expect(paramToFilters(p({}), { mode: "specific", op: "between", from: "2026-01-01" }, "d", NOW)).toEqual([{ col: "d", op: "ge", value: "2026-01-01" }]);
    expect(paramToFilters(p({}), { mode: "specific", op: "on", from: "" }, "d", NOW)).toEqual([]);
  });
});

describe("summarize all-options", () => {
  it("labels relative + specific", () => {
    expect(summarize(p({}), { mode: "relative", dir: "past", n: 7, unit: "day" })).toBe("Past 7 days");
    expect(summarize(p({}), { mode: "relative", dir: "next", n: 1, unit: "month" })).toBe("Next 1 month");
    expect(summarize(p({}), { mode: "relative", dir: "current", n: 0, unit: "week" })).toBe("Current week");
    expect(summarize(p({}), { mode: "specific", op: "after", from: "2026-01-01" })).toBe("After 2026-01-01");
    expect(summarize(p({}), { mode: "specific", op: "between", from: "2026-01-01", to: "2026-03-31" })).toBe("2026-01-01 – 2026-03-31");
  });
});
