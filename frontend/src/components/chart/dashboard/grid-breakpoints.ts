import type { LayoutItem } from "react-grid-layout";

/** Ported from Metabase utils/dashboard_grid.ts:11-26 and grid/utils.ts:21-36.
 *  The two breakpoint numbers are adjacent (752 / 753) because react-grid-layout picks the
 *  largest breakpoint whose value is <= the container width; a gap between them would leave
 *  widths that match neither. */
export const GRID_BREAKPOINTS = { desktop: 753, mobile: 752 } as const;
export const GRID_COLUMNS = { desktop: 24, mobile: 1 } as const;
export const GRID_MARGIN_BY_BREAKPOINT = {
  desktop: [6, 6] as [number, number],
  mobile: [6, 10] as [number, number],
};
export const MOBILE_DEFAULT_CARD_HEIGHT = 6;

export type GridBreakpoint = keyof typeof GRID_COLUMNS;

/** The edit panel (340px) and the 16px gap before it: what it takes from the canvas (KI-282). */
export const EDIT_PANEL_BESIDE_PX = 356;

/** Which breakpoint a container of `width` px is in. Container width, NOT viewport width —
 *  the dashboard canvas is narrowed by the app sidebar and by the edit sidebars, and it is the
 *  canvas that has to fit the cards. */
export function breakpointFor(width: number): GridBreakpoint {
  return width >= GRID_BREAKPOINTS.desktop ? "desktop" : "mobile";
}

/** Mobile row count for a card, keyed on its viz type (chart cards) or card type
 *  (text/heading/link). Mirrors MOBILE_HEIGHT_BY_DISPLAY_TYPE in
 *  visualizations/shared/utils/sizes.ts:36-58. */
export function mobileHeightFor(kind: string, desktopHeight: number): number {
  switch (kind) {
    case "link":
      return 1;
    case "heading":
      return 2;
    case "scalar":
    case "smartscalar":
      return 4;
    // A text card is the one kind whose desktop height is authored content rather than a
    // chart's aspect, so shrinking it would clip what the author wrote.
    case "text":
      return Math.max(2, desktopHeight);
    default:
      return MOBILE_DEFAULT_CARD_HEIGHT;
  }
}

/** Project a desktop layout onto the one-column mobile grid: every card full width, stacked in
 *  reading order (top-to-bottom, then left-to-right), heights by kind.
 *
 *  `kindOf` maps a layout id to the string `mobileHeightFor` keys on. The min/max constraints
 *  are deliberately dropped: they were computed for a 24-column grid and would make RGL clamp
 *  every card back above one column. */
export function toMobileLayout(layout: LayoutItem[], kindOf: (id: string) => string): LayoutItem[] {
  const ordered = [...layout].sort((a, b) => a.y - b.y || a.x - b.x);
  let y = 0;
  return ordered.map((l) => {
    const h = mobileHeightFor(kindOf(l.i), l.h);
    const next: LayoutItem = { i: l.i, x: 0, y, w: 1, h, minW: 1, minH: 1 };
    y += h;
    return next;
  });
}

/** May a layout or auto-height change be written into the draft?
 *
 *  Both halves are load-bearing. `editing` alone is not enough: react-grid-layout emits an
 *  onLayoutChange for the DERIVED one-column layout as soon as it mounts on a narrow screen,
 *  so an editing user on a phone would silently rewrite every card to x:0 w:1 and a Save would
 *  destroy the desktop layout. */
export function mayPersistLayout(editing: boolean, breakpoint: GridBreakpoint): boolean {
  return editing && breakpoint === "desktop";
}

/** How far the pointer may travel between press and click for it to still be a click. */
const CLICK_SLOP_PX = 4;

/** Did the pointer travel between the press and this click? Then the click is the end of a drag
 *  (a card moved by its handle) and must not act as a click on the card. */
export function movedSincePress(press: { x: number; y: number } | null, click: { clientX: number; clientY: number }): boolean {
  if (!press) return false;
  return Math.abs(click.clientX - press.x) > CLICK_SLOP_PX || Math.abs(click.clientY - press.y) > CLICK_SLOP_PX;
}
