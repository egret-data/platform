import type { ResultColumn } from "@/components/chart/ChartTable";
import type { ColumnFormat } from "@/components/chart/format-types";

function csvCell(v: unknown): string {
  if (v === null || v === undefined) return "";
  const s = String(v);
  return /[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
}

/**
 * RFC-4180 CSV: a header row of column names, then one row per data row (CRLF-separated).
 * The header follows the same rename the table shows (`column_title`), so a downloaded
 * file reads the same as the screen it came from — the machine name is still one click
 * away via "View SQL". `||`, not `??`: settings are never key-whitelisted server-side, so
 * a legacy/hand-edited `column_title: ""` is reachable, and `??` would emit a blank header
 * cell instead of falling back to the real name.
 */
export function rowsToCsv(columns: ResultColumn[], rows: unknown[][], columnSettings?: Record<string, ColumnFormat>): string {
  const header = columns.map((c) => csvCell(columnSettings?.[c.name]?.column_title || c.name)).join(",");
  const body = rows.map((r) => r.map(csvCell).join(",")).join("\r\n");
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

export function downloadCsv(title: string | undefined, columns: ResultColumn[], rows: unknown[][], columnSettings?: Record<string, ColumnFormat>): void {
  downloadBlob(exportFilename(title, "csv"), new Blob([rowsToCsv(columns, rows, columnSettings)], { type: "text/csv;charset=utf-8" }));
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
