import { describe, it, expect } from "vitest";
import { CURRENCIES } from "../currencies";
import { formatValue } from "../format";

/**
 * EVERY CURRENCY THE PICKER OFFERS HAS TO SURVIVE BEING PICKED.
 *
 * This is not a list-integrity test dressed up. `formatNumber` hands `fmt.currency` to
 * `Intl.NumberFormat` as `opts.currency`, which throws a `RangeError` on a code it does not know —
 * and `format.ts`'s `safeDecimals` records at length what that throw costs: it fires during the
 * OPTION BUILD, inside React's render, so `ChartErrorBoundary` replaces the whole card with the
 * error text rather than the chart drawing a wrong number, and it does so on every load of a chart
 * saved that way. The old free-text box could produce that state from a typo. A picker can produce
 * it only if the LIST contains a code Intl rejects, so the list is what has to be proven.
 *
 * Asserted by formatting a number through each one and reading the OUTPUT — what a reader would see
 * on an axis — rather than by counting entries or by checking the codes look like three capitals.
 * `expect(CURRENCIES).toHaveLength(122)` would pass against a list of 122 copies of USD.
 */
describe("the vendored currency list", () => {
  it("draws a number for every code it offers, throwing on none of them", () => {
    const drawn = CURRENCIES.map((c) => {
      // Through `formatValue`, i.e. the exact call the chart makes, not a bare Intl call — the
      // options the formatter builds are part of what could throw.
      const out = formatValue(1234.5, "double", {
        number_style: "currency", currency: c.code, currency_style: "code", decimals: 2,
      });
      return [c.code, out] as const;
    });
    // Every one produced text, and every one carried its own code into that text — which is what
    // rules out a silent fallback to the default currency.
    const wrong = drawn.filter(([code, out]) => !out.includes(code) || !out.includes("1,234.50"));
    expect(wrong).toStrictEqual([]);
  });

  it("draws the symbol Metabase's Display option promises, for the ones a reader recognises", () => {
    const symbol = (code: string) =>
      formatValue(100, "double", { number_style: "currency", currency: code, decimals: 0 });
    expect(symbol("USD")).toBe("$100");
    expect(symbol("EUR")).toBe("€100");
    expect(symbol("GBP")).toBe("£100");
    expect(symbol("VND")).toBe("₫100");
  });

  it("carries the names a reader would search for, spelled Metabase's way", () => {
    const byCode = Object.fromEntries(CURRENCIES.map((c) => [c.code, c.name]));
    expect(byCode.USD).toBe("US Dollar");
    expect(byCode.VND).toBe("Vietnamese Dong");
    // The two that `Intl.supportedValuesOf("currency")` does not return and
    // `Intl.DisplayNames` has no name for — the concrete reason `currencies.ts` is vendored
    // rather than derived. Both format fine.
    expect(byCode.BTC).toBe("Bitcoin");
    expect(byCode.ETH).toBe("Ethereum");
  });

  it("names each code once, so the picker cannot show two rows that store the same value", () => {
    expect(new Set(CURRENCIES.map((c) => c.code)).size).toBe(CURRENCIES.length);
  });

  // Metabase's file leads with the three it expects to be picked most and is alphabetical by code
  // after that. The picker shows the list in this order, so the order is part of the parity.
  it("keeps the reference's order, common currencies first", () => {
    expect(CURRENCIES.slice(0, 3).map((c) => c.code)).toStrictEqual(["USD", "CAD", "EUR"]);
  });
});
