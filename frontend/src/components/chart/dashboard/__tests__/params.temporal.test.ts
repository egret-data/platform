import { describe, it, expect } from "vitest";
import { applyParamsToChart, summarize } from "../params";
import type { DashboardParameter } from "@/api/dashboards";
import type { Chart } from "@/api/charts";

const tuParam = (over: Partial<DashboardParameter> = {}): DashboardParameter => ({
  id: "p1", name: "Date grouping", slug: "date_grouping", type: "temporal_unit",
  operator: "is", default: "month", ...over,
});

const chart = (temporal_units: Record<string, string>): Chart => ({
  id: "c", dataset_id: "d", name: "C", description: "", created_by: null, updated_at: "",
  viz_type: "bar",
  params: { spec: { metrics: ["rev"], dimensions: ["created_at"], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [], temporal_units } },
});

describe("applyParamsToChart — temporal_unit", () => {
  it("overrides the unit of an already-grouped column", () => {
    const out = applyParamsToChart(chart({ created_at: "month" }), [tuParam()], { date_grouping: "quarter" }, { p1: "created_at" });
    expect(out.params.spec.temporal_units).toEqual({ created_at: "quarter" });
  });

  it("is a no-op when the mapped column is not already grouped", () => {
    const c = chart({});
    const out = applyParamsToChart(c, [tuParam()], { date_grouping: "quarter" }, { p1: "created_at" });
    expect(out).toBe(c);
  });

  it("falls back to the param default when the value is blank", () => {
    const out = applyParamsToChart(chart({ created_at: "month" }), [tuParam({ default: "year" })], { date_grouping: "" }, { p1: "created_at" });
    expect(out.params.spec.temporal_units).toEqual({ created_at: "year" });
  });

  it("does nothing when the param is not mapped", () => {
    const c = chart({ created_at: "month" });
    const out = applyParamsToChart(c, [tuParam()], { date_grouping: "quarter" }, {});
    expect(out).toBe(c);
  });
});

describe("summarize — temporal_unit", () => {
  it("shows the unit label", () => {
    expect(summarize(tuParam(), "month")).toBe("Month");
    expect(summarize(tuParam(), "day_of_week")).toBe("Day of week");
  });
});
