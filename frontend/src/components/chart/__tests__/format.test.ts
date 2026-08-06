import { describe, it, expect } from "vitest";
import { formatValue } from "../format";
import { isNumericType, isTemporalType } from "../format-types";

describe("type detection across engines", () => {
  it("recognises Trino SQL type names", () => {
    expect(isNumericType("bigint")).toBe(true);
    expect(isNumericType("double")).toBe(true);
    expect(isNumericType("varchar(15)")).toBe(false);
  });
  it("recognises MySQL-protocol enum names from StarRocks", () => {
    expect(isNumericType("MYSQL_TYPE_LONGLONG")).toBe(true);  // bigint
    expect(isNumericType("MYSQL_TYPE_DOUBLE")).toBe(true);
    expect(isNumericType("MYSQL_TYPE_NEWDECIMAL")).toBe(true);
    expect(isNumericType("MYSQL_TYPE_VAR_STRING")).toBe(false);
    expect(isTemporalType("MYSQL_TYPE_DATETIME")).toBe(true);
  });
});

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
  it("keeps the currency symbol when compacting", () => {
    expect(formatValue(1_234_567, "double", { compact: true, number_style: "currency", currency: "USD" }))
      .toBe("$1.2M");
  });
  it("negative in parentheses", () => {
    expect(formatValue(-42, "double", { negative_in_parentheses: true })).toBe("(42)");
  });
  it("custom separators (space thousands, comma decimal)", () => {
    expect(formatValue(1234.5, "double", { number_separators: ", ", decimals: 1 })).toBe("1 234,5");
  });
});

/**
 * THE TWO SEPARATOR STYLES THE PANEL COULD NOT OFFER, AND THE REASON IT COULD NOT.
 *
 * Metabase offers five (`visualizations/lib/settings/column.ts`, `number_separators`); egret offered
 * the first three, and the two missing ones are `100000.00` (`"."`, no thousands separator at all)
 * and `100’000.00` (`".’"`). The second character of the stored string is the THOUSANDS separator,
 * so "no separator" is expressed by a one-character value whose `[dec, thou]` destructure leaves
 * `thou` undefined — which is a shape the option list simply never produced.
 *
 * WHY THE OPTIONS COULD NOT JUST BE ADDED. `applySeparators` did the swap in three passes using a
 * SPACE as its scratch marker: `,`→" ", `.`→dec, " "→thou. That is safe only while no space in the
 * string means anything, and `Intl.NumberFormat`'s `currencyDisplay: "name"` puts real spaces in —
 * `"100,000.00 US dollars"`. Under the EXISTING `100.000,00` option that already drew
 * `100.000,00.US.dollars` on a live bar chart's y-axis (measured in the browser on `MB By Category`
 * with Display = "dollars"), and both new options would have extended the same defect: `"."` maps
 * the marker to the empty string, so it would have drawn `100000.00USdollars`.
 *
 * So `applySeparators` is now a SINGLE pass with a per-character map and no scratch marker — which
 * is, character for character, Metabase's own `replaceNumberSeparators`
 * (`utils/formatting/numbers.tsx:293`), down to `groupingSeparator || ""` carrying the one-character
 * case. The three existing options are unchanged on every input that has no space in it, which is
 * every non-`name` currency display; the fourth test below is the one that was failing.
 *
 * The STORED VALUE stays Metabase's two-character string and stays the thing `formatNumber` reads,
 * so nothing migrates. Note the apostrophe is U+2019 (’), not ASCII — Metabase's option is `".’"`,
 * and storing `".'"` would be a silently different value that no Metabase export would match.
 */
describe("formatValue — the five separator styles, as Metabase spells them", () => {
  const n = 100000;
  it("100,000.00 — the default, and the one that means 'change nothing'", () => {
    expect(formatValue(n, "double", { number_separators: ".,", decimals: 2 })).toBe("100,000.00");
  });
  it("100 000,00", () => {
    expect(formatValue(n, "double", { number_separators: ", ", decimals: 2 })).toBe("100 000,00");
  });
  it("100.000,00", () => {
    expect(formatValue(n, "double", { number_separators: ",.", decimals: 2 })).toBe("100.000,00");
  });
  it("100000.00 — no thousands separator, which nothing could express before", () => {
    expect(formatValue(n, "double", { number_separators: ".", decimals: 2 })).toBe("100000.00");
  });
  it("100’000.00 — U+2019, not an ASCII apostrophe", () => {
    expect(formatValue(n, "double", { number_separators: ".’", decimals: 2 })).toBe("100’000.00");
  });

  // The measured defect. `currencyDisplay: "name"` is the only Display option that puts an ASCII
  // space in the formatted string, and the three-pass swap ate it.
  it("leaves the spaces inside a spelled-out currency name alone", () => {
    const fmt = {
      number_style: "currency", currency: "USD", currency_style: "name", decimals: 2,
    } as const;
    expect(formatValue(n, "double", { ...fmt, number_separators: ",." })).toBe("100.000,00 US dollars");
    expect(formatValue(n, "double", { ...fmt, number_separators: "." })).toBe("100000.00 US dollars");
    expect(formatValue(n, "double", { ...fmt, number_separators: ".’" }))
      .toBe("100’000.00 US dollars");
  });

  // `currencyDisplay: "code"` separates the code from the number with U+00A0, not U+0020, so it
  // never hit the defect above — asserted so a future rewrite of this function cannot start
  // treating that gap as a thousands separator either.
  it("leaves the non-breaking gap after a currency code alone", () => {
    expect(formatValue(n, "double", {
      number_style: "currency", currency: "USD", currency_style: "code", decimals: 2,
      number_separators: ".",
    })).toBe("USD 100000.00");
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

describe("date styles for time buckets", () => {
  // A pivot bucketed by month/quarter/year arrives as the first instant of the bucket, so a full
  // date shows a day that carries no meaning. These are the shapes Metabase labels buckets with.
  it("formats a month bucket as month and year", () => {
    expect(formatValue("2026-01-01", "date", { date_style: "MMMM YYYY" })).toBe("January 2026");
  });
  it("formats a quarter bucket with the Q token", () => {
    expect(formatValue("2026-04-01", "date", { date_style: "[Q]Q YYYY" })).toBe("Q2 2026");
  });
  it("formats a year bucket as the year alone", () => {
    expect(formatValue("2026-01-01", "date", { date_style: "YYYY" })).toBe("2026");
  });
  it("abbreviating a month bucket shortens the month name", () => {
    expect(formatValue("2026-01-01", "date", { date_style: "MMMM YYYY", date_abbreviate: true }))
      .toBe("Jan 2026");
  });
});
