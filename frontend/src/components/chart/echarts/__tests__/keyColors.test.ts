import { describe, expect, it } from "vitest";
import { colorsForKeys, PREFERRED_COLORS } from "../keyColors";
import { SERIES_PALETTE } from "../palette";

describe("colorsForKeys", () => {
  it("gives every key a distinct accent when there are no more than eight", () => {
    const keys = ["alpha", "beta", "gamma", "delta"];
    const out = colorsForKeys(keys);
    expect(Object.keys(out).sort()).toEqual([...keys].sort());
    expect(new Set(Object.values(out)).size).toBe(4);
    Object.values(out).forEach((c) => expect(SERIES_PALETTE).toContain(c));
  });

  it("keys a colour to the NAME, so order cannot change it", () => {
    const a = colorsForKeys(["revenue", "orders", "profit"]);
    const b = colorsForKeys(["profit", "revenue", "orders"]);
    expect(b).toEqual(a);
  });

  // The test above is order-independent for the easy reason as well as the hard one: "revenue" and
  // "orders" hash to different accents (slots 4 and 3), so nothing has to be arbitrated and it
  // passes even with the sort removed. "beta" and "delta" BOTH hash to slot 0, so which of them
  // keeps the blue and which is walked on to the next accent is decided purely by iteration order —
  // and that is the case the sort exists for.
  it("keys a colour to the NAME even when two names want the same one", () => {
    expect(colorsForKeys(["delta", "beta"])).toEqual(colorsForKeys(["beta", "delta"]));
  });

  it("gives a semantic name its own colour ahead of the hash", () => {
    const out = colorsForKeys(["profit", "loss", "widgets"]);
    expect(out.profit).toBe(PREFERRED_COLORS.success);
    expect(out.loss).toBe(PREFERRED_COLORS.error);
  });

  it("matches on case, not on exact spelling", () => {
    expect(colorsForKeys(["Completed"]).Completed).toBe(PREFERRED_COLORS.success);
  });

  it("lets an explicit colour win, and does not hand it to anyone else", () => {
    const out = colorsForKeys(["a", "b", "c"], { a: SERIES_PALETTE[2] });
    expect(out.a).toBe(SERIES_PALETTE[2]);
    expect(out.b).not.toBe(SERIES_PALETTE[2]);
    expect(out.c).not.toBe(SERIES_PALETTE[2]);
  });

  // The colour a user "picks" is usually the one already in the swatch: both settings panels
  // default a native `<input type="color">` to a raw SERIES_PALETTE entry (SeriesListSection,
  // PieDisplaySection) and write `e.target.value` straight back. Per the HTML spec that value is
  // always normalised to LOWER-case, while SERIES_PALETTE is written in upper — so simply accepting
  // the default swatch stores a colour that is the same ink as a palette entry but not the same
  // string. If the ledger compares those as strings, the pinned series does not consume its slot
  // and the hash pass hands the identical colour to somebody else.
  it("consumes its palette slot even when the picker lower-cased the colour", () => {
    const picked = SERIES_PALETTE[5].toLowerCase(); // what the DOM gives back for "#F2A86F"
    const out = colorsForKeys(["pinned", "a", "b", "c", "d", "e", "f", "g"], { pinned: picked });

    expect(out.pinned).toBe(picked); // the caller's own casing comes back untouched
    const inks = Object.values(out).map((c) => c.toLowerCase());
    expect(new Set(inks).size).toBe(8); // and nobody else was given that same ink
  });

  it("takes the success colour from the summarize green, byte for byte", () => {
    // palm[50] = hsla(89, 48%, 40%). If this ever drifts, one of the two is wrong.
    expect(PREFERRED_COLORS.success).toBe("#689735");
  });

  it("terminates when there are more keys than colours in the hash path", () => {
    // Eight is the boundary: still the hash path, and every accent is spent.
    const keys = Array.from({ length: 8 }, (_, i) => `k${i}`);
    expect(Object.keys(colorsForKeys(keys))).toHaveLength(8);
  });
});
