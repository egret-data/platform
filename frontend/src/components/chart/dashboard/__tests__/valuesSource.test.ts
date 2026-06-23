import { describe, it, expect } from "vitest";
import { resolveValuesSource } from "../valuesSource";
import type { DashboardParameter } from "@/api/dashboards";

const param = (over: Partial<DashboardParameter>): DashboardParameter =>
  ({ id: "p", name: "P", slug: "p", type: "category", operator: "is", ...over });

const cards = [
  { id: "c1", card_type: "chart", chart_id: "ch1", options: { paramMappings: { p: "region" } } },
  { id: "c2", card_type: "chart", chart_id: "ch2", options: {} },
] as never[];
const chartById = new Map<string, { dataset_id: string }>([["ch1", { dataset_id: "ds1" }], ["ch2", { dataset_id: "ds2" }]]);

describe("resolveValuesSource", () => {
  it("static → returns the static list", () => {
    expect(resolveValuesSource(param({ valuesSource: { type: "static", static: ["A", "B"] } }), cards, chartById))
      .toEqual({ kind: "static", values: ["A", "B"] });
  });
  it("distinct (default) → the mapped card's dataset + column", () => {
    expect(resolveValuesSource(param({}), cards, chartById)).toEqual({ kind: "query", datasetId: "ds1", column: "region" });
  });
  it("card → the chosen card's dataset + chosen column", () => {
    expect(resolveValuesSource(param({ valuesSource: { type: "card", cardId: "c2", column: "city" } }), cards, chartById))
      .toEqual({ kind: "query", datasetId: "ds2", column: "city" });
  });
  it("card with missing card → null", () => {
    expect(resolveValuesSource(param({ valuesSource: { type: "card", cardId: "nope", column: "x" } }), cards, chartById)).toBeNull();
  });
  it("card with missing cardId/column → null", () => {
    expect(resolveValuesSource(param({ valuesSource: { type: "card" } }), cards, chartById)).toBeNull();
  });
});
