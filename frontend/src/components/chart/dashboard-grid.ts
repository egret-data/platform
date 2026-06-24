// Faithful port of Metabase's dashboard grid math (frontend/src/metabase/lib/dashboard_grid.js
// + DashboardGrid.getRowHeight + dashboards/constants.cljc). Clean-room reimplementation.
export const GRID_COLS = 24;             // GRID_WIDTH
export const GRID_ASPECT_RATIO = 10 / 9; // cell w:h
export const GRID_MIN_ROW_HEIGHT = 40;   // px (MIN_ROW_HEIGHT)
export const GRID_MARGIN: [number, number] = [6, 6];   // [horizontal, vertical] gap between cards
export const GRID_PADDING: [number, number] = [0, 0];  // container padding
export const CARD_MIN_SIZE = { w: 2, h: 2 };

/** rowHeight in px: each cell is GRID_ASPECT_RATIO wider than tall, clamped to the 40px floor. */
export function gridRowHeight(containerWidth: number): number {
  return Math.max(Math.floor(containerWidth / GRID_COLS / GRID_ASPECT_RATIO), GRID_MIN_ROW_HEIGHT);
}

/** Pixel width of one grid column after removing the inter-column margins. */
export function cellWidth(containerWidth: number): number {
  return (containerWidth - (GRID_COLS - 1) * GRID_MARGIN[0]) / GRID_COLS;
}

/** Stored rows needed to fit `px` of content (auto-height). A card of n rows occupies
 *  n*rowHeight + (n-1)*marginY px, so the row pitch is rowHeight + marginY. */
export function contentRowsFor(px: number, rowHeight: number): number {
  return Math.max(1, Math.ceil(px / (rowHeight + GRID_MARGIN[1])));
}

interface GridRect { grid_col: number; grid_row: number; size_x: number; size_y: number }

function intersects(a: GridRect, b: GridRect): boolean {
  return !(
    b.grid_col >= a.grid_col + a.size_x ||
    b.grid_col + b.size_x <= a.grid_col ||
    b.grid_row >= a.grid_row + a.size_y ||
    b.grid_row + b.size_y <= a.grid_row
  );
}

/** First free slot scanning left→right, top→bottom (Metabase getPositionForNewDashCard). */
export function getPositionForNewDashCard(
  cards: GridRect[],
  size_x: number,
  size_y: number,
  width: number = GRID_COLS,
): { grid_col: number; grid_row: number } {
  for (let row = 0; row < 1000; row++) {
    for (let col = 0; col <= width - size_x; col++) {
      const candidate: GridRect = { grid_col: col, grid_row: row, size_x, size_y };
      if (!cards.some((c) => intersects(c, candidate))) return { grid_col: col, grid_row: row };
    }
  }
  return { grid_col: 0, grid_row: 0 };
}

interface CardSize { default: { w: number; h: number }; min: { w: number; h: number } }
const GENERIC_CHART: CardSize = { default: { w: 12, h: 6 }, min: { w: 4, h: 3 } };

/** Per-viz default/min sizes, ported from Metabase card-size-defaults (keyed by egret viz/card keys). */
export const CARD_SIZES_BY_VIZ: Record<string, CardSize> = {
  table:       { default: { w: 12, h: 9 }, min: { w: 4, h: 3 } },
  pivot_table: { default: { w: 12, h: 9 }, min: { w: 4, h: 3 } },
  bar:         { default: { w: 12, h: 6 }, min: { w: 4, h: 3 } },
  row:         { default: { w: 12, h: 6 }, min: { w: 4, h: 3 } },
  line:        { default: { w: 12, h: 6 }, min: { w: 4, h: 3 } },
  area:        { default: { w: 12, h: 6 }, min: { w: 4, h: 3 } },
  scatter:     { default: { w: 12, h: 6 }, min: { w: 4, h: 3 } },
  combo:       { default: { w: 12, h: 6 }, min: { w: 4, h: 3 } },
  pie:         { default: { w: 12, h: 8 }, min: { w: 4, h: 3 } },
  progress:    { default: { w: 12, h: 6 }, min: { w: 4, h: 3 } },
  gauge:       { default: { w: 12, h: 6 }, min: { w: 4, h: 3 } },
  funnel:      { default: { w: 12, h: 6 }, min: { w: 4, h: 3 } },
  waterfall:   { default: { w: 14, h: 6 }, min: { w: 4, h: 3 } },
  sankey:      { default: { w: 16, h: 10 }, min: { w: 4, h: 3 } },
  map:         { default: { w: 16, h: 10 }, min: { w: 6, h: 4 } },
  scalar:      { default: { w: 6, h: 3 }, min: { w: 2, h: 2 } },
  smartscalar: { default: { w: 6, h: 3 }, min: { w: 2, h: 2 } },
  text:        { default: { w: 12, h: 3 }, min: { w: 1, h: 1 } },
  heading:     { default: { w: 24, h: 1 }, min: { w: 1, h: 1 } },
  link:        { default: { w: 8, h: 1 }, min: { w: 1, h: 1 } },
};

/** Default + min grid size for a viz/card key, with a generic-chart fallback (e.g. boxplot). */
export function sizeForViz(key: string | null | undefined): CardSize {
  return (key && CARD_SIZES_BY_VIZ[key]) || GENERIC_CHART;
}
