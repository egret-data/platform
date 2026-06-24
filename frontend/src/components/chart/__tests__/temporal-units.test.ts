import { describe, it, expect } from "vitest";
import { TEMPORAL_UNITS, isExtractUnit, temporalLabel, pruneUnits } from "../temporal-units";

describe("temporal-units", () => {
  it("lists all 15 Metabase tokens grouped trunc/extract", () => {
    expect(TEMPORAL_UNITS).toHaveLength(15);
    expect(TEMPORAL_UNITS.filter((u) => u.group === "trunc").map((u) => u.token)).toEqual([
      "minute", "hour", "day", "week", "month", "quarter", "year",
    ]);
    expect(TEMPORAL_UNITS.filter((u) => u.group === "extract")).toHaveLength(8);
  });

  it("classifies extraction units", () => {
    expect(isExtractUnit("month")).toBe(false);
    expect(isExtractUnit("day_of_week")).toBe(true);
    expect(isExtractUnit(undefined)).toBe(false);
  });

  it("maps extraction integers to friendly labels", () => {
    expect(temporalLabel("day_of_week", 1)).toBe("Mon");
    expect(temporalLabel("day_of_week", 7)).toBe("Sun");
    expect(temporalLabel("month_of_year", 1)).toBe("Jan");
    expect(temporalLabel("month_of_year", 12)).toBe("Dec");
    expect(temporalLabel("quarter_of_year", 2)).toBe("Q2");
    expect(temporalLabel("month", "2024-01-01")).toBe("2024-01-01");
    expect(temporalLabel("day_of_week", 9)).toBe("9");
    expect(temporalLabel(undefined, 5)).toBe("5");
  });

  it("prunes units to the selected dimension set", () => {
    expect(pruneUnits({ created_at: "month", region: "day" }, ["created_at"])).toEqual({
      created_at: "month",
    });
    expect(pruneUnits({ created_at: "month" }, [])).toEqual({});
  });
});
