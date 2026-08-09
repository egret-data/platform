import { describe, it, expect } from "vitest";
import { rowsToCsv, rowsToJson, tableForExport } from "../export-chart";

const cols = [{ name: "region", type: "VARCHAR" }, { name: "rev", type: "BIGINT" }];
const rows = [["EU", 1234], ["NA", 20]];

describe("tableForExport", () => {
  it("resolves display headers when formatted and machine names when not", () => {
    const settings = { rev: { column_title: "Revenue" } };
    expect(tableForExport(cols, rows, settings, true).header).toEqual(["region", "Revenue"]);
    expect(tableForExport(cols, rows, settings, false).header).toEqual(["region", "rev"]);
  });

  it("strips pivot rollup marker columns and rollup rows for every format, not just CSV", () => {
    // This used to live inside rowsToCsv. A second writer that forgot it would export the grand
    // total as its first row — which is the defect the strip was added for in the first place.
    const pivotCols = [...cols, { name: "__grp_0", type: "INTEGER" }];
    const pivotRows = [[null, 30, 1], ["EU", 10, 0], ["NA", 20, 0]];
    const t = tableForExport(pivotCols, pivotRows, undefined, true);
    expect(t.header).toEqual(["region", "rev"]);
    expect(t.columns.map((c) => c.name)).toEqual(["region", "rev"]);
    expect(t.rows).toEqual([["EU", 10], ["NA", 20]]);
  });

  it("keeps the raw cells — formatting is the writer's job, not the shaper's", () => {
    expect(tableForExport(cols, rows, undefined, true).rows).toEqual([["EU", 1234], ["NA", 20]]);
  });
});

describe("rowsToCsv formatted flag", () => {
  it("formats cells when on — grouping separators and all", () => {
    // formatNumber builds with useGrouping: true (format.ts:105), so a formatted BIGINT gains a
    // comma and therefore gains CSV quoting. This is Metabase's behaviour and the whole reason
    // the toggle exists; it is pinned here so nobody quietly reverts it.
    expect(rowsToCsv(cols, rows)).toBe('region,rev\r\nEU,"1,234"\r\nNA,20');
  });

  it("emits the database's bytes when off, machine header included", () => {
    expect(rowsToCsv(cols, rows, { rev: { column_title: "Revenue" } }, false))
      .toBe("region,rev\r\nEU,1234\r\nNA,20");
  });
});

describe("rowsToJson", () => {
  it("emits one object per row keyed by the resolved header", () => {
    expect(JSON.parse(rowsToJson(cols, rows, { rev: { column_title: "Revenue" } }, false)))
      .toEqual([{ region: "EU", rev: 1234 }, { region: "NA", rev: 20 }]);
  });

  it("keys on display names and carries formatted strings when formatted", () => {
    expect(JSON.parse(rowsToJson(cols, rows, { rev: { column_title: "Revenue" } }, true)))
      .toEqual([{ region: "EU", Revenue: "1,234" }, { region: "NA", Revenue: "20" }]);
  });

  it("is pretty-printed, so a downloaded file is readable", () => {
    expect(rowsToJson(cols, [["EU", 1]], undefined, false)).toContain("\n  {\n");
  });
});
