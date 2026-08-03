import { describe, it, expect } from "vitest";
import {
  TEMPORAL_UNITS, isExtractUnit, temporalLabel, pruneUnits, withDefaultTemporalUnits,
} from "../temporal-units";
import { buildPieOption } from "../echarts/options/pie";

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

  // The studio's monthly default, extracted here so the parity-harness seed script applies the
  // same rule instead of a copy. It used to live inline in ChartExplore's useEffect and had no
  // test of its own, which is how a harness that wrote `temporal_units: {}` went unnoticed.
  describe("withDefaultTemporalUnits", () => {
    const COLUMNS = [
      { column_name: "created_at", is_temporal: true },
      { column_name: "region", is_temporal: false },
    ];

    it("defaults a date dimension to monthly grouping", () => {
      expect(withDefaultTemporalUnits({}, ["created_at"], COLUMNS)).toEqual({ created_at: "month" });
    });

    it("leaves a non-temporal dimension alone", () => {
      expect(withDefaultTemporalUnits({}, ["region"], COLUMNS)).toEqual({});
    });

    it("never overwrites a unit the user already chose", () => {
      const chosen = { created_at: "day_of_week" };
      expect(withDefaultTemporalUnits(chosen, ["created_at"], COLUMNS)).toEqual(chosen);
    });

    it("returns the same object when nothing changed, so a state setter is a no-op", () => {
      const prev = { created_at: "month" };
      expect(withDefaultTemporalUnits(prev, ["created_at", "region"], COLUMNS)).toBe(prev);
    });

    it("ignores a dimension the dataset does not describe", () => {
      expect(withDefaultTemporalUnits({}, ["unknown"], COLUMNS)).toEqual({});
    });
  });

  it("pie slice names use the extraction label", () => {
    const opt = buildPieOption(
      [{ name: "dow", type: "int" }, { name: "n", type: "int" }],
      [[1, 10], [7, 3]],
      { dimensions: ["dow"], metric: "n", dimensionUnits: { dow: "day_of_week" } },
    ) as { series: { data: { name: string }[] }[] };
    const names = opt.series[0].data.map((d) => d.name);
    expect(names).toContain("Mon");
    expect(names).toContain("Sun");
  });
});
