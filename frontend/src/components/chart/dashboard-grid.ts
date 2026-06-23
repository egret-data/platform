// Mirrors Metabase's dashboard grid (frontend/src/metabase/utils/dashboard_grid.ts).
export const GRID_COLS = 24;             // GRID_WIDTH
export const GRID_ASPECT_RATIO = 10 / 9; // cell w:h
export const GRID_MIN_ROW_HEIGHT = 40;   // px (MIN_ROW_HEIGHT)
export const CARD_DEFAULT_SIZE = { w: 6, h: 4 };  // when a card is first added
export const CARD_MIN_SIZE = { w: 2, h: 2 };

/** rowHeight in px for a given container width (RGL needs a fixed px row height). */
export function gridRowHeight(containerWidth: number): number {
  return Math.max(Math.floor(containerWidth / GRID_COLS / GRID_ASPECT_RATIO), GRID_MIN_ROW_HEIGHT);
}

// Render-time vertical scale: stored geometry is unchanged on disk, but the grid renders at a
// finer rowHeight (30px) with ×3 the row units, so each stored row = 3 drag steps. A 6×4 stored
// card still renders at 360px (12 × 30). Persisted size_y/grid_row are divided back on save.
export const ROW_PX = 30;
export const ROW_SCALE = 3;

export function toLayoutRows(storedRows: number): number {
  return storedRows * ROW_SCALE;
}

export function fromLayoutRows(layoutRows: number): number {
  return Math.round(layoutRows / ROW_SCALE);
}

/** Stored rows needed to fit `px` of content (auto-height). One stored row = ROW_PX*ROW_SCALE px. */
export function contentRowsFor(px: number): number {
  return Math.max(1, Math.ceil(px / (ROW_PX * ROW_SCALE)));
}
