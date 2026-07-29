import { describe, it, expect } from "vitest";
import {
  calcChordLength,
  calcAvailableDonutSliceLabelLength,
  calcInnerOuterRadiusesForRing,
  fitSliceLabel,
  truncateLabelText,
  CHAR_ELLIPSIS,
  MIN_TRUNCATED_LABEL_CHARS,
} from "../sliceLabel";

// A deterministic stub measurer so expected numbers are obvious: 7px per character, whatever
// the requested font size/family/weight — matches the ground rules' guidance for tests 2, 9, 10.
const stubMeasure = (s: string) => s.length * 7;

describe("calcChordLength", () => {
  it("a half-circle (angle = π) chord is the diameter", () => {
    expect(calcChordLength(10, Math.PI)).toBeCloseTo(20, 9);
  });
});

describe("calcAvailableDonutSliceLabelLength", () => {
  const fontSize = 12;

  it("a wide-arc slice in a thick ring fits its label; a 1-degree slice in the SAME ring does not", () => {
    const inner = 50;
    const outer = 100; // donutThickness 50, well over 2*fontSize (24)
    const wide = calcAvailableDonutSliceLabelLength(inner, outer, 0, Math.PI, fontSize, "horizontal");
    const oneDegree = calcAvailableDonutSliceLabelLength(
      inner,
      outer,
      0,
      (1 * Math.PI) / 180,
      fontSize,
      "horizontal",
    );
    // The 1-degree slice has drastically less room than the half-circle slice — not necessarily
    // exactly 0 (a horizontal label's constraint is a rectangle-within-donut, which isn't
    // strictly clamped to 0 the way the radial branch is), so what actually matters is whether a
    // real label FITS in each: it fits the wide arc but not the sliver.
    expect(oneDegree).toBeLessThan(wide);

    const label = "EMEA"; // 4 chars * 7px = 28px
    expect(fitSliceLabel({ label, availableSpace: wide, showLabels: true, measureText: stubMeasure, fontStyle: { size: fontSize } })).toBe(label);
    expect(fitSliceLabel({ label, availableSpace: oneDegree, showLabels: false, measureText: stubMeasure, fontStyle: { size: fontSize } })).toBeNull();
  });

  it("radial: a wide arc returns the full donut thickness; a narrow arc returns 0", () => {
    const inner = 50;
    const outer = 100;
    const wide = calcAvailableDonutSliceLabelLength(inner, outer, 0, Math.PI, fontSize, "radial");
    expect(wide).toBe(outer - inner);

    const narrow = calcAvailableDonutSliceLabelLength(
      inner,
      outer,
      0,
      (1 * Math.PI) / 180,
      fontSize,
      "radial",
    );
    expect(narrow).toBe(0);
  });

  // The near-x-axis branch (Metabase's isNearXAxis, ported): a HORIZONTAL label on a slice whose
  // mid-angle sits within ±π/8 of 3 or 9 o'clock runs ALONG the ring rather than across it, so it
  // is allowed the donut's thickness instead of being pinned to the (much shorter) inner chord.
  // Both sides of that branch were previously unexercised — hardcoding it to `true` AND to
  // `false` each left every sliceLabel and pie test green.
  //
  // One slice shape, moved around the circle: inner 50, outer 100 (thickness 50), a 0.4rad arc
  // (inner chord ~19.87, comfortably over the 12px font size the branch also requires). The two
  // outcomes are far enough apart — 50 vs ~19.87 — that whichever way the branch is nailed shut,
  // some position below reports the wrong number.
  describe("near-x-axis slices are allowed the ring's thickness instead of their chord", () => {
    const inner = 50;
    const outer = 100;
    const arc = 0.4;
    const innerChord = calcChordLength(inner, arc);
    const at = (midAngle: number) =>
      calcAvailableDonutSliceLabelLength(inner, outer, midAngle - arc / 2, midAngle + arc / 2, fontSize, "horizontal");

    it("a slice centred at 3 or 9 o'clock gets the full donut thickness", () => {
      expect(at(Math.PI / 2)).toBeCloseTo(outer - inner, 9);
      expect(at((3 * Math.PI) / 2)).toBeCloseTo(outer - inner, 9);
    });

    it("the same slice at 12 or 6 o'clock gets only its inner chord", () => {
      expect(at(0)).toBeCloseTo(innerChord, 9);
      expect(at(Math.PI)).toBeCloseTo(innerChord, 9);
      expect(innerChord).toBeLessThan(outer - inner); // the two branches really do differ here
    });

    it("the allowance ends at the ±π/8 tolerance, not gradually", () => {
      const justInside = at(Math.PI / 2 + Math.PI / 8 - 0.01);
      const justOutside = at(Math.PI / 2 + Math.PI / 8 + 0.01);
      expect(justInside).toBeGreaterThan(innerChord);
      expect(justOutside).toBeCloseTo(innerChord, 9);
    });

    it("an arc too narrow to hold one character is pinned to its chord even at 3 o'clock", () => {
      // The branch needs BOTH conditions: near the axis AND an inner chord wider than the font
      // size. A 0.1rad arc's chord is ~5px, under the 12px font — no thickness allowance.
      const narrow = 0.1;
      const narrowChord = calcChordLength(inner, narrow);
      expect(narrowChord).toBeLessThan(fontSize);
      expect(
        calcAvailableDonutSliceLabelLength(inner, outer, Math.PI / 2 - narrow / 2, Math.PI / 2 + narrow / 2, fontSize, "horizontal"),
      ).toBeCloseTo(narrowChord, 9);
    });
  });

  it("a ring thinner than 2 font sizes returns 0 regardless of arc width", () => {
    const inner = 50;
    const outer = inner + 2 * fontSize; // exactly at the cutoff — donutThickness <= 2*fontSize
    const wideArcHorizontal = calcAvailableDonutSliceLabelLength(inner, outer, 0, Math.PI, fontSize, "horizontal");
    const wideArcRadial = calcAvailableDonutSliceLabelLength(inner, outer, 0, Math.PI, fontSize, "radial");
    expect(wideArcHorizontal).toBe(0);
    expect(wideArcRadial).toBe(0);

    // Even a full circle can't rescue a too-thin ring.
    const fullCircle = calcAvailableDonutSliceLabelLength(inner, outer, 0, 2 * Math.PI, fontSize, "radial");
    expect(fullCircle).toBe(0);
  });
});

describe("calcInnerOuterRadiusesForRing", () => {
  it("splits 3 rings evenly, contiguously, and ring 3's outer equals the chart's outer radius", () => {
    const inner = 30;
    const outer = 100;
    const width = (outer - inner) / 3;
    const ring1 = calcInnerOuterRadiusesForRing(inner, outer, 3, 1);
    const ring2 = calcInnerOuterRadiusesForRing(inner, outer, 3, 2);
    const ring3 = calcInnerOuterRadiusesForRing(inner, outer, 3, 3);

    expect(ring1.inner).toBeCloseTo(inner, 9);
    expect(ring1.outer).toBeCloseTo(inner + width, 9);
    expect(ring2.inner).toBeCloseTo(ring1.outer, 9);
    expect(ring2.outer).toBeCloseTo(inner + 2 * width, 9);
    expect(ring3.inner).toBeCloseTo(ring2.outer, 9);
    expect(ring3.outer).toBeCloseTo(outer, 9);
  });
});

describe("fitSliceLabel", () => {
  const fontStyle = { size: 12 };

  it("returns the full label when it fits", () => {
    const result = fitSliceLabel({
      label: "EMEA",
      availableSpace: 100,
      showLabels: true,
      measureText: stubMeasure,
      fontStyle,
    });
    expect(result).toBe("EMEA");
  });

  it("drops the label (no truncation attempted) when labels are off and it doesn't fit", () => {
    const result = fitSliceLabel({
      label: "A rather long region name",
      availableSpace: 10,
      showLabels: false,
      measureText: stubMeasure,
      fontStyle,
    });
    expect(result).toBeNull();
  });

  it("truncates with an ellipsis when the label almost fits, keeps at least MIN_TRUNCATED_LABEL_CHARS, and labels are on", () => {
    // "REGION" is 42px (7*6); give it just enough room for 3 chars + ellipsis (7*3 + 7 = 28) —
    // exactly MIN_TRUNCATED_LABEL_CHARS survivors, the shortest truncation this function keeps.
    const result = fitSliceLabel({
      label: "REGION",
      availableSpace: 28,
      showLabels: true,
      measureText: stubMeasure,
      fontStyle,
    });
    expect(result).toBe(`REG${CHAR_ELLIPSIS}`);
  });

  it("drops the label rather than showing a bare ellipsis when there's no room for any character", () => {
    const result = fitSliceLabel({
      label: "EMEA",
      availableSpace: 5, // less than the ellipsis's own width (7)
      showLabels: true,
      measureText: stubMeasure,
      fontStyle,
    });
    expect(result).toBeNull();
  });

  it("drops the label when availableSpace is 0 or negative, even with labels on", () => {
    const result = fitSliceLabel({
      label: "EMEA",
      availableSpace: 0,
      showLabels: true,
      measureText: stubMeasure,
      fontStyle,
    });
    expect(result).toBeNull();
  });

  // Judgement call (see MIN_TRUNCATED_LABEL_CHARS's own doc comment): a truncation that keeps
  // FEWER than the minimum survives geometrically (there's room to draw it) but is dropped
  // anyway, because 1-2 leading characters identify nothing and are easily mistaken for a real,
  // short, untruncated value (many real dimension values ARE 2-letter codes: "US", "EU", "CA"…).
  it("drops a truncation that keeps fewer than MIN_TRUNCATED_LABEL_CHARS real characters, even though it geometrically fits", () => {
    // Same "REGION" label, one character less of room (availableSpace 21 -> availableWidth 14
    // -> only "RE" (2 chars, 14px) fits) — geometrically valid, but below the floor.
    const result = fitSliceLabel({
      label: "REGION",
      availableSpace: 21,
      showLabels: true,
      measureText: stubMeasure,
      fontStyle,
    });
    expect(result).toBeNull();
  });

  it("keeps a truncation right at the MIN_TRUNCATED_LABEL_CHARS floor, drops one character below it", () => {
    const atFloor = fitSliceLabel({
      label: "REGION",
      availableSpace: MIN_TRUNCATED_LABEL_CHARS * 7 + 7, // MIN chars + ellipsis, exactly
      showLabels: true,
      measureText: stubMeasure,
      fontStyle,
    });
    expect(atFloor).toBe("REG…");

    const belowFloor = fitSliceLabel({
      label: "REGION",
      availableSpace: (MIN_TRUNCATED_LABEL_CHARS - 1) * 7 + 7, // one char short of the floor
      showLabels: true,
      measureText: stubMeasure,
      fontStyle,
    });
    expect(belowFloor).toBeNull();
  });

  // Bug fix — a slice whose dimension value is blank/missing (name === "", or all-whitespace)
  // used to fall through the "already fits" fast path (an empty string trivially fits ANY
  // availableSpace) and render as a bare "" — not caught by the bare-ellipsis guard at all,
  // since it never reaches truncation. This is what actually produced the reported "renders as
  // a blank ' '" symptom for some slices (see options/pie.ts's own "nothing to show" sentinel,
  // which only ever gets a chance to run when fitSliceLabel returns null).
  it("treats a blank or all-whitespace label as nothing to show, not a trivially-fitting empty string", () => {
    expect(fitSliceLabel({ label: "", availableSpace: 100, showLabels: true, measureText: stubMeasure, fontStyle })).toBeNull();
    expect(fitSliceLabel({ label: "   ", availableSpace: 100, showLabels: true, measureText: stubMeasure, fontStyle })).toBeNull();
  });
});

describe("truncateLabelText", () => {
  it("returns the text unchanged when it already fits", () => {
    expect(truncateLabelText("EU", 100, stubMeasure, { size: 12 })).toBe("EU");
  });

  it("returns the bare ellipsis when even one character plus ellipsis doesn't fit", () => {
    expect(truncateLabelText("EMEA", 6, stubMeasure, { size: 12 })).toBe(CHAR_ELLIPSIS);
  });
});
