import { describe, it, expect } from "vitest";
import { formatValue } from "../format";

describe("formatValue numbers", () => {
  it("null/empty → empty string", () => {
    expect(formatValue(null, "double")).toBe("");
    expect(formatValue(undefined, "double")).toBe("");
  });
  it("default numeric uses locale grouping", () => {
    expect(formatValue(1234567, "bigint")).toBe("1,234,567");
  });
  it("coerces string-encoded numerics", () => {
    expect(formatValue("1234.5", "double")).toBe("1,234.5");
  });
  it("decimals forces fraction digits", () => {
    expect(formatValue(3, "double", { decimals: 2 })).toBe("3.00");
  });
  it("percent style", () => {
    expect(formatValue(0.125, "double", { number_style: "percent", decimals: 1 })).toBe("12.5%");
  });
  it("currency style with code", () => {
    expect(formatValue(1234.5, "double", { number_style: "currency", currency: "USD", currency_style: "code", decimals: 2 })).toBe("USD 1,234.50");
  });
  it("scale multiplies before formatting", () => {
    expect(formatValue(2, "double", { scale: 1000 })).toBe("2,000");
  });
  it("prefix + suffix", () => {
    expect(formatValue(5, "double", { prefix: "~", suffix: " pts" })).toBe("~5 pts");
  });
  it("compact notation", () => {
    expect(formatValue(1500000, "double", { compact: true })).toBe("1.5M");
  });
  it("negative in parentheses", () => {
    expect(formatValue(-42, "double", { negative_in_parentheses: true })).toBe("(42)");
  });
  it("custom separators (space thousands, comma decimal)", () => {
    expect(formatValue(1234.5, "double", { number_separators: ", ", decimals: 1 })).toBe("1 234,5");
  });
});

describe("formatValue dates", () => {
  it("default date style", () => {
    expect(formatValue("2024-01-31", "date")).toBe("January 31, 2024");
  });
  it("abbreviated + custom separator", () => {
    expect(formatValue("2024-01-31", "date", { date_style: "M/D/YYYY", date_separator: "-" })).toBe("1-31-2024");
  });
  it("invalid date → string passthrough", () => {
    expect(formatValue("not-a-date", "date")).toBe("not-a-date");
  });
});

describe("formatValue other", () => {
  it("text passthrough", () => {
    expect(formatValue("hello", "varchar")).toBe("hello");
  });
});
