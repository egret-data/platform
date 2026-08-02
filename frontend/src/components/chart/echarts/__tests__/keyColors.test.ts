import { describe, expect, it } from "vitest";
import { colorsForKeys, PREFERRED_COLORS } from "../keyColors";
import { chartShade, chartTint, SERIES_PALETTE } from "../palette";

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

describe("colorsForKeys beyond eight", () => {
  it("extends the harmony rather than repeating an accent", () => {
    const keys = Array.from({ length: 9 }, (_, i) => `n${i}`);
    const out = colorsForKeys(keys);
    expect(new Set(Object.values(out)).size).toBe(9);
  });

  it("interleaves base and dark, so the ninth is accent0's dark", () => {
    // The sankey case, measured live in Metabase: a0, a0-dark, a1, a1-dark, …
    const keys = Array.from({ length: 9 }, (_, i) => `n${i}`);
    const out = colorsForKeys(keys);
    expect(Object.values(out)).toContain(chartShade(SERIES_PALETTE[0]));
  });

  it("brings in the light variants only past sixteen keys", () => {
    const under = colorsForKeys(Array.from({ length: 16 }, (_, i) => `n${i}`));
    expect(Object.values(under)).not.toContain(chartTint(SERIES_PALETTE[0]));
    const over = colorsForKeys(Array.from({ length: 17 }, (_, i) => `n${i}`));
    expect(Object.values(over)).toContain(chartTint(SERIES_PALETTE[0]));
  });

  it("assigns in key order past eight, without hashing", () => {
    const keys = ["z", "y", "x", "w", "v", "u", "t", "s", "r"];
    const out = colorsForKeys(keys);
    expect(out.z).toBe(SERIES_PALETTE[0]);
    expect(out.y).toBe(chartShade(SERIES_PALETTE[0]));
  });

  it("still lets a semantic name and an explicit colour win", () => {
    const keys = ["profit", ...Array.from({ length: 9 }, (_, i) => `n${i}`)];
    const out = colorsForKeys(keys, { n0: SERIES_PALETTE[5] });
    expect(out.profit).toBe(PREFERRED_COLORS.success);
    expect(out.n0).toBe(SERIES_PALETTE[5]);
  });

  // The order path's counterpart to "terminates when there are more keys than colours". It cannot
  // hang — it visits each key once — so the failure mode is quieter and worse: the tail of the
  // series list comes back with NO entry at all, and every caller reads `undefined` as a colour.
  // Twenty-four is the whole harmony (three ranges × eight accents), so the 25th is the first key
  // there is nothing left for.
  it("still colours a key past the end of the harmony, by starting it over", () => {
    const keys = Array.from({ length: 25 }, (_, i) => `n${i}`);
    const out = colorsForKeys(keys);
    expect(Object.keys(out)).toHaveLength(25);
    keys.forEach((k) => expect(out[k]).toBeTruthy());
    expect(out.n24).toBe(SERIES_PALETTE[0]); // the harmony restarts at its own head
  });

  // The same live bug the hash path has already been fixed for (see `canon`), on the order path:
  // the settings panels hand back whatever a native `<input type="color">` lower-cased, so a pinned
  // colour reaches here spelled differently from the palette entry it IS. If the order pass compares
  // raw strings the pin does not consume its slot and the very next key is handed the same ink.
  it("consumes its harmony slot even when the picker lower-cased the colour", () => {
    const keys = Array.from({ length: 9 }, (_, i) => `n${i}`);
    const picked = SERIES_PALETTE[0].toLowerCase(); // what the DOM gives back for "#509EE3"
    const out = colorsForKeys(keys, { n8: picked });

    expect(out.n8).toBe(picked); // the caller's own casing comes back untouched
    const inks = Object.values(out).map((c) => c.toLowerCase());
    expect(new Set(inks).size).toBe(9); // and nobody else was given that same ink
  });
});
