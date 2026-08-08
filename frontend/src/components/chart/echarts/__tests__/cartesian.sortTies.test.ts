import { describe, it, expect } from "vitest";
import { buildCartesianOption, cartesianSeriesNames, sortCategories } from "../options/cartesian";

/**
 * THE TIE, which is the half of #394 and #436 that neither of them closed.
 *
 * Both PRs replaced arrival order with a sort, and both sorts compare on a DERIVED key —
 * `Number(a) - Number(b)` on a numeric axis, `Date.parse(a) - Date.parse(b)` on a temporal one,
 * `localeCompare` on a nominal one. Deriving a key is lossy, so two DISTINCT raw values can produce
 * the SAME key. `distinct` deduplicates on `String(v)` and therefore keeps both; the comparator then
 * returns 0 for them; `Array.prototype.sort` has been stable since ES2019, so a 0 means "leave them
 * in the order you found them" — and the order they were found in is `distinct`'s arrival order,
 * which is the engine's row order, which is the exact nondeterminism both PRs existed to remove.
 * The sort did not fix arrival order for tied values. It only made it harder to see.
 *
 * HOW A TIE IS MADE TO HAPPEN RELIABLY, which is the whole difficulty of testing this. A test that
 * loads the chart repeatedly and hopes the engine reorders its rows is a coin flip that passes on a
 * good day; #436 had to watch SIX page loads because loads 3 and 4 both repeated load 1. So these
 * tests do not sample. They exploit the property that makes the defect a defect: a correct sort is
 * a TOTAL ORDER, and a total order is PERMUTATION-INVARIANT — feed it the same values in any
 * arrival order and it must return the same list. Every case below therefore builds the same chart
 * TWICE from the same rows in two different arrival orders and asserts the two outputs are equal,
 * as well as asserting the one order that is correct. Against the unfixed code the two disagree on
 * every single run, deterministically, with no sampling and no luck. That is a stronger instrument
 * than repeated loads and it is also a faster one.
 *
 * THREE REAL TIES, none of them contrived:
 *
 *  - NUMERIC, `Number(null) === Number(0)`. #436's own example. `Number` maps `null`, `""` and
 *    `false` all onto 0, and a breakout column with a NULL in it beside a genuine zero is an
 *    ordinary result set. Note that the row filter upstream (`rows.filter((r) => r[xi] != null)`)
 *    drops nulls in the CATEGORY column only — the BAND column is never null-filtered, and
 *    `cartesianSeriesNames` documents that it folds a null band to `(empty)` AFTER sorting. So a
 *    null reaching the band sort is not an edge case, it is the designed path.
 *
 *  - TEMPORAL, `Date.parse(String(null))` is NaN. Same null, a worse outcome — see below.
 *
 *  - NOMINAL, Unicode normalisation. `"Café"` written NFC (U+00E9) and `"Café"` written NFD
 *    (`e` + U+0301) are two different strings — `distinct` keeps both — that `localeCompare`
 *    correctly reports as EQUAL, because they are the same text. Mixed NFC/NFD in one column is
 *    what you get when rows arrive from more than one source; macOS filesystems emit NFD and
 *    almost everything else emits NFC.
 *
 * AND THE NaN CASE IS NOT A TIE AT ALL — IT IS WORSE, which is the finding that made this bigger
 * than the brief for it. `Number("n/a")` and `Date.parse("null")` are NaN, so the comparator returns
 * NaN, which is neither negative, zero, nor positive. That is an INCONSISTENT comparator, and the
 * spec leaves the whole result implementation-defined. Measured on V8: twelve months with a single
 * null among them came back
 * `Jan Feb Mar Apr May Jul Sep null Jun Aug Oct Nov` — not "the null landed somewhere odd" but the
 * chronology BROKEN for the values after it, June and August drawn after September. One NULL in a
 * date breakout does not misplace one band; it scrambles the axis. The unparseable values are
 * therefore sorted to the END here rather than left to poison the comparison, which is also where
 * a reader looking for a missing value would think to look.
 *
 * THE TIE-BREAK IS THE RAW STRING, and it is guaranteed to settle every tie rather than merely
 * making them rarer. `distinct` has already deduplicated on exactly `String(v)`, so within any list
 * reaching these comparators the raw strings are pairwise DISTINCT by construction — a code-unit
 * comparison of them can never itself return 0. The order is therefore total, and the last of the
 * three sorts stops being a preference and becomes a fact. Code units rather than `localeCompare`
 * for the tie-break specifically because `localeCompare` is the thing that just tied.
 *
 * WHAT METABASE DOES, since parity is the standing rule: nothing, and that is not an oversight to
 * copy. #436 established that Metabase never sorts breakout bands at all, because its queries carry
 * their own order — `add-implicit-breakout-order-by` puts an ascending `order-by` on every breakout
 * column before the query runs, so the engine returns rows already ordered and the client has
 * nothing left to decide. Ties are settled by the DATABASE's collation, server-side, before the
 * data is ever serialised. egret's stored specs carry `orderby: []`, so there is no server-side
 * order to inherit and the tie has to be settled here. There is no Metabase behaviour to match;
 * there is only the guarantee Metabase gets for free from its query builder, which is that the same
 * data draws the same picture twice. That guarantee is what these tests assert.
 */

/** The same chart from the same rows in two arrival orders. A total order returns one answer. */
const bothWays = <T>(rows: unknown[][], f: (r: unknown[][]) => T): [T, T] => [f(rows), f([...rows].reverse())];

describe("sortCategories settles ties instead of falling back to arrival order", () => {
  it("orders a numeric axis where null and 0 derive the same key", () => {
    // Number(null) === Number(0) === 0. Two distinct values, one sort key.
    const [a, b] = bothWays([[0], [null], [2]], (r) => sortCategories(r.map((x) => x[0]), "bigint"));
    expect(a).toStrictEqual(b);
    expect(a).toStrictEqual([0, null, 2]);
  });

  it("orders a numeric axis where null, empty string and 0 all derive the same key", () => {
    const [a, b] = bothWays([[""], [0], [null]], (r) => sortCategories(r.map((x) => x[0]), "bigint"));
    expect(a).toStrictEqual(b);
    // Raw strings "", "0", "null" in code-unit order.
    expect(a).toStrictEqual(["", 0, null]);
  });

  it("keeps the rest of a numeric axis sorted when one value is not a number", () => {
    // The blast-radius case: Number("n/a") is NaN, the comparator becomes inconsistent, and V8
    // stops sorting the values around it too. 1..11 must still come back ascending.
    const vals = [9, 5, "n/a", 1, 7, 3, 11, 2, 4, 6, 8, 10];
    const [a, b] = bothWays(vals.map((v) => [v]), (r) => sortCategories(r.map((x) => x[0]), "bigint"));
    expect(a).toStrictEqual(b);
    expect(a).toStrictEqual([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, "n/a"]);
  });

  it("keeps a temporal axis chronological when one value is null", () => {
    // Date.parse(String(null)) is NaN. Measured unfixed: Jun/Aug drawn after Sep.
    const vals = [
      "2026-09-01", "2026-05-01", null, "2026-01-01", "2026-07-01", "2026-03-01",
      "2026-11-01", "2026-02-01", "2026-04-01", "2026-06-01", "2026-08-01", "2026-10-01",
    ];
    const [a, b] = bothWays(vals.map((v) => [v]), (r) => sortCategories(r.map((x) => x[0]), "date"));
    expect(a).toStrictEqual(b);
    expect(a).toStrictEqual([
      "2026-01-01", "2026-02-01", "2026-03-01", "2026-04-01", "2026-05-01", "2026-06-01",
      "2026-07-01", "2026-08-01", "2026-09-01", "2026-10-01", "2026-11-01", null,
    ]);
  });

  it("orders a temporal axis where two spellings name the same instant", () => {
    const vals = ["2026-01-01T00:00:00.000Z", "2026-01-01T00:00:00Z"];
    const [a, b] = bothWays(vals.map((v) => [v]), (r) => sortCategories(r.map((x) => x[0]), "timestamp"));
    expect(a).toStrictEqual(b);
  });

  it("orders a nominal axis where two spellings collate equal", () => {
    // NFC vs NFD. Distinct strings, same text, localeCompare returns 0.
    const nfc = "Café";
    const nfd = "Café";
    expect(nfc.localeCompare(nfd)).toBe(0); // the tie, asserted rather than assumed
    expect(nfc).not.toBe(nfd); //             and the two really are different strings
    const [a, b] = bothWays([[nfd], ["Apple"], [nfc]], (r) => sortCategories(r.map((x) => x[0]), "varchar"));
    expect(a).toStrictEqual(b);
    expect(a[0]).toBe("Apple");
  });

  it("leaves the preserve arm alone", () => {
    // The narrow exemption #394 and #436 added: rows already in a meaningful order keep it.
    // Ties are irrelevant here because nothing is compared at all.
    expect(sortCategories(["z", "a"], "varchar", true)).toStrictEqual(["z", "a"]);
  });
});

describe("sortBands settles the same ties on the other axis", () => {
  const build = (cols: { name: string; type: string }[], rows: unknown[][], cfg: Record<string, unknown>) =>
    buildCartesianOption(cols, rows, cfg as never) as unknown as { series: { name: string; __raw?: string }[] };

  it("bands a numeric breakout deterministically when null sits beside a real 0", () => {
    // #436's example, on the axis #436 was about.
    const cols = [{ name: "cat", type: "varchar" }, { name: "band", type: "bigint" }, { name: "rev", type: "double" }];
    const rows = [["x", 0, 1], ["x", null, 2], ["x", 2, 3]];
    const base = { kind: "bar", xAxis: "cat", yMetrics: ["rev"], series: "band" };
    const [a, b] = bothWays(rows, (r) => build(cols, r, base).series.map((s) => s.name));
    expect(a).toStrictEqual(b);
    // The null band renders as "(empty)", substituted after the sort — see cartesianSeriesNames.
    expect(a).toStrictEqual(["0", "(empty)", "2"]);
  });

  it("bands a temporal breakout chronologically when one band is null", () => {
    const cols = [{ name: "cat", type: "varchar" }, { name: "band", type: "date" }, { name: "rev", type: "double" }];
    const rows = [["x", "2026-03-01", 1], ["x", null, 2], ["x", "2026-01-01", 3], ["x", "2026-02-01", 4]];
    const base = { kind: "bar", xAxis: "cat", yMetrics: ["rev"], series: "band" };
    const [a, b] = bothWays(rows, (r) => build(cols, r, base).series.map((s) => s.__raw));
    expect(a).toStrictEqual(b);
    expect(a).toStrictEqual(["2026-01-01", "2026-02-01", "2026-03-01", "(empty)"]);
  });

  it("hands the settings panel the same tie-broken order the chart draws", () => {
    // The panel's list IS what the reorder arrows write back as `seriesOrder`. If the panel and the
    // chart settle a tie differently, the first click stores an order the user never composed.
    //
    // ASSERTED AGAINST THE CONCRETE ORDER AND NOT MERELY AGAINST THE CHART, because the two agree
    // even when both are wrong: they call the same `sortBands` on the same arrival order, so a pure
    // `panel === chart` assertion PASSES against the unfixed builder and proves only that one bug
    // is shared, not that either is right. Checked: written that way this test was green before the
    // fix. The literal below is what makes it an instrument.
    const cols = [{ name: "cat", type: "varchar" }, { name: "band", type: "bigint" }, { name: "rev", type: "double" }];
    const rows = [["x", 0, 1], ["x", null, 2], ["x", 2, 3]];
    const panel = cartesianSeriesNames(cols, rows, { yMetrics: ["rev"], xAxis: "cat", series: "band" });
    const chart = build(cols, rows, { kind: "bar", xAxis: "cat", yMetrics: ["rev"], series: "band" })
      .series.map((s) => s.name);
    expect(panel).toStrictEqual(["0", "(empty)", "2"]);
    expect(panel).toStrictEqual(chart);
  });

  it("still short-circuits on preserveSeriesOrder", () => {
    // #436's guard. The tie-break sits inside the sorted branch and must not reach past it.
    const cols = [{ name: "cat", type: "varchar" }, { name: "band", type: "bigint" }, { name: "rev", type: "double" }];
    const rows = [["x", 2, 1], ["x", 0, 2], ["x", null, 3]];
    const o = build(cols, rows, { kind: "bar", xAxis: "cat", yMetrics: ["rev"], series: "band", preserveSeriesOrder: true });
    expect(o.series.map((s) => s.name)).toStrictEqual(["2", "0", "(empty)"]);
  });
});
