import type { ResultColumn } from "@/components/chart/ChartTable";
import type { ColumnFormat } from "@/components/chart/format-types";
import { downloadBlob, exportCell, exportFilename, tableForExport } from "./export-chart";

/**
 * One `.xlsx` cell as `write-excel-file` wants it: a bare value, or `null` for an empty cell.
 *
 * Spelled out here rather than imported from the library. An `import type` is erased and would be
 * safe, but this file's whole contract is that the library's name appears in exactly ONE place —
 * the dynamic import in `downloadXlsx` — and a second mention is a second thing for a future reader
 * to "tidy up" into a static import.
 */
export type XlsxCell = string | number | boolean | Date | null;
export type XlsxSheetData = XlsxCell[][];

/** `write-excel-file` derives a cell's type from its value. Anything it cannot type — an object, a
 *  bigint, a NaN — is written as an empty cell rather than as `[object Object]` or a `#NUM!`. */
function toXlsxCell(v: unknown): XlsxCell {
  if (v === null || v === undefined) return null;
  if (typeof v === "number") return Number.isFinite(v) ? v : null;
  if (typeof v === "string" || typeof v === "boolean") return v;
  if (v instanceof Date) return v;
  return String(v);
}

/**
 * Project a result onto sheet rows: a header row, then one row per data row.
 *
 * WITH `formatted` OFF numbers stay numbers, so Excel sums the column. WITH IT ON every cell is the
 * string the screen shows — which is the point of the toggle and also the reason the column stops
 * being summable. Metabase makes exactly this trade (`enableFormatting` picks a string writer over
 * a typed one); the toggle is how the reader chooses which file they wanted.
 */
export function rowsToSheetData(
  columns: ResultColumn[],
  rows: unknown[][],
  columnSettings?: Record<string, ColumnFormat>,
  formatted = true,
): XlsxSheetData {
  const t = tableForExport(columns, rows, columnSettings, formatted);
  return [
    t.header.map((h): XlsxCell => h),
    ...t.rows.map((r) => r.map((v, i) => toXlsxCell(exportCell(v, t.columns[i], columnSettings, formatted)))),
  ];
}

/**
 * Download the result as `.xlsx`.
 *
 * THE DYNAMIC IMPORT IS THE POINT, and it is load-bearing rather than tidy. `write-excel-file` plus
 * `fflate` is the only dependency this workstream adds, and no dashboard needs a byte of it until
 * someone picks `.xlsx` in the download popover — so it must land in its own chunk.
 *
 * `frontend/scripts/assert-xlsx-lazy.mjs` asserts that against `dist/`, because no unit test can:
 * NEITHER VITEST NOR `vite dev` TREE-SHAKES OR CODE-SPLITS, so a green test says nothing about
 * which chunk this ended up in. This repo has already shipped a library that passed every test and
 * was absent from the bundle — see `scripts/assert-label-layout-bundled.mjs`.
 */
export async function downloadXlsx(
  title: string | undefined,
  columns: ResultColumn[],
  rows: unknown[][],
  columnSettings?: Record<string, ColumnFormat>,
  formatted = true,
): Promise<void> {
  const { default: writeXlsxFile } = await import("write-excel-file/browser");
  const blob = await writeXlsxFile(rowsToSheetData(columns, rows, columnSettings, formatted)).toBlob();
  downloadBlob(exportFilename(title, "xlsx"), blob);
}
