import { describe, it, expect } from "vitest";
import { computeLinkedFilters } from "../params";
import type { DashboardParameter } from "@/api/dashboards";

const param = (over: Partial<DashboardParameter>): DashboardParameter =>
  ({ id: "1", name: "City", slug: "city", type: "category", operator: "is", ...over });

describe("computeLinkedFilters", () => {
  const country = param({ id: "c", name: "Country", slug: "country", type: "location", operator: "is" });
  const city = param({ id: "1", filteringParameters: ["c"] });

  it("builds filters from a linked param's value using the same card's mapping", () => {
    const mappings = { c: "country_col", "1": "city_col" };
    const values = { country: ["US"] };
    expect(computeLinkedFilters(city, mappings, [city, country], values))
      .toEqual([{ col: "country_col", op: "in", value: ["US"] }]);
  });

  it("ignores a linked param with no value", () => {
    const mappings = { c: "country_col", "1": "city_col" };
    expect(computeLinkedFilters(city, mappings, [city, country], {})).toEqual([]);
  });

  it("ignores a linked param not mapped on this card", () => {
    const mappings = { "1": "city_col" }; // no mapping for 'c'
    const values = { country: ["US"] };
    expect(computeLinkedFilters(city, mappings, [city, country], values)).toEqual([]);
  });

  it("returns [] when there are no filteringParameters", () => {
    expect(computeLinkedFilters(param({ id: "1" }), {}, [], {})).toEqual([]);
  });
});
