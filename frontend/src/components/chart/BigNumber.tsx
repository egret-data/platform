import { useMemo, useRef, type CSSProperties } from "react";
import type { ScalarSegment } from "@/api/charts";
import type { DrillContext } from "@/components/chart/drill";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import type { ColumnFormat } from "./format-types";
import { findSize, measureText } from "./measure-text";
import {
  compactifyValue, matchSegment,
  MIN_SIZE_REM, SCALAR_PADDING, SIZE_STEP_REM, maxFontSizeRem,
} from "./scalar";
import { useElementSize } from "./useElementSize";
import { GRID_COLS } from "./dashboard-grid";

interface ResultColumn { name: string; type: string }

/** Fallback for the first paint, before the ResizeObserver has reported a box. Matches the size
 * this chart was fixed at before it could size itself, so the initial frame is never larger than
 * what it settles to. */
const INITIAL_SIZE = "2.5rem";
const VALUE_FONT_WEIGHT = 700;

/** Metabase-style "Number" viz: one aggregated value, large and formatted. */
export function BigNumber({
  columns, rows, metric, format = {}, segments, label, sublabel, onValueClick, gridCols,
}: {
  columns: ResultColumn[];
  rows: unknown[][];
  metric?: string;
  format?: ColumnFormat;
  /** Conditional colour bands; first match wins (see `matchSegment`). */
  segments?: ScalarSegment[];
  /** Optional caption under the value. */
  label?: string;
  /** Optional second caption line. */
  sublabel?: string;
  /** Present when the chart is drillable. A scalar has no dimensions, so its drill context is
   * empty — "show me the records behind this number". */
  onValueClick?: (ctx: DrillContext, x: number, y: number) => void;
  /** The card's width in dashboard grid columns. A card that owns more of the grid earns a bigger
   * ceiling; without it (the studio preview) the ceiling is `NO_GRID_MAX_SIZE`. */
  gridCols?: number;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  const captionRef = useRef<HTMLDivElement>(null);
  const size = useElementSize(hostRef);
  const captionSize = useElementSize(captionRef);
  // The captions sit in the same centred column as the value, so the room left for the number is
  // the box minus whatever they take. Measured, not assumed: a first attempt gave the value host
  // `flex-1 basis-0`, which let it swallow all the spare height and pinned the captions to the
  // BOTTOM of the card, ~300px under the number. Every numeric check passed — the number fitted,
  // nothing overflowed — because none of them measured the gap. Only the screenshot showed it.
  const captionHeight = captionSize?.height ?? 0;

  const mi = metric != null ? columns.findIndex((c) => c.name === metric) : -1;
  const raw = mi >= 0 && rows.length > 0 ? rows[0][mi] : undefined;
  const type = mi >= 0 ? columns[mi].type : "double";

  // Compact against the box, then size the RESULT. Sizing the uncompacted string would pick a
  // font for text that is not on screen.
  const { display, full } = useMemo(() => {
    if (raw == null) return { display: "—", full: "—" };
    const width = size ? Math.max(size.width - SCALAR_PADDING, 0) : Number.POSITIVE_INFINITY;
    return compactifyValue(raw, type, width, format);
  }, [raw, type, size, format]);

  const fontSize = useMemo(() => {
    if (!size || raw == null) return INITIAL_SIZE;
    const host = hostRef.current;
    // The family must be the one the DOM actually renders — chart 05 drew every chart in
    // `sans-serif` while the UI was Inter, and a fit computed against the wrong family is wrong
    // by however much the two differ in advance width.
    const fontFamily = host ? window.getComputedStyle(host).fontFamily : "inherit";
    return findSize({
      text: display,
      targetHeight: Math.max(size.height - captionHeight - SCALAR_PADDING, 0),
      targetWidth: Math.max(size.width - SCALAR_PADDING, 0),
      unit: "rem",
      fontFamily,
      fontWeight: VALUE_FONT_WEIGHT,
      step: SIZE_STEP_REM,
      min: MIN_SIZE_REM,
      max: maxFontSizeRem(gridCols, GRID_COLS, size.width),
      measurer: measureText,
    });
  }, [size, display, raw, gridCols, captionHeight]);

  const segment = matchSegment(raw, segments);
  const isClickable = onValueClick != null && raw != null;
  const isCompacted = display !== full;

  // The full value, never the compacted one — a screen reader has no use for "3.4M" when the
  // number is 3,428,911. The band's label rides along so the colour is not the only carrier of
  // its meaning.
  const spoken = raw == null
    ? (metric ? `${metric}: no value` : "No value")
    : [metric ? `${metric}: ${full}` : full, segment?.label].filter(Boolean).join(", ");

  const valueEl = (
    <span
      className={cn(
        "tabular-nums leading-none",
        // A band colour is applied inline; without one the token carries it. `hover:text-primary`
        // is the affordance ONLY when no band is in force — recolouring a banded number on hover
        // would overwrite the one thing its colour is there to say.
        !segment && "text-foreground",
        isClickable && !segment && "group-hover:text-primary",
        isClickable && segment && "group-hover:underline",
      )}
      style={{
        fontSize,
        fontWeight: VALUE_FONT_WEIGHT,
        ...(segment ? { color: segment.color } : {}),
      } as CSSProperties}
    >
      {display}
    </span>
  );

  const interactive = isClickable ? (
    <button
      type="button"
      className="group rounded focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
      aria-label={`${spoken}. Show records`}
      onClick={(e) => {
        const r = e.currentTarget.getBoundingClientRect();
        onValueClick?.({ dims: [] }, r.left + r.width / 2, r.bottom);
      }}
    >
      {valueEl}
    </button>
  ) : (
    valueEl
  );

  return (
    <div
      className="relative flex h-full min-h-[5rem] w-full flex-col items-center justify-center"
      // When the number is not interactive the whole card reads as one image, which is what makes
      // a screen reader say "total_revenue: 25,468" as a single utterance instead of a bare number
      // followed by an unrelated-sounding word. Once it IS interactive that role would hide the
      // control, so the accessible name moves onto the button.
      {...(isClickable ? {} : { role: "img", "aria-label": spoken })}
    >
      {/* `absolute inset-0`: this measures the WHOLE box without taking part in the layout, so the
          number and its captions stay one centred group. Sizing it by `flex-1` instead made it
          swallow the spare height and stranded the captions at the bottom of the card. */}
      <div ref={hostRef} aria-hidden className="pointer-events-none absolute inset-0" />

      <div className="flex w-full min-w-0 items-center justify-center px-2">
        {isCompacted ? (
          <TooltipProvider delayDuration={200}>
            <Tooltip>
              {/* `asChild` keeps the button/span as the trigger, so the exact figure is reachable
                  by keyboard focus and not only by hover. */}
              <TooltipTrigger asChild>{interactive}</TooltipTrigger>
              <TooltipContent>{full}</TooltipContent>
            </Tooltip>
          </TooltipProvider>
        ) : (
          interactive
        )}
      </div>

      {/* One measured block, so the space it takes can be subtracted from the number's own. The
          band's name is TEXT here, not only colour: Metabase keeps it in a hover tooltip, which
          leaves a red and a green number indistinguishable to a colourblind reader or on a
          monochrome display — `color-not-only`, High severity. */}
      <div ref={captionRef} className="w-full shrink-0">
        {segment?.label && (
          <div className="mt-1.5 px-2 text-center text-xs font-medium" style={{ color: segment.color }}>
            {segment.label}
          </div>
        )}
        {label && <div className="mt-3 px-2 text-center text-sm text-foreground">{label}</div>}
        {sublabel && <div className="mt-1 px-2 text-center text-xs text-muted-foreground">{sublabel}</div>}
      </div>
    </div>
  );
}
