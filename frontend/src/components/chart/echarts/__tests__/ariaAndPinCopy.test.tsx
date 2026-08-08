import { describe, it, expect } from "vitest";
import { render } from "@testing-library/react";
import { buildCartesianOption } from "../options/cartesian";
import { buildBoxplotOption } from "../options/boxplot";
import { pinCoverage, pinNote, pinProblem, type PinCoverage } from "../options/map";
import { SmartScalarChart } from "../../SmartScalarChart";

/**
 * THE FOUR POLISH FINDINGS from the same live sweep — a row chart that called itself a bar chart, a
 * box plot that read four bare numbers, a pin map whose refusal overclaimed, and a trend arrow that
 * wrapped away from its own number.
 *
 * Three of the four are pure string questions and are fully pinned here. The fourth (the arrow) is
 * a LAYOUT defect, and jsdom runs no layout — so what is asserted is the DOM shape that makes the
 * layout impossible rather than the layout itself, and the browser measurement that motivated it is
 * quoted where the assertion sits.
 */

describe("P8 — a row chart's description calls itself a row chart", () => {
  /**
   * The UI names this type "Row" in the chart-type picker, in the settings panel and in the saved
   * `display`; only the screen reader was told "Bar chart". `kind: "row"` is a first-class member of
   * `CartesianViz` — a bar chart merely TURNED horizontal keeps `kind: "bar"` and is still a bar
   * chart, which `isHorizontal` is careful to distinguish and which is why this reads the kind and
   * not the orientation.
   */
  const cols = [{ name: "category", type: "varchar" }, { name: "revenue", type: "double" }];
  const rows = [["Doohickey", 10], ["Gadget", 20]];
  const describe1 = (kind: string, extra: Record<string, unknown> = {}) =>
    ((buildCartesianOption(cols, rows, { kind, xAxis: "category", yMetrics: ["revenue"], ...extra } as never) as never as
      { aria?: { label?: { description?: string } } }).aria?.label?.description ?? "");

  it("says Row chart", () => {
    expect(describe1("row")).toMatch(/^Row chart of revenue by category\./);
  });

  it("still says Bar chart for a bar merely turned horizontal, which is a different setting", () => {
    expect(describe1("bar", { horizontal: true })).toMatch(/^Bar chart of revenue by category\./);
  });
});

describe("P7 — a box plot says its numbers are medians, and describes the spread", () => {
  /**
   * It announced "Box plot of price by category. 4 values: Doohickey 50.06, …". Every number was a
   * MEDIAN and nothing said so, so the sentence invited a reader to take it for the category's
   * value; and the quartiles and whiskers — the reason to draw a box rather than a bar — were not
   * described at all, while a sighted reader gets all five from the shape and the tooltip.
   */
  const cols = [{ name: "category", type: "varchar" }, { name: "price", type: "double" }];
  const rows = [
    ["A", 10], ["A", 20], ["A", 30], ["A", 40], ["A", 50],
    ["B", 100], ["B", 200], ["B", 300], ["B", 400], ["B", 500],
  ];
  const built = () =>
    (buildBoxplotOption(cols, rows, { category: "category", value: "price" } as never) as never as
      { aria?: { label?: { description?: string } } }).aria?.label?.description ?? "";

  it("counts boxes rather than values", () => {
    expect(built()).toContain("2 boxes:");
    expect(built()).not.toContain("2 values:");
  });

  it("names the median and states the four numbers around it", () => {
    const d = built();
    expect(d).toMatch(/A 30 \(median; min 10, Q1 \S+, Q3 \S+, max 50\)/);
    expect(d).toMatch(/B 300 \(median; min 100, Q1 \S+, Q3 \S+, max 500\)/);
  });
});

describe("P9 — a pin map only refuses when it has drawn nothing", () => {
  /**
   * MEASURED on the harness's pin map: 48 rows, 10 off the US states map and 38 with coordinates out
   * of range, and the card was replaced whole by "None of these 48 rows can be shown: 10 are outside
   * the US states map and 38 have a latitude or longitude out of range. Check whether the Latitude
   * and Longitude columns are the other way round. Try a map that covers them."
   *
   * Every count reconciled and the grammar was right, and it was still false of ten rows. #438 chose
   * to KEEP off-map pins precisely because the geo component roams and panning reaches them — so
   * those ten were drawable, and the sentence claiming otherwise had removed the map they would have
   * been drawn on. It also offered two remedies at once, aimed at different rows.
   *
   * The distinction restored here is #438's own: off-map is recoverable, out-of-range is not.
   */
  /** `total` is DERIVED, never passed: the four buckets partition the rows by construction
   *  (`pinCoverage`'s own contract), and a fixture free to disagree with its own parts would let an
   *  assertion pass against a coverage the counter can never produce. */
  const cov = (p: Omit<Partial<PinCoverage>, "total">): PinCoverage => ({
    plotted: 0, offMap: 0, outOfRange: 0, missing: 0, ...p,
    total: (p.plotted ?? 0) + (p.offMap ?? 0) + (p.outOfRange ?? 0) + (p.missing ?? 0),
  });

  it("does not refuse a result whose only drawn pins are off the map", () => {
    const c = cov({ offMap: 10, outOfRange: 38 });
    expect(pinProblem(c, "US states")).toBeUndefined();
  });

  it("notes that nothing is in view and says how to reach what is drawn", () => {
    const c = cov({ offMap: 10, outOfRange: 38 });
    expect(pinNote(c, "US states")).toBe(
      "Showing 0 of 48 rows: 10 are outside the US states map and 38 have a latitude or longitude out of range."
      + " Zoom out or pan the map to reach them.",
    );
  });

  it("still refuses when not one pin was drawn, and then offers a single remedy", () => {
    const c = cov({ outOfRange: 3 });
    expect(pinProblem(c, "US states")).toBe(
      "None of these 3 rows can be shown: 3 have a latitude or longitude out of range."
      + " Check whether the Latitude and Longitude columns are the other way round.",
    );
  });

  it("never offers 'Try a map that covers them' beside the transposition hint again", () => {
    // The two used to be appended together whenever a refusal had both buckets. That pairing is now
    // unreachable rather than merely unlikely: a refusal implies nothing was drawn, which implies
    // `offMap` is zero, which is the only branch that ever produced the second hint.
    for (const c of [cov({ outOfRange: 5 }), cov({ missing: 5 }), cov({ outOfRange: 2, missing: 3 })]) {
      expect(pinProblem(c, "US states") ?? "").not.toContain("Try a map that covers them");
    }
  });

  it("keeps problem and note mutually exclusive over every partition of 4 rows", () => {
    // The two presentations must still never both appear, and must still never both be silent on a
    // result that has rows — the invariant MapChart.tsx relies on to pick one.
    const cols = [{ name: "lat", type: "double" }, { name: "lon", type: "double" }];
    for (let plotted = 0; plotted <= 4; plotted++) {
      for (let offMap = 0; offMap + plotted <= 4; offMap++) {
        for (let outOfRange = 0; outOfRange + offMap + plotted <= 4; outOfRange++) {
          const missing = 4 - plotted - offMap - outOfRange;
          const c = { total: 4, plotted, offMap, outOfRange, missing };
          const both = pinProblem(c, "US states") != null && pinNote(c, "US states") != null;
          expect(both).toBe(false);
          // Something is always said unless every row landed in view.
          const silent = pinProblem(c, "US states") == null && pinNote(c, "US states") == null;
          expect(silent).toBe(plotted === 4);
        }
      }
    }
    // …and the fixture the buckets are really counted from still partitions the way the invariant
    // above assumes, so this is not a test of a hand-built record only.
    const c = pinCoverage(cols, [[10, -100], [1000, -100]], { type: "pin", latitude: "lat", longitude: "lon" } as never);
    expect(c.plotted + c.offMap + c.outOfRange + c.missing).toBe(c.total);
  });
});

describe("P6 — the trend's arrow and its number are one text node", () => {
  /**
   * MEASURED at 209px on the harness's trend card, and the measurement is the whole reason this
   * assertion is shaped the way it is. "▲" rendered at y=1398 and "9.54%" at y=1418 — twenty pixels
   * apart, on two lines — so the arrow sat beside "vs previous month:" and read as that caption's
   * punctuation rather than as the direction of its own number.
   *
   * A NO-BREAK SPACE WAS ALREADY THERE AND WAS NOT WORKING, which is the part worth recording: the
   * glyph, the no-break space and the number were three JSX expressions, so React emitted three adjacent
   * DOM Text nodes, and the span around them is `display: flex`. Flexbox wraps contiguous text runs
   * in anonymous flex items and then does not render an anonymous item that is only white space —
   * so the nbsp became an item of its own and was dropped (it reported ZERO client rects in the
   * browser), leaving the glyph and the number as two independent flex items free to land on
   * different lines. The fix is one interpolated string.
   *
   * jsdom CANNOT SEE ANY OF THAT: it has no layout, no line boxes and no flex algorithm, so it
   * cannot observe the two y values or the dropped item. What it CAN see is the cause — that the
   * span now holds a single text node carrying both — and a single text node cannot be split into
   * two flex items by any layout, which is the property the browser measurement was really about.
   */
  const cols = [{ name: "mo", type: "varchar" }, { name: "rev", type: "double" }];

  it("puts the glyph, a no-break space and the percentage in one node", () => {
    const { container } = render(
      <SmartScalarChart columns={cols} rows={[["Jan", 100], ["Feb", 120]]} dimension="mo" metric="rev" format={{ decimals: 0 }} />,
    );
    const trend = container.querySelector("[data-trend]");
    const texts = [...(trend?.childNodes ?? [])].filter((n) => n.nodeType === 3);
    expect(texts).toHaveLength(1);
    expect(texts[0].textContent).toBe("▲\u00A020.00%");
  });

  it("does the same for a decrease", () => {
    const { container } = render(
      <SmartScalarChart columns={cols} rows={[["Jan", 100], ["Feb", 80]]} dimension="mo" metric="rev" format={{ decimals: 0 }} />,
    );
    const texts = [...(container.querySelector("[data-trend]")?.childNodes ?? [])].filter((n) => n.nodeType === 3);
    expect(texts).toHaveLength(1);
    expect(texts[0].textContent).toBe("▼\u00A020.00%");
  });

  it("lets the comparison wrap between its two spans rather than inside them", () => {
    // The other half of the layout answer, and the only part of it a class assertion can carry: the
    // row was `nowrap`, so the only place it could give was inside each span. `flex-wrap` moves the
    // break to the gap between them. Asserted as a class because that is genuinely all this is —
    // and unlike the arrow, nothing about correctness depends on it, only on where the line falls.
    const { container } = render(
      <SmartScalarChart columns={cols} rows={[["Jan", 100], ["Feb", 120]]} dimension="mo" metric="rev" format={{ decimals: 0 }} />,
    );
    const row = container.querySelector("[data-trend]")?.parentElement;
    expect(row?.className).toContain("flex-wrap");
  });
});
