import { describe, it, expect } from "vitest";
import * as echarts from "echarts/core";
import { platformApi } from "zrender/lib/core/platform.js";
import { registerEChartsModules } from "../register";
import { buildPieOption } from "../options/pie";

/**
 * WHAT A PIE'S SLICE LABELS REALLY OCCUPY, read back out of the chart ECharts draws.
 *
 * THE DEFECT, measured in the browser on the live 166x129 dashboard card (revenue by category):
 * four labels drawn, two of them through each other —
 *
 *     Doohickey   x 34-106   y 38-55
 *     Widget      x 76-126   y 42-59
 *
 * 30px of shared ink horizontally, 13px vertically, and BOTH drawn. The values below are that
 * card's own shares, recovered from the arc endpoints it drew, and the option this file builds
 * from them puts every label within a tenth of a pixel of where the live card put it
 * (`translate(100.4151 50.1353)`, `(94.4859 83.9346)`, `(62.7947 74.5685)`, `(69.9158 46.1034)`).
 *
 * WHY IT HAPPENED — two mechanisms, both of which had to be checked rather than assumed, because
 * the builder already contained the code that should have prevented each:
 *
 *   1. `buildPieOption` measures every label against its own slice (`fitSliceLabel`), EXCEPT when
 *      the pie has no donut hole: `calcAvailableDonutSliceLabelLength` is a donut formula whose
 *      inner chord degenerates to a point at radius 0, so that case was given `Infinity` — no fit
 *      at all, every label drawn in full whatever its slice measured. egret's `donut: false` pie
 *      is a shape Metabase does not have (its own single-ring pie still keeps a 3/5 hole), so
 *      there was nothing to port and the case was waived instead.
 *
 *   2. `labelLayout: { hideOverlap: true }`, the documented backstop, WAS NOT RUNNING IN THE
 *      SHIPPED APP when this file was written. echarts installs that feature as a top-level
 *      `use(installLabelLayout)` in `lib/export/core.js`; the package's `sideEffects` allowlist
 *      does not name that file, so Rollup dropped the call from a production build and the live
 *      bundle contained no LabelManager at all, while a dev server and Vitest — neither of which
 *      tree-shakes — ran it happily.
 *
 *      THAT IS NO LONGER TRUE: `register.ts` now lists `LabelLayout` explicitly, and
 *      `scripts/assert-label-layout-bundled.mjs` fails CI if it ever stops reaching the bundle.
 *      The backstop runs in the shipped app.
 *
 * WHICH IS WHY THIS FILE STRIPS `labelLayout` BEFORE RENDERING (see `asShipped`) — AND WHY IT
 * STILL SHOULD, for a reason that has outlived the bug above.
 *
 * What is measured here is the pie's OWN geometric fit, in isolation: that `sliceLabel.ts` decides
 * each label against its own slice's chord, so that nothing has to be tidied up afterwards. Left
 * in, `hideOverlap` would satisfy every overlap assertion below on its own, and the case that
 * pins the defect (`drew four labels at 166x129, two of them through each other`) could not be
 * written at all — a builder that had thrown the geometric fit away entirely would pass this file
 * end to end. Stripping the key is what keeps these cases falsifiable, and it states the standard
 * the pie is held to: place your own labels, rather than draw them and hope something downstream
 * tidies up.
 *
 * It is also why enabling the backstop changed nothing on a pie. Measured across five sizes, in
 * every configuration including two rings: not one slice label disappears, because the geometry
 * has already removed everything that would have collided. The control that proves the mechanism
 * really does reach these labels — build with `size: undefined`, so the geometric fit is skipped,
 * and `hideOverlap` drops two to five of them.
 *
 * NO PIXEL IS ASSERTED, and no placement is recomputed here: where each label lands is ECharts'
 * own sunburst layout, read out of the markup. The one fabricated input is the font metric,
 * fabricated identically on both sides — the builder is handed ZRENDER'S OWN per-glyph estimator,
 * which is exactly what lays this SVG out when there is no canvas (jsdom has none). What is
 * asserted is SHARED INK between two drawn labels, and which labels survive at a given size.
 */

/** Zrender's own text estimator, in the shape `buildPieOption` takes a measurer in. */
const measure = (text: string, f: { size: number; family?: string; weight?: number | string }) =>
  platformApi.measureText(text, `${f.weight ?? 400} ${f.size}px ${f.family ?? "sans-serif"}`).width;

/** The pie's single-ring slice label size — `buildPieOption`'s `sliceFontSize`. Used to tell a
 *  slice label apart from the hole's readout, which is drawn much larger. */
const SLICE_FONT_SIZE = 14;

const cols = [{ name: "cat", type: "varchar" }, { name: "rev", type: "double" }];

/**
 * THE LIVE CARD'S OWN SHARES — Widget 28.06%, Gadget 26.92%, Gizmo 25.35%, Doohickey 19.68% —
 * recovered from the four arcs it drew.
 *
 * WHY THIS FIXTURE CANNOT HAND A WRONG IMPLEMENTATION THE RIGHT ANSWER. Four slices, four
 * different arcs, four names of three different widths, and the pairing between them is the
 * point: the WIDEST name (`Doohickey`, 65.5px) sits on the NARROWEST slice (70.9°), so a fit that
 * consults only the name and one that consults only the arc agree here. They are pulled apart by
 * `INVERTED` below, where the same four names are dealt the opposite way round.
 */
const LIVE_SHARES: [string, number][] = [
  ["Widget", 280.6], ["Gadget", 269.2], ["Gizmo", 253.5], ["Doohickey", 196.8],
];

/**
 * The same four names, dealt so that the LONGEST sits on the WIDEST slice (Doohickey, 162°) and
 * the SHORTEST on the narrowest (Gizmo, 36°) — the mirror of the live card, and the fixture that
 * makes "measure the label against ITS OWN slice" a falsifiable claim rather than a description.
 *
 * At one size (see the `each slice is decided on its own geometry` case) these two fixtures
 * disagree about every name: what the live one truncates, this one writes in full, and what the
 * live one writes in full, this one drops. No rule that reads only the name, only the card, or
 * only "the widest label" can satisfy both.
 */
const INVERTED: [string, number][] = [
  ["Doohickey", 45], ["Widget", 25], ["Gadget", 20], ["Gizmo", 10],
];

/**
 * One slice bigger than a semicircle (252°) and three small ones — the shape neither fixture above
 * has, and the only one that can tell a capped arc from an uncapped one.
 *
 * A chord's length is NOT monotonic in its angle: it grows to the diameter at 180° and shrinks
 * again beyond it, so the chord of a 252° arc is SHORTER than the chord of a 180° one at the same
 * radius. Charged uncapped, a dominant slice is therefore billed for less room than the smallest
 * slice that spans a half turn — Metabase caps at π for exactly this reason, and with four slices
 * of a similar size nothing ever reaches the cap to notice.
 */
const DOMINANT: [string, number][] = [
  ["Doohickey", 70], ["Widget", 12], ["Gadget", 10], ["Gizmo", 8],
];

function build(rows: [string, number][], size: { width: number; height: number }) {
  return buildPieOption(
    // `showLabels: true` — this whole file is about WHERE a slice label goes and how much of it
    // fits, and a one-ring pie's labels are off by default now (`resolveShowLabels`). Asking for
    // them is what puts the subject of these tests on the canvas; nothing here tests the default.
    cols, rows.map(([n, v]) => [n, v]), { dimensions: ["cat"], metric: "rev", showLabels: true }, measure, size,
  ) as unknown as Record<string, unknown>;
}

/**
 * The option with the post-layout backstop removed, so what is rendered below is the pie's own
 * geometric fit and nothing else.
 *
 * This used to be a claim about the build — the feature was tree-shaken out and stripping the key
 * was how this file rendered what a reader's browser rendered. It no longer is: `register.ts`
 * lists `LabelLayout` and the backstop ships. The strip stays because of what this file measures,
 * not because of what the bundle contains; see the header.
 */
function asShipped(option: Record<string, unknown>): Record<string, unknown> {
  const series = (option.series as Record<string, unknown>[]).map(({ labelLayout: _dropped, ...rest }) => rest);
  return { ...option, series };
}

function render(option: Record<string, unknown>, size: { width: number; height: number }): string {
  registerEChartsModules();
  const chart = echarts.init(null, null, { renderer: "svg", ssr: true, ...size });
  chart.setOption({ ...asShipped(option), animation: false });
  const svg = chart.renderToSVGString();
  chart.dispose();
  return svg;
}

interface Box { text: string; x0: number; x1: number; y0: number; y1: number }

/**
 * The slice labels the pie DREW, each with the band of card its ink occupies.
 *
 * Every one is emitted with `text-anchor="middle"` and `dominant-baseline="central"`, so the
 * `translate` is the text's CENTRE in both directions and the box is half a width either side and
 * half a line above and below. A dropped label is not absent from the markup — ECharts'
 * `SunburstPiece._updateLabel` does `text = text || this.node.name`, so the builder emits a single
 * space for one it decided against (its own comment says so) — and a space is what the blank
 * filter here removes.
 */
function sliceLabels(svg: string): Box[] {
  const family = /font-family:([^;"]+)/.exec(svg)?.[1] ?? "sans-serif";
  return [...svg.matchAll(/<text\b([^>]*)>([^<]*)<\/text>/g)]
    .map(([, attrs, text]) => ({
      text,
      x: Number(/transform="translate\((-?[\d.]+) (-?[\d.]+)\)"/.exec(attrs)?.[1] ?? NaN),
      y: Number(/transform="translate\((-?[\d.]+) (-?[\d.]+)\)"/.exec(attrs)?.[2] ?? NaN),
      fontSize: Number(/font-size:([\d.]+)px/.exec(attrs)?.[1] ?? 0),
    }))
    .filter((t) => t.fontSize === SLICE_FONT_SIZE && t.text.trim() !== "")
    .map((t) => {
      const w = platformApi.measureText(t.text, `700 ${t.fontSize}px ${family}`).width;
      return {
        text: t.text,
        x0: t.x - w / 2, x1: t.x + w / 2,
        y0: t.y - t.fontSize / 2, y1: t.y + t.fontSize / 2,
      };
    });
}

/**
 * Every pair of drawn labels that shares ink, named so a failure says which two.
 *
 * EVERY PAIR, not just neighbouring slices: two labels sit at the same radius round a circle, and
 * the pair that collides on a small pie is whichever two happen to be anchored near each other on
 * the page — on the live card it is the first slice and the last, which are not neighbours in the
 * data at all.
 *
 * The bar is SHARED INK, deliberately, and not a clearance: this asserts the defect that was
 * measured (two labels drawn over one another) and nothing beyond it, so it stays honest whatever
 * mechanism keeps them apart.
 */
function overlappingPairs(boxes: Box[]): string[] {
  const bad: string[] = [];
  for (let i = 0; i < boxes.length; i++) {
    for (let j = i + 1; j < boxes.length; j++) {
      const a = boxes[i], b = boxes[j];
      const sharedX = Math.min(a.x1, b.x1) - Math.max(a.x0, b.x0);
      const sharedY = Math.min(a.y1, b.y1) - Math.max(a.y0, b.y0);
      if (sharedX > 0 && sharedY > 0) {
        bad.push(`${a.text} / ${b.text} share ${sharedX.toFixed(1)}x${sharedY.toFixed(1)}px`);
      }
    }
  }
  return bad;
}

const drawn = (rows: [string, number][], size: { width: number; height: number }) =>
  sliceLabels(render(build(rows, size), size));

/** The live card, to the pixel. */
const LIVE_CARD = { width: 166, height: 129 };
/** A pie with room to spare, where every label must still be written out in full. */
const ROOMY_CARD = { width: 640, height: 497 };
/** Where the two fixtures disagree about every name — see `INVERTED`. */
const MIDDLING_CARD = { width: 400, height: 311 };

/** Dashboard card widths from the narrowest tile to a full-width one, at the live card's aspect —
 *  a pie's radius comes off `min(width, height)`, so holding the height flat would stop exercising
 *  it half way along. */
const WIDTHS = [166, 180, 200, 220, 250, 280, 320, 360, 400, 460, 520, 640];
const sized = (width: number) => ({ width, height: Math.round((width * 129) / 166) });

describe("a pie's slice labels, as the app really draws them", () => {
  /**
   * THE DEFECT. Four labels, `Doohickey` and `Widget` sharing 30x13px of the card.
   *
   * Pinned first as a fact about the unfixed geometry so the case below cannot pass vacuously —
   * if the extraction ever stops finding labels, this fails and says so, rather than the overlap
   * case quietly asserting nothing about an empty list.
   */
  it("drew four labels at 166x129, two of them through each other", () => {
    const boxes = sliceLabels(render(
      // The builder's own option, but with the fit given nothing to measure against — which is
      // what the no-hole case used to do for every slice at every size. This is the defect
      // itself, reproduced from the code that produced it rather than described.
      unfitted(build(LIVE_SHARES, LIVE_CARD)), LIVE_CARD,
    ));
    expect(boxes.map((b) => b.text).sort()).toEqual(["Doohickey", "Gadget", "Gizmo", "Widget"]);
    // The pair the browser measured, named. Not the ONLY pair sharing ink at this size, which is
    // why this asks for that one rather than for the whole list.
    expect(overlappingPairs(boxes)).toContainEqual(expect.stringContaining("Widget / Doohickey"));
  });

  it("draws nothing over anything else on the live card", () => {
    expect(overlappingPairs(drawn(LIVE_SHARES, LIVE_CARD))).toEqual([]);
  });

  it.each(WIDTHS)("draws nothing over anything else at %ipx", (width) => {
    expect(overlappingPairs(drawn(LIVE_SHARES, sized(width)))).toEqual([]);
    expect(overlappingPairs(drawn(INVERTED, sized(width)))).toEqual([]);
    expect(overlappingPairs(drawn(DOMINANT, sized(width)))).toEqual([]);
  });

  /** The other direction, and what stops "drop every label" passing everything above: a pie with
   *  the room writes every name out in full. */
  it("writes every name in full when the pie has the room", () => {
    expect(drawn(LIVE_SHARES, ROOMY_CARD).map((b) => b.text).sort())
      .toEqual(["Doohickey", "Gadget", "Gizmo", "Widget"]);
    expect(drawn(INVERTED, ROOMY_CARD).map((b) => b.text).sort())
      .toEqual(["Doohickey", "Gadget", "Gizmo", "Widget"]);
  });

  /**
   * Each slice is decided on ITS OWN geometry — the claim that no card-wide threshold and no
   * name-length rule can satisfy. Same size, same four names, opposite deal:
   *
   *   - live card's shares: `Doohickey` (widest name, narrowest slice) is cut down; `Gizmo` is not.
   *   - inverted:           `Doohickey` (widest name, WIDEST slice) survives whole; `Gizmo`, now
   *                         on the narrowest slice, is gone entirely.
   */
  it("decides each slice on its own geometry, not the card's size or the name's length", () => {
    const live = drawn(LIVE_SHARES, MIDDLING_CARD).map((b) => b.text);
    const inverted = drawn(INVERTED, MIDDLING_CARD).map((b) => b.text);

    expect(live).toContain("Gizmo");
    expect(live).not.toContain("Doohickey");
    expect(inverted).toContain("Doohickey");
    expect(inverted).not.toContain("Gizmo");
  });

  /**
   * And a slice bigger than a semicircle is measured across the widest chord it spans, not across
   * the short one its full angle subtends — see `DOMINANT`. At this size the 252° slice has room
   * for its whole name and the 28.8° one has room for nothing; billed uncapped, the big slice is
   * charged 65.4px where it has 80.8 and loses the end of its own name.
   */
  it("measures a slice bigger than a semicircle across its widest chord", () => {
    const dominant = drawn(DOMINANT, MIDDLING_CARD).map((b) => b.text);

    expect(dominant).toContain("Doohickey");
    expect(dominant).not.toContain("Gizmo");
  });
});

/**
 * The option with every label's fit waived — `availableSpace: Infinity` for every slice, which is
 * what a hole-less pie used to be given. Reproduces the defect from the shipped builder rather
 * than from a hand-written option, so the first case above measures the real thing.
 */
function unfitted(option: Record<string, unknown>): Record<string, unknown> {
  const series = (option.series as { data: { name: string; label: Record<string, unknown> }[] }[]).map((s) => ({
    ...s,
    data: s.data.map((d) => ({ ...d, label: { ...d.label, formatter: () => d.name } })),
  }));
  return { ...option, series: series as unknown as Record<string, unknown>[] };
}
