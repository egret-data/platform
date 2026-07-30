import { describe, it, expect } from "vitest";
import { pivotCellDrill, drillFilters } from "../drill";

describe("drillFilters", () => {
  it("uses equality for a plain dimension", () => {
    expect(drillFilters({ dims: [{ col: "region", value: "EU" }] }))
      .toEqual([{ col: "region", op: "eq", value: "EU" }]);
  });

  it("turns a bucketed dimension into the bucket's half-open range", () => {
    // The live defect: a cell reading 2,084 for January drilled to a single row of 681, because
    // `= 2026-01-01` asks for one day while the cell covers the month. Silent under-reporting.
    expect(drillFilters({ dims: [{ col: "d", value: "2026-01-01", unit: "month" }] }))
      .toEqual([
        { col: "d", op: "ge", value: "2026-01-01" },
        { col: "d", op: "lt", value: "2026-02-01" },
      ]);
  });

  it("advances each truncation unit by its own length", () => {
    const end = (unit: string, from: string) => {
      const f = drillFilters({ dims: [{ col: "d", value: from, unit }] })[1];
      return "value" in f ? f.value : undefined;
    };
    expect(end("day", "2026-01-31")).toBe("2026-02-01");
    expect(end("week", "2026-01-01")).toBe("2026-01-08");
    expect(end("quarter", "2026-01-01")).toBe("2026-04-01");
    expect(end("year", "2026-01-01")).toBe("2027-01-01");
    // month arithmetic must roll the year over, not produce month 13
    expect(end("month", "2026-12-01")).toBe("2027-01-01");
  });

  it("drops an extraction unit rather than inventing a range", () => {
    // `day_of_week` is an integer field, not an instant. Filtering `= 3` would be a type error;
    // omitting it widens the result instead of hiding rows.
    expect(drillFilters({ dims: [{ col: "region", value: "EU" }, { col: "d", value: 3, unit: "day_of_week" }] }))
      .toEqual([{ col: "region", op: "eq", value: "EU" }]);
  });

  it("falls back to equality when the value is not a date at all", () => {
    expect(drillFilters({ dims: [{ col: "d", value: "not a date", unit: "month" }] }))
      .toEqual([{ col: "d", op: "eq", value: "not a date" }]);
  });
});

describe("pivotCellDrill", () => {
  it("tags a dimension with its temporal unit so the range can be built", () => {
    const ctx = pivotCellDrill(["region"], ["EU"], ["d"], ["2026-01-01"], { d: "month" });
    expect(ctx.dims).toEqual([
      { col: "region", value: "EU" },
      { col: "d", value: "2026-01-01", unit: "month" },
    ]);
  });

  it("leaves dimensions untagged when nothing is bucketed", () => {
    expect(pivotCellDrill(["region"], ["EU"], [], [], {}).dims).toEqual([{ col: "region", value: "EU" }]);
  });
});
