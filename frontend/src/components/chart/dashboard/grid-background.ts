interface GridBackgroundOptions {
  cellWidth: number;
  rowHeight: number;
  cols: number;
  marginX: number;
  marginY: number;
  stroke: string;
}

/** A repeating SVG grid (one cell-rect per column, one row tall) as a CSS background url().
 *  Mirrors Metabase's generateGridBackground. `stroke` MUST be a CONCRETE color (e.g. the
 *  resolved value of `getComputedStyle(...).getPropertyValue("--border")`) — a CSS custom
 *  property like `var(--border)` does NOT resolve inside an SVG embedded as a data-URL
 *  background (the SVG is an isolated document) and renders nothing. */
export function generateGridBackground({ cellWidth, rowHeight, cols, marginX, marginY, stroke }: GridBackgroundOptions): string {
  const w = Math.round(cellWidth);
  const h = Math.round(rowHeight);
  const rects = Array.from({ length: cols }, (_, i) => {
    const x = Math.round(i * (cellWidth + marginX));
    return `<rect stroke='${stroke}' stroke-width='1' fill='none' x='${x}' y='0' width='${w}' height='${h}'/>`;
  }).join("");
  const totalWidth = Math.round(cols * cellWidth + (cols - 1) * marginX);
  const svg = `<svg xmlns='http://www.w3.org/2000/svg' width='${totalWidth}' height='${h + marginY}'>${rects}</svg>`;
  return `url("data:image/svg+xml;utf8,${encodeURIComponent(svg)}")`;
}
