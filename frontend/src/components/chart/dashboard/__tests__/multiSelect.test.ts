import { describe, it, expect } from "vitest";
import {
  SINGLE_OR_MULTI_SELECTABLE,
  canRenderMultiValue,
  isMultiValue,
  isSingleOrMultiSelectable,
} from "../multiSelect";
import type { DashboardParameter } from "@/api/dashboards";

const p = (over: Partial<DashboardParameter>): DashboardParameter =>
  ({ id: "1", name: "Region", slug: "region", type: "category", operator: "is", ...over });

describe("SINGLE_OR_MULTI_SELECTABLE", () => {
  it("carries exactly Metabase's pairs, under egret's operator spellings", () => {
    // Ported from metabase-lib/v1/parameters/constants.ts:243-259. Metabase's "string" is
    // egret's "text" and its "=" / "!=" subtypes are egret's "is" / "is-not"; every other
    // spelling is already identical.
    expect(SINGLE_OR_MULTI_SELECTABLE).toEqual({
      text: ["is", "is-not", "contains", "does-not-contain", "starts-with", "ends-with"],
      category: "any",
      id: "any",
      location: ["is", "is-not"],
      number: ["=", "!="],
    });
  });
});

describe("isSingleOrMultiSelectable", () => {
  it("is true for every listed pair and false for the types Metabase omits", () => {
    expect(isSingleOrMultiSelectable(p({ type: "category", operator: "is" }))).toBe(true);
    expect(isSingleOrMultiSelectable(p({ type: "id", operator: "is" }))).toBe(true);
    expect(isSingleOrMultiSelectable(p({ type: "location", operator: "is-not" }))).toBe(true);
    expect(isSingleOrMultiSelectable(p({ type: "text", operator: "contains" }))).toBe(true);
    expect(isSingleOrMultiSelectable(p({ type: "number", operator: "=" }))).toBe(true);
    // number/between is NOT in Metabase's list — the operator gate has to bite, not just the type.
    expect(isSingleOrMultiSelectable(p({ type: "number", operator: "between" }))).toBe(false);
    expect(isSingleOrMultiSelectable(p({ type: "date", operator: "range" }))).toBe(false);
    expect(isSingleOrMultiSelectable(p({ type: "boolean", operator: "is" }))).toBe(false);
    expect(isSingleOrMultiSelectable(p({ type: "temporal_unit", operator: "is" }))).toBe(false);
  });
});

describe("canRenderMultiValue", () => {
  it("is true only where egret has a list widget that can honour it", () => {
    expect(canRenderMultiValue(p({ type: "category" }))).toBe(true);
    expect(canRenderMultiValue(p({ type: "location", operator: "is" }))).toBe(true);
    expect(canRenderMultiValue(p({ type: "id", operator: "is" }))).toBe(true);
    // In Metabase's table but single-value inputs here — a radio would change nothing visible.
    expect(canRenderMultiValue(p({ type: "text", operator: "contains" }))).toBe(false);
    expect(canRenderMultiValue(p({ type: "number", operator: "=" }))).toBe(false);
    // The input-box widget is a free-text box, so multiplicity has nowhere to land.
    expect(canRenderMultiValue(p({ type: "category", widget: "input" }))).toBe(false);
    expect(canRenderMultiValue(p({ type: "category", widget: "search" }))).toBe(true);
  });
});

describe("isMultiValue", () => {
  it("defaults multi for the list types, single for id, and obeys an explicit choice", () => {
    expect(isMultiValue(p({ type: "category" }))).toBe(true);
    expect(isMultiValue(p({ type: "location", operator: "is" }))).toBe(true);
    // id stays single by default: every saved id filter is a scalar today, and flipping the
    // default would hand `paramToFilters` an array on load.
    expect(isMultiValue(p({ type: "id", operator: "is" }))).toBe(false);
    expect(isMultiValue(p({ type: "id", operator: "is", isMultiSelect: true }))).toBe(true);
    expect(isMultiValue(p({ type: "category", isMultiSelect: false }))).toBe(false);
    expect(isMultiValue(p({ type: "text", operator: "is" }))).toBe(false);
  });
});
