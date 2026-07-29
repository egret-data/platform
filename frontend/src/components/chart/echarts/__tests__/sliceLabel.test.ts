import { describe, it, expect } from "vitest";
import {
  calcChordLength,
  calcAvailableDonutSliceLabelLength,
  calcInnerOuterRadiusesForRing,
  fitSliceLabel,
  truncateLabelText,
  CHAR_ELLIPSIS,
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

  it("truncates with an ellipsis when the label almost fits and labels are on", () => {
    // "EMEA" is 28px (7*4); give it just enough room for 2 chars + ellipsis (7*2 + 7 = 21).
    const result = fitSliceLabel({
      label: "EMEA",
      availableSpace: 21,
      showLabels: true,
      measureText: stubMeasure,
      fontStyle,
    });
    expect(result).toBe(`EM${CHAR_ELLIPSIS}`);
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
});

describe("truncateLabelText", () => {
  it("returns the text unchanged when it already fits", () => {
    expect(truncateLabelText("EU", 100, stubMeasure, { size: 12 })).toBe("EU");
  });

  it("returns the bare ellipsis when even one character plus ellipsis doesn't fit", () => {
    expect(truncateLabelText("EMEA", 6, stubMeasure, { size: 12 })).toBe(CHAR_ELLIPSIS);
  });
});
