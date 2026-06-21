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
