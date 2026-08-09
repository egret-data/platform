import { describe, it, expect, vi, beforeEach } from "vitest";

// The real writer zips through fflate, which reaches for a Worker. jsdom has none, and this file
// is not testing fflate — it is testing what WE hand the writer and what we do with its Blob.
const toBlob = vi.fn(() => Promise.resolve(new Blob(["xlsx"], { type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" })));
const writeXlsxFile = vi.fn(() => ({ toBlob }));
vi.mock("write-excel-file/browser", () => ({ default: (...a: unknown[]) => writeXlsxFile(...a) }));

const downloadBlob = vi.fn();
vi.mock("../export-chart", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../export-chart")>()),
  downloadBlob: (...a: unknown[]) => downloadBlob(...a),
}));

import { downloadXlsx, rowsToSheetData } from "../export-xlsx";

const cols = [{ name: "region", type: "VARCHAR" }, { name: "rev", type: "BIGINT" }];
const rows = [["EU", 1234], [null, 20]];

beforeEach(() => { writeXlsxFile.mockClear(); toBlob.mockClear(); downloadBlob.mockClear(); });

describe("rowsToSheetData", () => {
  it("puts the header first, then one row per data row", () => {
    expect(rowsToSheetData(cols, rows, undefined, false)).toEqual([
      ["region", "rev"],
      ["EU", 1234],
      [null, 20],
    ]);
  });

  it("keeps numbers typed when unformatted, so Excel can sum the column", () => {
    expect(typeof rowsToSheetData(cols, rows, undefined, false)[1][1]).toBe("number");
  });

  it("writes strings when formatted, which is what the toggle trades away", () => {
    expect(rowsToSheetData(cols, rows, undefined, true)[1]).toEqual(["EU", "1,234"]);
  });

  it("renames the header from column_title, like every other format", () => {
    expect(rowsToSheetData(cols, rows, { rev: { column_title: "Revenue" } }, true)[0])
      .toEqual(["region", "Revenue"]);
  });

  it("writes an empty cell as null and a non-finite number as null, never as a broken cell", () => {
    const odd = rowsToSheetData([{ name: "n", type: "BIGINT" }], [[null], [Number.NaN]], undefined, false);
    expect(odd[1]).toEqual([null]);
    expect(odd[2]).toEqual([null]);
  });

  it("drops pivot rollup markers and rows, via the shared shaper", () => {
    const pivotCols = [...cols, { name: "__grp_0", type: "INTEGER" }];
    const pivotRows = [[null, 30, 1], ["EU", 10, 0]];
    expect(rowsToSheetData(pivotCols, pivotRows, undefined, false)).toEqual([
      ["region", "rev"],
      ["EU", 10],
    ]);
  });
});

describe("downloadXlsx", () => {
  it("hands the sheet data to the writer and downloads its Blob under an .xlsx name", async () => {
    await downloadXlsx("My Chart", cols, rows, undefined, false);
    expect(writeXlsxFile).toHaveBeenCalledWith([["region", "rev"], ["EU", 1234], [null, 20]]);
    expect(toBlob).toHaveBeenCalledTimes(1);
    expect(downloadBlob).toHaveBeenCalledWith("My Chart.xlsx", expect.any(Blob));
  });
});
