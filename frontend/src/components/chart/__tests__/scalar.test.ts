import { describe, it, expect } from "vitest";
import {
  compactifyValue, isValidSegment, matchSegment, formatSegmentRange, shouldCompact, maxFontSizeRem,
  MAX_SIZE_LARGE, NARROW_CARD_MAX_SIZE,
} from "../scalar";
import { findSize, type TextMeasurer } from "../measure-text";

describe("matchSegment", () => {
  const bands = [
    { min: 0, max: 1500, color: "#E75454", label: "Low" },
    { min: 1501, max: 1700, color: "#51528D", label: "Medium" },
    { min: 1701, max: 10000, color: "#689735", label: "High" },
  ];

  it("picks the band containing the value", () => {
    expect(matchSegment(1600, bands)?.label).toBe("Medium");
    expect(matchSegment(0, bands)?.label).toBe("Low");
    expect(matchSegment(10000, bands)?.label).toBe("High");
  });

  it("treats both bounds as inclusive", () => {
    expect(matchSegment(1500, bands)?.label).toBe("Low");
    expect(matchSegment(1501, bands)?.label).toBe("Medium");
  });

  it("returns the FIRST match, so authored order decides an overlap", () => {
    // Not a hypothetical: the editor does not sort, so a user can author overlapping bands.
    // Reversing the order must change the answer — if it does not, something is sorting.
    const overlapping = [
      { min: 0, max: 100, color: "#aaa", label: "first" },
      { min: 50, max: 200, color: "#bbb", label: "second" },
    ];
    expect(matchSegment(75, overlapping)?.label).toBe("first");
    expect(matchSegment(75, [...overlapping].reverse())?.label).toBe("second");
  });

  it("reads a null bound as unbounded", () => {
    const open = [{ min: 1701, max: null, color: "#689735", label: "High" }];
    expect(matchSegment(9_999_999, open)?.label).toBe("High");
    expect(matchSegment(1700, open)).toBeUndefined();

    const openLow = [{ min: null, max: 10, color: "#aaa", label: "Low" }];
    expect(matchSegment(-9_999_999, openLow)?.label).toBe("Low");
  });

  it("drops a band open at BOTH ends instead of letting it swallow every value", () => {
    const bad = [
      { min: null, max: null, color: "#aaa", label: "everything" },
      { min: 0, max: 10, color: "#bbb", label: "real" },
    ];
    expect(matchSegment(5, bad)?.label).toBe("real");
  });

  it("returns undefined for a non-numeric value, no bands, or no match", () => {
    expect(matchSegment("not a number", bands)).toBeUndefined();
    expect(matchSegment(null, bands)).toBeUndefined();
    expect(matchSegment(5, undefined)).toBeUndefined();
    expect(matchSegment(99999, bands)).toBeUndefined();
  });
});

describe("isValidSegment", () => {
  it("requires at least one finite bound", () => {
    expect(isValidSegment({ min: 0, max: null, color: "#a" })).toBe(true);
    expect(isValidSegment({ min: null, max: 0, color: "#a" })).toBe(true);
    expect(isValidSegment({ min: null, max: null, color: "#a" })).toBe(false);
    expect(isValidSegment(null)).toBe(false);
    expect(isValidSegment({ min: NaN, max: null, color: "#a" })).toBe(false);
  });
});

describe("formatSegmentRange", () => {
  it("renders closed and open-ended ranges", () => {
    expect(formatSegmentRange({ min: 1, max: 10, color: "#a" })).toBe("1 - 10");
    expect(formatSegmentRange({ min: 1, max: null, color: "#a" })).toBe("≥ 1");
    expect(formatSegmentRange({ min: null, max: 10, color: "#a" })).toBe("≤ 10");
    expect(formatSegmentRange({ min: null, max: null, color: "#a" })).toBe("");
  });
});

describe("shouldCompact", () => {
  // Thresholds copied from Metabase: len > 6 && (width < 250 || width < len * 25).
  it("leaves a short value alone however narrow the box", () => {
    expect(shouldCompact("123456", 10)).toBe(false); // exactly 6 — not longer than 6
  });

  it("compacts a long value in a narrow box", () => {
    expect(shouldCompact("1234567", 249)).toBe(true);
  });

  it("compacts a long value whose per-digit width exceeds the box", () => {
    // 10 chars × 25 = 250 needed; 240 is less, so compact even though 240 is under 250 anyway.
    // Use a wide box to isolate the per-digit rule from the < 250 rule:
    expect(shouldCompact("12345678901234567890", 400)).toBe(true); // 20 × 25 = 500 > 400
    expect(shouldCompact("1234567", 400)).toBe(false); // 7 × 25 = 175 < 400, and 400 ≥ 250
  });
});

describe("compactifyValue", () => {
  it("returns the full value when it fits", () => {
    const { display, full } = compactifyValue(1234, "double", 800);
    expect(display).toBe("1,234");
    expect(full).toBe("1,234");
  });

  it("compacts when the box is too narrow, and keeps the full value alongside", () => {
    const { display, full } = compactifyValue(3428911, "double", 200);
    expect(full).toBe("3,428,911");
    expect(display).not.toBe(full);
    expect(display).toMatch(/M$/);
  });

  it("honours an explicit compact setting even in a wide box", () => {
    const { display, full } = compactifyValue(3428911, "double", 5000, { compact: true });
    expect(display).toMatch(/M$/);
    expect(full).toBe("3,428,911");
  });

  it("keeps prefix and suffix on both values", () => {
    const { display, full } = compactifyValue(3428911, "double", 200, { prefix: "$" });
    expect(full).toBe("$3,428,911");
    expect(display.startsWith("$")).toBe(true);
  });
});

describe("maxFontSizeRem", () => {
  it("caps a narrow card regardless of its grid share", () => {
    expect(maxFontSizeRem(24, 24, 239)).toBe(NARROW_CARD_MAX_SIZE);
  });

  it("scales with the card's share of the grid", () => {
    // Independently known: a full-width card is the top of the range, and half-width sits halfway
    // between the small and large bounds — 2.2 + 0.5 × (7 − 2.2) = 4.6.
    expect(maxFontSizeRem(24, 24, 800)).toBeCloseTo(MAX_SIZE_LARGE);
    expect(maxFontSizeRem(12, 24, 800)).toBeCloseTo(4.6);
  });

  it("falls back to the large bound when there is no grid to read", () => {
    expect(maxFontSizeRem(undefined, undefined, 800)).toBe(MAX_SIZE_LARGE);
  });
});

describe("findSize", () => {
  /** Width is `len × size × 10`; height is `size × 10`. Chosen so the answer can be computed by
   * hand rather than by re-running the function's own arithmetic. */
  const measurer: TextMeasurer = (text, style) => {
    const size = parseFloat(String(style.size));
    return { width: text.length * size * 10, height: size * 10 };
  };

  const base = {
    unit: "rem" as const, fontFamily: "Inter", fontWeight: 700,
    step: 0.2, min: 1, max: 7, measurer,
  };

  it("steps down to the largest size that fits the width", () => {
    // "abcd" at size s is 4 × s × 10 wide. 100px allows s ≤ 2.5; stepping 7 → 2.6 (104, too wide)
    // → 2.4 (96, fits).
    expect(findSize({ ...base, text: "abcd", targetWidth: 100, targetHeight: 10_000 }))
      .toBe("2.4rem");
  });

  it("is bounded by height as well as width", () => {
    // Height is size × 10, so 30px allows s ≤ 3; the width here is unlimited.
    expect(findSize({ ...base, text: "a", targetWidth: 100_000, targetHeight: 30 })).toBe("3rem");
  });

  it("returns the maximum when everything fits", () => {
    expect(findSize({ ...base, text: "a", targetWidth: 100_000, targetHeight: 100_000 }))
      .toBe("7rem");
  });

  it("bottoms out at the minimum rather than going below it", () => {
    expect(findSize({ ...base, text: "a very long string indeed", targetWidth: 1, targetHeight: 1 }))
      .toBe("1rem");
  });
});
