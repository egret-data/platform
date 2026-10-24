import type { ResultColumn } from "@/components/chart/ChartTable";
import type { ColumnFormat } from "@/components/chart/format-types";
import { formatValue, withIdentifierDefault } from "@/components/chart/format";

function csvCell(v: unknown): string {
  if (v === null || v === undefined) return "";
  const s = String(v);
  return /[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
}

/**
 * A RESULT, SHAPED FOR A FILE — the one place pivot rollup markers die and the one place a header
 * is resolved. Three writers share it (`csv`, `json`, `xlsx`).
 *
 * A rollup result carries one `__grp_N` marker per dimension and, interleaved with the data, a row
 * for every subtotal and the grand total. Those are how the pivot builds its bands — they are not
 * columns or rows anyone asked for. Exported raw, the file opened with `__grp_0,__grp_1,__grp_2` in
 * the header and a first row of `,,,116269,1,1,1`: a total, indistinguishable from data unless the
 * reader knows to decode the markers. When this lived inside `rowsToCsv` the strip was a property
 * of the CSV writer; the moment a second writer existed it had to become a property of the export.
 *
 * `formatted` is Metabase's "Keep the data formatted" (`ExportSettingsWidget.tsx:99`). On, the file
 * reads like the screen — the rename the table shows, and `formatValue`'d cells. Off, it reads like
 * the database — machine column names and raw values. egret's CSV used to be neither: renamed
 * headers over raw cells. That hybrid is gone.
 *
 * `||`, not `??`, on the rename: settings are never key-whitelisted server-side, so a
 * legacy/hand-edited `column_title: ""` is reachable, and `??` would emit a blank header cell
 * instead of falling back to the real name.
 */
export interface ExportTable {
  /** The header as it should be written, one entry per kept column. */
  header: string[];
  /** The kept columns themselves — a writer needs `type` to format or to type a cell. */
  columns: ResultColumn[];
  /** Data rows, markers and rollup rows removed, cells still raw. */
  rows: unknown[][];
}

export function tableForExport(
  columns: ResultColumn[],
  rows: unknown[][],
  columnSettings?: Record<string, ColumnFormat>,
  formatted = true,
): ExportTable {
  const markerIdx = columns.map((c, i) => (/^__grp_\d+$/.test(c.name) ? i : -1)).filter((i) => i >= 0);
  const keep = columns.map((_, i) => i).filter((i) => !markerIdx.includes(i));
  const isRollupRow = (r: unknown[]) => markerIdx.some((i) => Number(r[i]) === 1);
  const kept = keep.map((i) => columns[i]);
  return {
    header: formatted ? kept.map((c) => columnSettings?.[c.name]?.column_title || c.name) : kept.map((c) => c.name),
    columns: kept,
    rows: rows.filter((r) => !isRollupRow(r)).map((r) => keep.map((i) => r[i])),
  };
}

/** One cell as the file should carry it: the string the screen shows when formatted, the raw value
 *  otherwise. Only `.xlsx` keeps the raw type — `.csv` stringifies and `.json` re-serialises. */
export function exportCell(
  value: unknown,
  column: ResultColumn,
  columnSettings: Record<string, ColumnFormat> | undefined,
  formatted: boolean,
): unknown {
  return formatted
    ? formatValue(value, column.type, withIdentifierDefault(column.name, column.type, columnSettings?.[column.name] ?? {}))
    : value;
}

/** RFC-4180 CSV: a header row, then one row per data row, CRLF-separated. */
export function rowsToCsv(
  columns: ResultColumn[],
  rows: unknown[][],
  columnSettings?: Record<string, ColumnFormat>,
  formatted = true,
): string {
  const t = tableForExport(columns, rows, columnSettings, formatted);
  const header = t.header.map(csvCell).join(",");
  const body = t.rows
    .map((r) => r.map((v, i) => csvCell(exportCell(v, t.columns[i], columnSettings, formatted))).join(","))
    .join("\r\n");
  return body ? `${header}\r\n${body}` : header;
}

/** Sanitise a title into a safe filename stem (fallback "chart"); append `.ext`. */
export function exportFilename(title: string | undefined, ext: string): string {
  const stem = (title ?? "").replace(/[\\/:*?"<>|]/g, "_").trim() || "chart";
  return `${stem}.${ext}`;
}

/** Trigger a client-side download of a Blob. */
export function downloadBlob(filename: string, blob: Blob): void {
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  a.remove();
  URL.revokeObjectURL(url);
}

export function downloadCsv(title: string | undefined, columns: ResultColumn[], rows: unknown[][], columnSettings?: Record<string, ColumnFormat>, formatted = true): void {
  downloadBlob(exportFilename(title, "csv"), new Blob([rowsToCsv(columns, rows, columnSettings, formatted)], { type: "text/csv;charset=utf-8" }));
}

/**
 * THE MARK AN IMAGE-EXPORTABLE CHART SURFACE CARRIES, and the reason `.png` asks for it rather than
 * for "the first <svg> in the card". Every icon in this app is an inline <svg> too: the object
 * detail's pager arrows, a trend's arrow, a menu chevron. Asked for any <svg>, the exporter handed
 * the Object detail's 32x32 pager arrow to the rasteriser and downloaded a 286-byte picture of it.
 * `EChartsRenderer` is the one surface that draws a whole chart into ONE <svg>, so it is the one
 * that carries this; a card without it (a table, an object detail, a progress bar, a trapezoid
 * funnel drawn as one <svg> per band) has no single image to export and is not offered `.png` —
 * Metabase's `canSavePng` likewise leaves out table, pivot and object.
 */
export const CHART_IMAGE_ATTR = "data-chart-image";

/** The chart image inside `container`, or null when the card did not draw one. */
export function chartImageSvg(container: ParentNode | null | undefined): SVGElement | null {
  return container?.querySelector<SVGElement>(`[${CHART_IMAGE_ATTR}] svg`) ?? null;
}

/** Rasterise an <svg> element to a PNG Blob via Image → canvas. ECharts SVG has no foreignObject, so no taint. */
export function svgToPngBlob(svg: SVGElement, scale = 2): Promise<Blob> {
  return new Promise((resolve, reject) => {
    const rect = svg.getBoundingClientRect();
    const w = Math.max(1, Math.round((rect.width || 600) * scale));
    const h = Math.max(1, Math.round((rect.height || 400) * scale));
    const xml = new XMLSerializer().serializeToString(svg);
    const svgUrl = URL.createObjectURL(new Blob([xml], { type: "image/svg+xml;charset=utf-8" }));
    const img = new Image();
    const cleanup = () => URL.revokeObjectURL(svgUrl);
    img.onload = () => {
      cleanup();
      const canvas = document.createElement("canvas");
      canvas.width = w;
      canvas.height = h;
      const ctx = canvas.getContext("2d");
      if (!ctx) { reject(new Error("no 2d context")); return; }
      ctx.fillStyle = "#ffffff";
      ctx.fillRect(0, 0, w, h);
      ctx.drawImage(img, 0, 0, w, h);
      canvas.toBlob((b) => (b ? resolve(b) : reject(new Error("toBlob failed"))), "image/png");
    };
    img.onerror = () => { cleanup(); reject(new Error("svg image load failed")); };
    img.src = svgUrl;
  });
}

export async function downloadPng(title: string | undefined, svg: SVGElement): Promise<void> {
  downloadBlob(exportFilename(title, "png"), await svgToPngBlob(svg));
}

/**
 * One JSON object per row, keyed by the resolved header — the shape Metabase's `.json` export has.
 *
 * TWO COLUMNS THAT RESOLVE TO THE SAME HEADER COLLIDE, and the last one wins. Reachable only by
 * renaming two columns to the same title, which is also true of Metabase's export, and the
 * alternative (suffixing duplicates) invents a key the reader never typed.
 */
export function rowsToJson(columns: ResultColumn[], rows: unknown[][], columnSettings?: Record<string, ColumnFormat>, formatted = true): string {
  const t = tableForExport(columns, rows, columnSettings, formatted);
  const objects = t.rows.map((r) =>
    Object.fromEntries(r.map((v, i) => [t.header[i], exportCell(v, t.columns[i], columnSettings, formatted)])),
  );
  return JSON.stringify(objects, null, 2);
}

export function downloadJson(title: string | undefined, columns: ResultColumn[], rows: unknown[][], columnSettings?: Record<string, ColumnFormat>, formatted = true): void {
  downloadBlob(exportFilename(title, "json"), new Blob([rowsToJson(columns, rows, columnSettings, formatted)], { type: "application/json;charset=utf-8" }));
}
