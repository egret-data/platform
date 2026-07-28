import { describe, it, expect } from "vitest";
import { compactNumber } from "../format";

describe("compactNumber", () => {
  it("shortens thousands, millions and billions", () => {
    expect(compactNumber(25468)).toBe("25.5K");
    expect(compactNumber(1_250_000)).toBe("1.3M");
    expect(compactNumber(3_400_000_000)).toBe("3.4B");
  });

  it("leaves small numbers alone", () => {
    expect(compactNumber(0)).toBe("0");
    expect(compactNumber(999)).toBe("999");
    expect(compactNumber(-42)).toBe("-42");
  });

  it("keeps the sign on shortened negatives", () => {
    expect(compactNumber(-25468)).toBe("-25.5K");
  });

  it("drops a trailing .0", () => {
    expect(compactNumber(2000)).toBe("2K");
  });

  it("passes non-finite values through as an empty string", () => {
    expect(compactNumber(NaN)).toBe("");
    expect(compactNumber(Infinity)).toBe("");
  });

  it("rounds to the next unit when the value wraps", () => {
    expect(compactNumber(999_999)).toBe("1M");
  });

  it("handles trillions when notation >= 1e12", () => {
    expect(compactNumber(1e12)).toBe("1T");
  });

  it("rounds decimals for sub-100 compacted values", () => {
    expect(compactNumber(45.6789012)).toBe("45.7");
  });

  it("applies separators when provided (compacted)", () => {
    expect(compactNumber(1234.5, ", ")).toBe("1,2K");
  });

  it("applies separators to decimal point in compact result", () => {
    expect(compactNumber(45.6789012, ", ")).toBe("45,7");
  });
});
