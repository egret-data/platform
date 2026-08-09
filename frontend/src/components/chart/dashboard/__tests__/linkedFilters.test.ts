import { describe, it, expect } from "vitest";
import { linkedFilterState } from "../linkedFilters";
import type { DashboardParameter } from "@/api/dashboards";

const p = (over: Partial<DashboardParameter>): DashboardParameter =>
  ({ id: "1", name: "Region", slug: "region", type: "category", operator: "is", ...over });
const other = p({ id: "2", name: "City", slug: "city" });
const timeGrouping = p({ id: "3", name: "By", slug: "by", type: "temporal_unit" });

describe("linkedFilterState", () => {
  it("lists the other parameters that could constrain this one", () => {
    expect(linkedFilterState(p({}), [other])).toEqual({ kind: "list", usable: [other] });
  });

  it("reports no-usable when every other parameter is a time grouping", () => {
    // A temporal_unit param re-grids a column; it holds no value that could narrow a list.
    expect(linkedFilterState(p({}), [timeGrouping])).toEqual({ kind: "no-usable" });
    expect(linkedFilterState(p({}), [])).toEqual({ kind: "no-usable" });
  });

  it("reports custom-source when the values do not come from the data", () => {
    expect(linkedFilterState(p({ valuesSource: { type: "static", static: ["EU"] } }), [other]).kind).toBe("custom-source");
    expect(linkedFilterState(p({ valuesSource: { type: "card", cardId: "c", column: "x" } }), [other]).kind).toBe("custom-source");
    expect(linkedFilterState(p({ valuesSource: { type: "distinct" } }), [other]).kind).toBe("list");
  });

  it("reports input-widget for a free-text widget", () => {
    expect(linkedFilterState(p({ widget: "input" }), [other]).kind).toBe("input-widget");
    expect(linkedFilterState(p({ widget: "search" }), [other]).kind).toBe("list");
  });

  it("keeps Metabase's precedence: no-usable, then custom source, then widget", () => {
    // ParameterLinkedFilters.tsx:92-103 checks in exactly this order, and a parameter can be in
    // more than one of these states at once.
    const both = p({ widget: "input", valuesSource: { type: "static", static: [] } });
    expect(linkedFilterState(both, [other]).kind).toBe("custom-source");
    expect(linkedFilterState(both, [timeGrouping]).kind).toBe("no-usable");
  });
});
