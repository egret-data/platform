import { describe, it, expect } from "vitest";
import { paramToFilters, applyParamsToChart, interpolateParams, slugify } from "../params";
import type { DashboardParameter } from "@/api/dashboards";
import type { Chart } from "@/api/charts";

const p = (over: Partial<DashboardParameter>): DashboardParameter => ({
  id: "p1", name: "P", slug: "p", type: "text", operator: "is", ...over,
});

describe("slugify", () => {
  it("kebabs and uniquifies", () => {
    expect(slugify("Order Region", [])).toBe("order_region");
    expect(slugify("Order Region", ["order_region"])).toBe("order_region_2");
  });
});

describe("paramToFilters", () => {
  it("text operators", () => {
    expect(paramToFilters(p({ operator: "is" }), "EU", "region")).toEqual([{ col: "region", op: "eq", value: "EU" }]);
    expect(paramToFilters(p({ operator: "is-not" }), "EU", "region")).toEqual([{ col: "region", op: "ne", value: "EU" }]);
    expect(paramToFilters(p({ operator: "contains" }), "EU", "region")).toEqual([{ col: "region", op: "like", value: "%EU%" }]);
    expect(paramToFilters(p({ operator: "starts-with" }), "EU", "region")).toEqual([{ col: "region", op: "like", value: "EU%" }]);
    expect(paramToFilters(p({ operator: "ends-with" }), "EU", "region")).toEqual([{ col: "region", op: "like", value: "%EU" }]);
  });
  it("number operators incl. between → two filters", () => {
    expect(paramToFilters(p({ type: "number", operator: "=" }), 5, "n")).toEqual([{ col: "n", op: "eq", value: 5 }]);
    expect(paramToFilters(p({ type: "number", operator: ">=" }), 5, "n")).toEqual([{ col: "n", op: "ge", value: 5 }]);
    expect(paramToFilters(p({ type: "number", operator: "between" }), [2, 8], "n")).toEqual([{ col: "n", op: "ge", value: 2 }, { col: "n", op: "le", value: 8 }]);
  });
  it("date range and single", () => {
    expect(paramToFilters(p({ type: "date", operator: "range" }), ["2026-01-01", "2026-02-01"], "d")).toEqual([{ col: "d", op: "ge", value: "2026-01-01" }, { col: "d", op: "le", value: "2026-02-01" }]);
    expect(paramToFilters(p({ type: "date", operator: "single" }), "2026-01-01", "d")).toEqual([{ col: "d", op: "eq", value: "2026-01-01" }]);
  });
  it("date relative computes a [start,end] range from a fixed now", () => {
    const now = new Date("2026-01-31T00:00:00Z");
    const out = paramToFilters(p({ type: "date", operator: "relative" }), "last7", "d", now);
    expect(out).toHaveLength(2);
    expect(out[0]).toMatchObject({ col: "d", op: "ge" });
    expect(out[1]).toMatchObject({ col: "d", op: "le", value: "2026-01-31" });
    expect((out[0] as { col: string; op: string; value: unknown }).value).toBe("2026-01-24");
  });
  it("category multi-select → in", () => {
    expect(paramToFilters(p({ type: "category", operator: "is" }), ["EU", "US"], "region")).toEqual([{ col: "region", op: "in", value: ["EU", "US"] }]);
  });
  it("id → eq", () => {
    expect(paramToFilters(p({ type: "id", operator: "is" }), 42, "id")).toEqual([{ col: "id", op: "eq", value: 42 }]);
  });
  it("blank / empty values produce no filters", () => {
    expect(paramToFilters(p({}), "", "region")).toEqual([]);
    expect(paramToFilters(p({ type: "category" }), [], "region")).toEqual([]);
    expect(paramToFilters(p({ type: "number", operator: "between" }), [null, null], "n")).toEqual([]);
  });
});

const chart = (): Chart => ({
  id: "c1", dataset_id: "ds1", name: "Sales", viz_type: "table", description: "",
  params: { spec: { metrics: ["rev"], dimensions: ["region"], columns: [], filters: [{ col: "region", op: "is_not_null" }], orderby: [], row_limit: 1000, rollup_groups: [] } },
  created_by: null, updated_at: "",
});

describe("applyParamsToChart", () => {
  it("injects mapped param filters and keeps existing ones", () => {
    const params: DashboardParameter[] = [p({ id: "pr", slug: "region", operator: "is" })];
    const out = applyParamsToChart(chart(), params, { region: "EU" }, { pr: "region" });
    expect(out.params.spec.filters).toEqual([
      { col: "region", op: "is_not_null" },
      { col: "region", op: "eq", value: "EU" },
    ]);
  });
  it("ignores params with no mapping for this card", () => {
    const params: DashboardParameter[] = [p({ id: "pr", slug: "region" })];
    const out = applyParamsToChart(chart(), params, { region: "EU" }, {});
    expect(out.params.spec.filters).toEqual([{ col: "region", op: "is_not_null" }]);
  });
});

describe("interpolateParams", () => {
  it("substitutes {{slug}} with the value, blanks when unset", () => {
    const params: DashboardParameter[] = [p({ slug: "region" })];
    expect(interpolateParams("Region: {{region}}", params, { region: "EU" })).toBe("Region: EU");
    expect(interpolateParams("Region: {{region}}", params, {})).toBe("Region: ");
    expect(interpolateParams("no tokens", params, {})).toBe("no tokens");
  });
});
