import type { DOMAttributes } from "react";

/**
 * The two controls that live in a table's column header — resize and sort — shared by `ChartTable`
 * and `pivot/PivotTable`.
 *
 * WHY THIS FILE EXISTS AT ALL. `ResizeHandle` was written for the pivot in chart 03, complete with
 * a focusable separator and arrow-key resizing, and `ChartTable`'s separator was simply left
 * behind: `role="separator"`, `aria-label="Resize column"`, `onMouseDown`/`onTouchStart`, and
 * measured `tabindex: null`. It advertised itself to assistive tech as a control and could not be
 * operated. Copying the pivot's version across would have made two implementations of one control,
 * which is exactly how this programme ended up with four per-series settings across three parallel
 * lists — so it moved here instead and the pivot imports it back.
 *
 * The sort control is new to both. `ChartTable`'s header answered Enter with a dnd-kit lift and
 * sorted only on `onClick`, so sorting had no keyboard route; the pivot's row-dimension header had
 * no `tabIndex` at all, so it could not even be focused. Both now render a real `<button>`, which
 * is the WAI-ARIA sortable-table pattern and the one shape a screen reader announces as operable
 * without being told.
 */

/**
 * THE FOCUS INDICATOR FOR THE CONTROLS ON THIS PAGE, and it deliberately does not use `--ring`.
 *
 * WCAG 1.4.11 / 2.4.11 ask 3:1 of a non-text focus indicator against what is around it — NOT the
 * 4.5:1 text bar, and saying which bar is being met matters more than the number. Converting the
 * tokens from OKLCH to sRGB numerically and compositing the alpha (Ottosson's oklab→linear-sRGB,
 * not a regex reading `oklch()`'s three numbers as RGB) gives, against `--background`:
 *
 *   light `--ring` solid   2.58:1  FAIL      dark `--ring` solid   4.08:1  pass
 *   light `ring-ring/50`   1.54:1  FAIL      dark `ring-ring/50`   1.83:1  FAIL
 *
 * THOSE NUMBERS ARE HISTORY NOW — they were the state when these two controls were introduced, and
 * they are kept here because they are why this file chose `--primary`. The product-wide fix landed
 * in `docs/en/plans/2026-08-05-focus-indicator-contrast.md`: light `--ring` was darkened from
 * `oklch(0.71 …)` to `oklch(0.63 …)` and the half-alpha halos were made solid, so `--ring` now
 * measures 3.51:1 on `--background` in light and clears 3:1 on every surface in both themes.
 *
 * These controls still use `--primary` anyway, and the reason was never only contrast: `outline`,
 * not `ring`, so the indicator is not clipped by the `overflow: auto` scroller both tables sit in —
 * a `ring` is a box-shadow and box-shadows get cut off by the scroll container. `--primary`
 * measures 3.29:1 on `--accent` (the header cell's own background) and 3.58:1 on `--background` in
 * light, 5.26 / 6.93 in dark. Switching these to `--ring` would be a behaviour change with its own
 * review, not a tidy-up, so the contrast PR deliberately left them alone.
 */
const FOCUS_RING =
  "focus-visible:outline-2 focus-visible:outline-offset-[-2px] focus-visible:outline-primary";

/** Below this a column is unreadable; both tables clamp to it, dragging and from the keyboard. */
export const MIN_COL_WIDTH = 60;
/** One arrow-key press. Chart 03 established that a mouse-only column interaction is a defect. */
export const KEY_RESIZE_STEP = 16;

/** `aria-sort`'s vocabulary, from the two words the rest of the code passes around. */
export function ariaSort(dir: "asc" | "desc" | undefined): "ascending" | "descending" | "none" {
  return dir === "asc" ? "ascending" : dir === "desc" ? "descending" : "none";
}

export function ResizeHandle({ label, onResize, getWidth, dragHandlers }: {
  /** Names THIS column. `"Resize column"` on every handle is N identical controls to a screen
   * reader, with nothing to tell them apart. */
  label: string;
  onResize: (width: number) => void;
  /** Measured from the DOM at gesture start — the stored width may be absent (auto-sized). */
  getWidth: (el: HTMLElement) => number;
  /**
   * A table that owns its own drag passes it here; the handle then runs no pointer loop of its own.
   * `ChartTable` does, because tanstack's `getResizeHandler` is what drives `columnSizingInfo`, and
   * that is what its persist-on-gesture-end effect watches — replacing it with the loop below would
   * have made every drag either silently unsaved or saved on every pointermove.
   */
  dragHandlers?: Pick<DOMAttributes<HTMLElement>, "onMouseDown" | "onTouchStart">;
}) {
  return (
    <span
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      tabIndex={0}
      // `focus-visible:bg-primary` SOLID, where the pivot's original said `/60`. See `FOCUS_RING`.
      className={`absolute right-0 top-0 h-full w-1.5 cursor-col-resize touch-none select-none hover:bg-primary/40 focus-visible:bg-primary ${FOCUS_RING}`}
      {...dragHandlers}
      onKeyDown={(e) => {
        if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
        e.preventDefault();
        // A header may be a dnd-kit drag activator; nothing it hears from in here is for it.
        e.stopPropagation();
        const cell = e.currentTarget.parentElement;
        if (!cell) return;
        const delta = e.key === "ArrowRight" ? KEY_RESIZE_STEP : -KEY_RESIZE_STEP;
        onResize(Math.max(MIN_COL_WIDTH, getWidth(cell) + delta));
      }}
      onPointerDown={(e) => {
        // BOTH TABLES DEPEND ON THIS STOPPING HERE, and for the same reason in each: a header click
        // cycles the sort and a drag must not. In `ChartTable` the `<th>` is also a dnd-kit drag
        // activator, so the propagation would start a column REORDER on top of it.
        e.stopPropagation();
        if (dragHandlers) return;
        e.preventDefault();
        const cell = e.currentTarget.parentElement;
        if (!cell) return;
        const startX = e.clientX;
        const startW = getWidth(cell);
        const move = (ev: PointerEvent) =>
          onResize(Math.max(MIN_COL_WIDTH, startW + ev.clientX - startX));
        const up = () => {
          window.removeEventListener("pointermove", move);
          window.removeEventListener("pointerup", up);
        };
        window.addEventListener("pointermove", move);
        window.addEventListener("pointerup", up);
      }}
      onClick={(e) => e.stopPropagation()}
    />
  );
}

/**
 * The sort control. Renders the header's own text, so the cell looks exactly as it did and a mouse
 * user's target has not shrunk — the button IS the title.
 *
 * `aria-label` rather than leaning on that text: the button has to say what it DOES, or a screen
 * reader announces "region, button" and the user has to guess.
 *
 * THE COST, MEASURED IN CHROME rather than reasoned about. Wrapping the header text in a labelled
 * control changes what the `<th>` around it is called: with the button's `aria-label` in place and
 * no label on the cell, Chrome's accessibility tree gave that columnheader NO NAME AT ALL — read
 * off the live page by deleting the attribute from one header and re-reading the tree, which showed
 * `columnheader [ref=…]` beside nine siblings each still named for their column. A columnheader with
 * no name is announced for every cell under it, so both call sites give the `<th>` an explicit
 * `aria-label` of the plain column title. jsdom's accname implementation does NOT reproduce this —
 * it answers "region" either way — so the specs can state the intent but cannot police it.
 *
 * The DIRECTION is on the `<th>` as `aria-sort`, never in here: the glyph below is decoration.
 */
export function SortButton({ label, direction, onClick, className, children }: {
  /** The column as it is drawn — a `column_title` rename included. */
  label: string;
  direction?: "asc" | "desc";
  onClick: () => void;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      aria-label={`Sort by ${label}`}
      // No font of its own: Tailwind's preflight already gives a `<button>` `font: inherit`, so the
      // header text reads exactly as it did before it was wrapped in one.
      className={`cursor-pointer bg-transparent p-0 text-inherit ${FOCUS_RING} ${className ?? ""}`}
      // NOT stopped on pointerdown, deliberately: in `ChartTable` the `<th>` around this is a
      // dnd-kit drag activator with an 8px activation distance, so letting pointerdown through is
      // what keeps a column draggable by its own title. A press that never moves 8px starts no drag
      // and arrives here as a click.
      onClick={(e) => { e.stopPropagation(); onClick(); }}
      onKeyDown={(e) => {
        // The header's OTHER keyboard control is dnd-kit's, whose activator fires on Enter and
        // Space. Without this, one Enter both sorted and lifted the column into a drag.
        if (e.key === "Enter" || e.key === " ") e.stopPropagation();
      }}
    >
      {children}
      {direction === "asc" ? " ▲" : direction === "desc" ? " ▼" : ""}
    </button>
  );
}
