import { describe, it, expect } from "vitest";
import { rowsToCsv, exportFilename } from "../export-chart";

const cols = [{ name: "region", type: "VARCHAR" }, { name: "rev", type: "DOUBLE" }];

describe("rowsToCsv", () => {
  it("emits a header row then data rows, CRLF-separated", () => {
    expect(rowsToCsv(cols, [["EU", 10], ["NA", 20]])).toBe('region,rev\r\nEU,10\r\nNA,20');
  });
  it("quotes cells containing comma, quote, or newline (doubling internal quotes)", () => {
    expect(rowsToCsv([{ name: "a", type: "x" }], [["x,y"], ['he"llo'], ["line\nbreak"]]))
      .toBe('a\r\n"x,y"\r\n"he""llo"\r\n"line\nbreak"');
  });
  it("renders null/undefined as empty and stringifies others", () => {
    expect(rowsToCsv(cols, [[null, undefined as unknown]])).toBe('region,rev\r\n,');
    expect(rowsToCsv([{ name: "n", type: "x" }], [[0], [false]])).toBe('n\r\n0\r\nfalse');
  });
  it("uses column_title in the header when set, alongside a column with none", () => {
    expect(
      rowsToCsv(cols, [["EU", 10]], { rev: { column_title: "Revenue" } }),
    ).toBe('region,Revenue\r\nEU,10');
  });
  it("falls back to the raw name for an empty-string column_title", () => {
    expect(
      rowsToCsv(cols, [["EU", 10]], { rev: { column_title: "" } }),
    ).toBe('region,rev\r\nEU,10');
  });
});

describe("exportFilename", () => {
  it("sanitises unsafe chars and appends the extension", () => {
    expect(exportFilename("Revenue / Region: 2026", "csv")).toBe("Revenue _ Region_ 2026.csv");
  });
  it("falls back to 'chart' for blank/undefined", () => {
    expect(exportFilename(undefined, "png")).toBe("chart.png");
    expect(exportFilename("   ", "csv")).toBe("chart.csv");
  });
});

describe("rowsToCsv — pivot rollup results", () => {
  // A rollup result carries a `__grp_N` marker per dimension plus, interleaved with the data, a
  // row per subtotal and the grand total. Exported raw, the file opened with `__grp_0` in the
  // header and a grand total as its FIRST row — indistinguishable from data unless the reader
  // knows to decode the markers.
  const cols = [
    { name: "region", type: "varchar" },
    { name: "revenue", type: "bigint" },
    { name: "__grp_0", type: "integer" },
  ];
  const rows = [
    [null, 30, 1],   // grand total
    ["EU", 10, 0],   // data
    ["NA", 20, 0],   // data
  ];

  it("drops the internal grouping-marker columns", () => {
    expect(rowsToCsv(cols, rows)).not.toContain("__grp_");
  });

  it("drops rollup rows, keeping only the leaves", () => {
    expect(rowsToCsv(cols, rows)).toBe("region,revenue\r\nEU,10\r\nNA,20");
  });

  it("leaves a non-rollup result untouched", () => {
    const plain = [{ name: "region", type: "varchar" }, { name: "rev", type: "bigint" }];
    expect(rowsToCsv(plain, [["EU", 10]])).toBe("region,rev\r\nEU,10");
  });

  it("emits just the header when every row was a rollup row", () => {
    expect(rowsToCsv(cols, [[null, 30, 1]])).toBe("region,revenue");
  });
});
