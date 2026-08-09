/**
 * The colour the edit-mode guide grid is stroked with, as a CSS expression that
 * `DashboardGridLayout` resolves against the live theme by painting it onto a probe element and
 * reading the computed value back — a CSS custom property does not resolve inside an SVG embedded
 * as a data-URL background, so the SVG needs a concrete colour baked in.
 *
 * It is PLAIN `--border`, not a blend toward `--foreground`. The blend was added because `--border`
 * looked "nearly invisible" — but nearly invisible is the entire point of a grid guide. It is a
 * hint under the cards, not a table of boxes, and Metabase's measures 1.27:1 against its own canvas.
 *
 * Composited against `--dashboard` and computed from index.css:
 *
 *   | expression                                        | dark   | light  |
 *   |---------------------------------------------------|--------|--------|
 *   | color-mix(in oklch, --border 80%, --foreground)    | 2.42:1 | 1.77:1 |
 *   | var(--border)                                      | 1.27:1 | 1.11:1 |
 *
 * (§2.4 of the design first quoted 2.36:1 for dark. That was measured against `--background`
 * `#090909`; the grid paints on `--dashboard` `#0d0d0d`, which is 2.42:1.)
 *
 * A 90% blend was tried as a middle ground and rejected: `--border` is 10% white in the dark theme,
 * so mixing in 10% of an opaque `--foreground` nearly doubles the composited alpha (0.10 → 0.19)
 * and the dark contrast climbs back to 1.73:1. Plain `--border` is the only single expression that
 * clears the target in both themes. Pinned by __tests__/gridStrokeContrast.test.ts.
 */
export const GRID_STROKE_CSS = "var(--border)";

/** Used only when the browser resolves `GRID_STROKE_CSS` to the empty string — an SVG with no
 *  stroke draws nothing, so a visible neutral is a better failure than an invisible grid. */
export const GRID_STROKE_FALLBACK = "oklch(0.8 0 0)";

/** The band the composited stroke must land in, against the dashboard canvas, in both themes.
 *
 *  These bounds run the opposite way from every other contrast bound in this codebase. WCAG
 *  1.4.11's 3:1 floor governs information a user must be able to READ; an editing guide is
 *  deliberately not that, so the requirement is a CEILING. The floor is here only so that a future
 *  change cannot satisfy the ceiling by painting nothing at all. */
export const GRID_STROKE_MAX_CONTRAST = 1.5;
export const GRID_STROKE_MIN_CONTRAST = 1.05;
