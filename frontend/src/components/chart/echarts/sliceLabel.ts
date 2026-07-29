// Slice-label fitting math, ported from Metabase's pie chart:
//   frontend/src/metabase/visualizations/echarts/pie/util/label.ts (calcChordLength,
//   getCoordOnCircle, calcCircleIntersectionByHorizontalLine, calcMaxRectLengthWithinDonut,
//   isNearXAxis, calcAvailableDonutSliceLabelLength, calcInnerOuterRadiusesForRing) and the
//   measure-then-truncate-or-drop decision embedded in that file's option.ts
//   (getSeriesDataFromSlices, ~L237-253) + frontend/src/metabase/visualizations/lib/text.ts
//   (truncateText/CHAR_ELLIPSES).
//
// Metabase's own sunburst has no built-in truncation, so it measures each label against the
// literal chord/rectangle its own slice can hold and truncates or drops it outright — this is
// the mechanism egret was missing (labelLayout.hideOverlap is only a backstop, see options/pie.ts).
//
// Kept as pure geometry + a measure-callback-driven fit decision (no DOM, no ECharts types) so
// it can be unit tested with a deterministic stub measurer instead of a real text rasteriser —
// see measureText below for the one place that talks to a real canvas.

/** Module-private for the same reason as `LabelPosition` below: with the two helpers that
 * returned it now private, nothing outside this file can see a `Point` at all. */
type Point = [number, number];
/** Whether a slice's label runs across the ring or along it. Module-private: nothing outside
 * names it — pie.ts passes `"radial"`/`"horizontal"` as literals — and an exported alias for a
 * two-member union is surface without a consumer. */
type LabelPosition = "horizontal" | "radial";

export interface FontStyle {
  size: number;
  family?: string;
  weight?: number | string;
}

/** Measures the rendered pixel width of `text` set in `fontStyle`. Metabase injects this via
 * `renderingContext.measureText`; egret has no such context object, so it's threaded through
 * as a plain callback instead — see `measureText` below for the real (canvas) implementation
 * and `defaultMeasureText`'s jsdom fallback. */
export type MeasureText = (text: string, fontStyle: FontStyle) => number;

/** Calculates the length of a chord given the radius of a circle and the central angle. */
export const calcChordLength = (radius: number, angle: number): number =>
  Math.sqrt(2 * radius ** 2 * (1 - Math.cos(angle)));

/** Coordinates of a point on a circle, angle measured from the top (12 o'clock), increasing
 * clockwise — the same convention ECharts' pie/sunburst series uses for its default
 * `startAngle: 90, clockwise: true`.
 *
 * Module-private: only calcAvailableDonutSliceLabelLength calls it, and the angle convention
 * that is its whole content is already pinned from outside — the near-x-axis tests move one
 * slice shape around the circle and assert a different allowance at 12/6 o'clock than at 3/9,
 * which only holds if this measures from the top. A direct test would restate that convention
 * rather than check it, so there is nothing here worth an export. */
const getCoordOnCircle = (radius: number, angle: number): Point => [
  radius * Math.sin(angle),
  radius * Math.cos(angle),
];

/** Intersection points of a circle and a horizontal line y = horizontalY. Empty if the line
 * misses the circle, one point if tangent, two if it crosses.
 *
 * Module-private: only getDonutHorizontalChordCoords calls it. Its one branch with a
 * consequence — the miss/tangent cases, which give that caller too few points to build a chord
 * from — is covered from outside through the degenerate-geometry tests above, which drive
 * exactly that path. */
const calcCircleIntersectionByHorizontalLine = (
  radius: number,
  horizontalY: number,
): [] | [Point] | [Point, Point] => {
  const absY = Math.abs(horizontalY);
  if (absY > radius) {
    return [];
  }
  if (absY === radius) {
    return [[0, horizontalY]];
  }

  const x = Math.sqrt(radius ** 2 - horizontalY ** 2);
  return [
    [-x, horizontalY],
    [x, horizontalY],
  ];
};

/**
 * Conditions already warned about, so each one is reported once per page rather than once per
 * slice per frame.
 *
 * The two warnings below sit inside a per-slice loop that runs on every option rebuild — and
 * since a pie's option is rebuilt against the container's real measured size, that includes once
 * per animation frame throughout a resize drag. A single chart with degenerate geometry could
 * therefore flood the console at frame rate, which both drowns out everything else and makes the
 * page slower exactly when it is already struggling. (Metabase's equivalent warnings can't do
 * this: nothing there is resize-driven.)
 *
 * Both are kept rather than dropped, because both DO carry something actionable: each one names a
 * CALLER CONTRACT VIOLATION that nothing else reveals. Given the thickness guard that runs ahead
 * of them, neither is reachable from sane geometry at all — an inner radius at or past the outer
 * is already short-circuited there for any positive font size, and a label band can only miss the
 * outer circle if a radius or the font size is negative. So if either fires, the caller handed
 * this module nonsense, and the only visible symptom otherwise is a label silently vanishing.
 * What is NOT actionable is the same violation restated 60 times a second with slightly
 * different radii.
 *
 * So the key is the CONDITION, not the numbers: keying on the geometry would defeat the point,
 * since a resize drag produces fresh radii on every frame and every one of them would be
 * "distinct". The first occurrence's numbers still go into the message, which is the one a
 * developer would investigate anyway.
 */
const warnedConditions = new Set<string>();

function warnOnce(condition: string, message: string): void {
  if (warnedConditions.has(condition)) return;
  warnedConditions.add(condition);
  console.warn(message);
}

function getDonutHorizontalChordCoords(
  y: number,
  innerRadius: number,
  outerRadius: number,
): [Point, Point] {
  const [innerLeft] = calcCircleIntersectionByHorizontalLine(innerRadius, y);
  const [outerLeft, outerRight] = calcCircleIntersectionByHorizontalLine(outerRadius, y);

  if (!outerLeft || !outerRight) {
    throw new Error(`Invalid horizontal label y = ${y}`);
  }

  return innerLeft ? [outerLeft, innerLeft] : [outerLeft, outerRight];
}

function calcMaxRectLengthWithinDonut(
  topY: number,
  bottomY: number,
  innerRadius: number,
  outerRadius: number,
): number {
  try {
    const [topLeft, topRight] = getDonutHorizontalChordCoords(topY, innerRadius, outerRadius);
    const [bottomLeft, bottomRight] = getDonutHorizontalChordCoords(bottomY, innerRadius, outerRadius);

    const leftX = Math.max(topLeft[0], bottomLeft[0]);
    const rightX = Math.min(topRight[0], bottomRight[0]);

    return rightX - leftX;
  } catch {
    warnOnce(
      "maxRectLength",
      `Could not calculate max rectangle length for innerRadius=${innerRadius} outerRadius=${outerRadius} topY=${topY} bottomY=${bottomY}`,
    );
    return 0;
  }
}

function isNearXAxis(angle: number, tolerance: number = Math.PI / 8): boolean {
  const normalizedAngle = ((angle % (2 * Math.PI)) + 2 * Math.PI) % (2 * Math.PI);
  return (
    Math.abs(normalizedAngle - Math.PI / 2) <= tolerance ||
    Math.abs(normalizedAngle - (3 * Math.PI) / 2) <= tolerance
  );
}

/**
 * Available pixel length for a donut slice's label — the core anti-overlap mechanism (fix 1b):
 * every label is measured against its OWN slice's geometry rather than relying solely on
 * ECharts' post-layout `labelLayout.hideOverlap` backstop.
 *
 * Two early exits, both ported as-is from Metabase:
 *  - a ring thinner than 2 font sizes has no room for a radial label at all;
 *  - for a "radial" label, the constraint is the INNER chord (the narrowest point a radial
 *    label crosses) rather than a rectangle — a label longer than the ring is fine as long as
 *    the ring is wide enough to write it along, but a slice too narrow at the inner edge can't
 *    hold even one character's width without a neighbour's label overlapping it.
 */
export function calcAvailableDonutSliceLabelLength(
  innerRadius: number,
  outerRadius: number,
  startAngle: number,
  endAngle: number,
  fontSize: number,
  labelPosition: LabelPosition,
): number {
  const donutThickness = outerRadius - innerRadius;
  if (donutThickness <= 2 * fontSize) {
    return 0;
  }

  if (innerRadius >= outerRadius) {
    warnOnce(
      "innerRadiusNotSmaller",
      `Outer radius must be bigger than inner. Outer: ${outerRadius} inner: ${innerRadius}`,
    );
    return 0;
  }

  const arcAngle = endAngle - startAngle;
  const innerChordLength = calcChordLength(innerRadius, Math.min(arcAngle, Math.PI));

  if (labelPosition === "radial") {
    return innerChordLength > fontSize ? donutThickness : 0;
  }

  const midRadius = (innerRadius + outerRadius) / 2;
  const midAngle = (startAngle + endAngle) / 2;
  const [, sliceCenterY] = getCoordOnCircle(midRadius, midAngle);

  const labelTopY = sliceCenterY - fontSize;
  const labelBottomY = sliceCenterY + fontSize;
  const maxRectLength = calcMaxRectLengthWithinDonut(labelTopY, labelBottomY, innerRadius, outerRadius);

  const sliceConstraint =
    isNearXAxis(midAngle) && innerChordLength > fontSize ? donutThickness : innerChordLength;

  return Math.min(maxRectLength, sliceConstraint);
}

/** Splits an overall [innerRadius, outerRadius] band evenly across `numRings`, returning the
 * band for the given 1-based `ring`. */
export function calcInnerOuterRadiusesForRing(
  innerRadius: number,
  outerRadius: number,
  numRings: number,
  ring: number,
): { inner: number; outer: number } {
  const donutWidth = (outerRadius - innerRadius) / numRings;
  return {
    inner: innerRadius + donutWidth * (ring - 1),
    outer: innerRadius + donutWidth * ring,
  };
}

/** Sentinel meaning "truncation ran out of room for any real character" — ported from
 * Metabase's `CHAR_ELLIPSES` (frontend/src/metabase/visualizations/lib/text.ts). */
export const CHAR_ELLIPSIS = "…";

/** Binary-searches the longest prefix of `text` (plus an ellipsis) that fits in `maxWidth`,
 * ported from Metabase's `truncateText`. Returns the bare ellipsis when even that doesn't fit. */
export function truncateLabelText(
  text: string,
  maxWidth: number,
  measureText: MeasureText,
  fontStyle: FontStyle,
): string {
  if (measureText(text, fontStyle) <= maxWidth) {
    return text;
  }

  const ellipsisWidth = measureText(CHAR_ELLIPSIS, fontStyle);
  const availableWidth = maxWidth - ellipsisWidth;
  if (availableWidth <= 0) {
    return CHAR_ELLIPSIS;
  }

  let left = 0;
  let right = text.length;
  let bestFit = "";

  while (left <= right) {
    const mid = Math.floor((left + right) / 2);
    const truncated = text.slice(0, mid);
    const truncatedWidth = measureText(truncated, fontStyle);

    if (truncatedWidth <= availableWidth) {
      bestFit = truncated;
      left = mid + 1;
    } else {
      right = mid - 1;
    }
  }

  return bestFit + CHAR_ELLIPSIS;
}

/** Minimum number of REAL (non-ellipsis) characters a truncated label must keep to be worth
 * showing at all — a judgement call beyond Metabase's own bare-ellipsis-only guard (Metabase
 * never has to make this call: it measures against real pixels, so it essentially never
 * produces a 1-2 character truncation in practice). Once egret ALSO measures against the
 * chart's real size (see options/pie.ts's `containerSize`), a razor-thin truncation like "L…"
 * becomes rare — but not impossible: a genuinely narrow slice (e.g. a thin donut ring, or a
 * slice pinned just above the "Other" fold threshold) can still have real, if small, room.
 *
 * A 1-2 character prefix identifies nothing on its own AND is easily mistaken for a real, short,
 * untruncated value — plenty of real dimension values ARE 2-letter codes ("US", "EU", "CA",
 * region/country abbreviations…), so a bare "U…" reads as if it might BE one of those rather
 * than a cut-off "United [something]". 3 is chosen so a kept truncation always shows strictly
 * more characters than the shortest common real codes are wide, rather than a floor that would
 * still let a 1- or 2-character prefix through looking like real, complete data.
 */
export const MIN_TRUNCATED_LABEL_CHARS = 3;

/**
 * Decides what a single slice's label should actually say, given the space its own geometry
 * allows (`availableSpace`, from `calcAvailableDonutSliceLabelLength` minus this chart's label
 * padding) — ports the fit/truncate/drop decision embedded in Metabase's
 * `getSeriesDataFromSlices` (option.ts ~L237-253), extended with the MIN_TRUNCATED_LABEL_CHARS
 * floor above (egret-specific — see its own comment for why):
 *  - blank or all-whitespace label (no real dimension value) → dropped outright — there is
 *    nothing to fit, and the trivial "an empty string fits any width" case would otherwise slip
 *    through as a bare "" label without ever reaching the truncation logic below, which is
 *    exactly what let this render as a content-less blank instead of the intentional
 *    "nothing to show" signal every other dropped label already produces;
 *  - fits as-is → shown in full;
 *  - doesn't fit and labels are off entirely → dropped (no truncation attempted, matching
 *    Metabase's `else if (settings["pie.show_labels"])` guard);
 *  - doesn't fit, labels are on, but there's no room at all (`availableSpace <= 0`) → dropped;
 *  - doesn't fit, labels are on, some room → truncated with an ellipsis, UNLESS that truncation
 *    keeps fewer than MIN_TRUNCATED_LABEL_CHARS real characters (which subsumes the old
 *    bare-ellipsis-only check — 0 real characters is just the smallest case of "too few").
 *
 * Pure and DOM-free: takes `measureText` as a parameter rather than calling a real rasteriser
 * itself, so it can be exercised with a deterministic stub in tests.
 */
export function fitSliceLabel(params: {
  label: string;
  availableSpace: number;
  showLabels: boolean;
  measureText: MeasureText;
  fontStyle: FontStyle;
}): string | null {
  const { label, availableSpace, showLabels, measureText, fontStyle } = params;

  if (label.trim() === "") {
    return null;
  }

  if (measureText(label, fontStyle) <= availableSpace) {
    return label;
  }
  if (!showLabels || availableSpace <= 0) {
    return null;
  }

  const truncated = truncateLabelText(label, availableSpace, measureText, fontStyle);
  const survivingChars = truncated.endsWith(CHAR_ELLIPSIS) ? truncated.length - CHAR_ELLIPSIS.length : truncated.length;
  return survivingChars < MIN_TRUNCATED_LABEL_CHARS ? null : truncated;
}

// Average glyph width, as a fraction of font size, for the jsdom fallback below. ~0.5 is a
// commonly cited average for proportional Latin UI fonts (narrow glyphs like "i"/"l" sit
// nearer 0.3, wide ones like "M"/"W" nearer 1); it's a rough approximation, not a substitute
// for real text measurement, and exists only so this code has SOMETHING to measure with in an
// environment with no text rasteriser at all.
const AVERAGE_GLYPH_WIDTH_RATIO = 0.5;

let sharedCanvas: HTMLCanvasElement | undefined;

/**
 * Real text measurement via an offscreen `<canvas>` 2D context — the DOM equivalent of
 * Metabase's `renderingContext.measureText` (which in the app is backed by the same API).
 *
 * jsdom (this project's unit test environment, see vitest.config.ts) has no `canvas` npm
 * package installed, so `HTMLCanvasElement.getContext("2d")` returns `null` there rather than
 * a working 2D context — there is no real rasteriser to fall back to in tests, only in an
 * actual browser. Rather than throw (which would take down every option-builder test that
 * exercises the pie chart's default path), this falls back to the fixed average-character-width
 * approximation above. Tests that care about EXACT fitting behaviour pass their own stub
 * measurer instead (see sliceLabel.test.ts) — this default is only meant to be "good enough"
 * for the real app, where a real canvas is always available.
 */
export function measureText(text: string, fontStyle: FontStyle): number {
  if (typeof document !== "undefined") {
    try {
      sharedCanvas ??= document.createElement("canvas");
      const ctx = sharedCanvas.getContext("2d");
      if (ctx) {
        ctx.font = `${fontStyle.weight ?? 400} ${fontStyle.size}px ${fontStyle.family ?? "sans-serif"}`;
        return ctx.measureText(text).width;
      }
    } catch {
      // No usable canvas (e.g. a non-browser DOM implementation) — fall through below.
    }
  }
  return text.length * fontStyle.size * AVERAGE_GLYPH_WIDTH_RATIO;
}
