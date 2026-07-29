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

export type Point = [number, number];
export type LabelPosition = "horizontal" | "radial";

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
 * `startAngle: 90, clockwise: true`. */
export const getCoordOnCircle = (radius: number, angle: number): Point => [
  radius * Math.sin(angle),
  radius * Math.cos(angle),
];

/** Intersection points of a circle and a horizontal line y = horizontalY. Empty if the line
 * misses the circle, one point if tangent, two if it crosses. */
export const calcCircleIntersectionByHorizontalLine = (
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
    console.warn(
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
    console.warn(`Outer radius must be bigger than inner. Outer: ${outerRadius} inner: ${innerRadius}`);
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

/**
 * Decides what a single slice's label should actually say, given the space its own geometry
 * allows (`availableSpace`, from `calcAvailableDonutSliceLabelLength` minus this chart's label
 * padding) — ports the fit/truncate/drop decision embedded in Metabase's
 * `getSeriesDataFromSlices` (option.ts ~L237-253):
 *  - fits as-is → shown in full;
 *  - doesn't fit and labels are off entirely → dropped (no truncation attempted, matching
 *    Metabase's `else if (settings["pie.show_labels"])` guard);
 *  - doesn't fit, labels are on, but there's no room at all (`availableSpace <= 0`) → dropped;
 *  - doesn't fit, labels are on, some room → truncated with an ellipsis, UNLESS that truncation
 *    collapses to the bare ellipsis alone, which reads as noise rather than a label and is
 *    dropped too.
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

  if (measureText(label, fontStyle) <= availableSpace) {
    return label;
  }
  if (!showLabels || availableSpace <= 0) {
    return null;
  }

  const truncated = truncateLabelText(label, availableSpace, measureText, fontStyle);
  return truncated === CHAR_ELLIPSIS ? null : truncated;
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
