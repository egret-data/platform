import { describe, it, expect } from "vitest";
import { nextParameterSlug, renameSlugReferences } from "../params";
import type { DashboardParameter } from "@/api/dashboards";

const p = (name: string, slug: string): DashboardParameter => ({ id: "x", name, slug, type: "text", operator: "is" });

// KI-065
describe("nextParameterSlug", () => {
  it("an unsaved filter whose slug came from its name follows a rename", () => {
    expect(nextParameterSlug(p("Filter", "filter_2"), p("Region", "filter_2"), ["filter"], false)).toBe("region");
  });
  it("a saved filter keeps its slug on a rename", () => {
    expect(nextParameterSlug(p("Region", "region"), p("Market", "region"), [], true)).toBe("region");
  });
  it("an unsaved filter whose slug was set by hand keeps it on a rename", () => {
    expect(nextParameterSlug(p("Filter", "geo"), p("Region", "geo"), [], false)).toBe("geo");
  });
  it("an explicit slug is made URL-safe and unique", () => {
    expect(nextParameterSlug(p("Region", "region"), p("Region", "Sales Market!"), [], true)).toBe("sales_market");
    expect(nextParameterSlug(p("Region", "region"), p("Region", "status"), ["status"], true)).toBe("status_2");
  });
});

describe("renameSlugReferences", () => {
  it("rewrites {{from}} in every string, nested, and leaves longer slugs alone", () => {
    const options = { text: "In {{region}} and {{ region }} not {{region_2}}", clickBehavior: { type: "url", template: "https://x/?r={{region}}" }, n: 3 };
    expect(renameSlugReferences(options, "region", "market")).toEqual({
      text: "In {{market}} and {{market}} not {{region_2}}",
      clickBehavior: { type: "url", template: "https://x/?r={{market}}" },
      n: 3,
    });
  });
});
