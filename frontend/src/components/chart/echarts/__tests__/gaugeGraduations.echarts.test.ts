import { describe, it, expect, vi } from "vitest";
import * as echarts from "echarts/core";
import { platformApi } from "zrender/lib/core/platform.js";
import { registerEChartsModules } from "../register";
import { HORIZONTAL_TICKS_GAP } from "../axisLabelFit";
import { buildGaugeOption, gaugeSplitNumber } from "../options/gauge";

/**
 * WHAT A GAUGE'S ARC CAN CARRY, measured against what ECharts really draws on it.
 *
 * THE DEFECT, read off the live 166x184 dashboard card: eleven numbers round a 240° arc — `0`,
 * `76,612.39`, `153,224.78`, `229,837.17`, `306,449.56`, `383,061.95`, `459,674.34`, `536,286.73`,
 * `612,899.12`, `689,511.51`, `766,123.9` — with `229,837.17` spanning x 65-122 and `306,449.56`
 * spanning x 73-135 on the same line. The builder set no `splitNumber`, so every gauge got ECharts'
 * default of 10 intervals whatever size it was drawn at.
 *
 * WHY THIS FILE BOOTS REAL ECHARTS, which is the same reason `rowAxisLabels.test.ts` and
 * `valueAxisGutter.echarts.test.ts` do, plus one that is specific to this chart:
 *
 *   1. THE PLAN FOR THIS TASK PROPOSED ASSERTING ON `splitNumber` ITSELF —
 *      `expect(opt.series[0].splitNumber).toBeLessThanOrEqual(4)` and `expect(large.splitNumber)
 *      .toBeGreaterThan(small.splitNumber)`. Both are assertions about what was REQUESTED, which is
 *      exactly the mistake the row chart's task proved wrong: there, `splitNumber: 3` on a 250px
 *      card drew five labels because a cartesian value axis rounds the hint back to a nice interval.
 *      Neither snippet is used here. The first is satisfied by `splitNumber: 4`, which at 166px
 *      still draws five labels through each other; the second is satisfied by nothing at all,
 *      because a gauge roomy enough for ECharts' own default is deliberately left WITHOUT the key
 *      (see "leaves a gauge that can afford them alone") and `undefined > 2` is not a passing
 *      assertion, it is a broken one. Both are replaced by counting and measuring what the SVG
 *      contains: how many graduations were drawn, and whether any two of them share ink.
 *   2. A GAUGE'S LABEL GEOMETRY IS ECHARTS' OWN. Where each number lands is `r` scaled from the
 *      container, less the split-line length and two distances, with `align` and `verticalAlign`
 *      chosen from the unit vector at that angle (`chart/gauge/GaugeView.js`, `_renderTicks`). The
 *      fit reproduces that arithmetic; an assertion that reproduced it a second time would pin this
 *      file's copy of the implementation rather than ECharts.
 *
 * THE ONE FABRICATED INPUT is the font metric, fabricated identically on both sides — the mock
 * below hands the builder ZRENDER'S OWN per-glyph estimator, which is what ECharts itself lays this
 * SVG out with when there is no canvas. `measure-text.ts` returns `{width: 0}` under jsdom by design
 * ("Size claims belong in a browser"), so without it the builder would measure every label at 0px
 * and decline to decide anything. Same mock, same argument, as `rowAxisLabels.test.ts`.
 *
 * NO PIXEL IS ASSERTED. What is asserted is a COUNT and a CLEARANCE — how many graduations the SVG
 * contains, and whether any two of them come nearer than the gap of clear ink this module demands
 * between two pieces of axis text. Both are read out of the markup, and the clearance is stated
 * against the shared constant rather than against a number written here.
 */

vi.mock("@/components/chart/measure-text", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/components/chart/measure-text")>();
  const { platformApi: api } = await import("zrender/lib/core/platform.js");
  return {
    ...actual,
    measureText: (text: string, style: { size: string | number; family: string; weight: string | number }) => {
      const size = typeof style.size === "number" ? `${style.size}px` : style.size;
      return { width: api.measureText(text, `${style.weight} ${size} ${style.family}`).width, height: 0 };
    },
  };
});

const cols = [{ name: "rev", type: "double" }];
const CARD = { width: 166, height: 184 };
/** The gauge's graduations; the centre detail is drawn at 24 and is excluded by that. */
const LABEL_FONT_SIZE = 11;
const DETAIL_FONT_SIZE = 24;

/**
 * THE LIVE CARD'S VALUE. `defaultGaugeSegments` runs the arc 0 -> 2v, so this is the gauge whose
 * eleven numbers were measured in the browser, down to the strings.
 *
 * IT IS NOT A ROUND NUMBER, and that is what stops the fixture handing a wrong implementation the
 * right answer. Three coincidences it avoids:
 *
 *   - THE WIDEST LABEL IS NOT THE MAXIMUM. `766,123.9` measures 49.3px, but the thirds this arc
 *     divides into at `splitNumber: 3` are `255,374.633` and `510,749.267` at 61.6px — WIDER than
 *     the whole they divide. An implementation that measured the extremes, or measured one candidate
 *     once and charged it at every rung, is a rung too generous here and the render says so.
 *   - THE MINIMUM IS NOT THE SAME WIDTH AS THE MAXIMUM. `0` is 6.2px against 49.3, so charging
 *     "twice the widest" and charging "the two ends" are different numbers, and the pair that
 *     actually collides at this size is the two ends.
 *   - THE MAXIMUM IS NOT ROUND, so an implementation that reached for a `nice()` ceiling the way the
 *     cartesian gutter does cannot land on the same string by luck.
 */
const LIVE_VALUE = 383_061.95;

/**
 * A gauge whose scale is AUTHORED rather than derived, and the fixture the live one cannot be:
 * every number on the live arc is a multiple of the datum (the arc is 0 -> 2v, so the midpoint at
 * `splitNumber: 2` IS the value), which means an implementation that measured the DATUM instead of
 * the scale would agree with a correct one at the rung that matters. Here the scale starts away from
 * zero, does not end at twice anything, and the value sits nowhere near its middle — the midpoint is
 * 499,999.95 and the needle is at 300,000 — so the two implementations cannot coincide.
 */
const OFFSET_SEGMENTS = [
  { min: 12_345.6, max: 400_000, color: "#ff0000" },
  { min: 400_000, max: 987_654.3, color: "#00ff00" },
];

/** Short labels, so ECharts' own eleven fit on a roomy card and the fit has to say NOTHING there. */
const SMALL_VALUE = 100;

/**
 * A MID-SIZED SCALE, 0 to 2,926.8, and the one fixture here chosen by search rather than from the
 * live card. It is what makes ECharts' `verticalAlign` rule observable.
 *
 * A graduation within about 37° of the top of the arc is placed with its TOP at the anchor rather
 * than its middle (`verticalAlign: unitY < -0.8 ? 'top' : ...`), which puts its ink half a line
 * lower than a naive model would. On the three fixtures above that half-line never decides
 * anything, so an implementation that ignored the rule passed every case — verified as a mutant.
 * Here it decides at 220, 300 and 320px, where ignoring it buys two graduations too many and the
 * render shows them touching.
 *
 * The scale is not round for the same reason the live one is not, and its role is narrow: it is a
 * fourth member of the width sweep, not a replacement for the two fixtures that guard against
 * measuring the datum or the extremes.
 */
const MID_VALUE = 1463.4;

interface Box { text: string; x0: number; x1: number; y0: number; y1: number }

function render(option: unknown, size: { width: number; height: number }): string {
  registerEChartsModules();
  const chart = echarts.init(null, null, { renderer: "svg", ssr: true, ...size });
  chart.setOption({ ...(option as object), animation: false });
  const svg = chart.renderToSVGString();
  chart.dispose();
  return svg;
}

const familyOf = (svg: string) => /font-family:([^;"]+)/.exec(svg)?.[1] ?? "sans-serif";

interface RawText { text: string; x: number; y: number; anchor: string; fontSize: number }
function parseTexts(svg: string): RawText[] {
  return [...svg.matchAll(/<text\b([^>]*)>([^<]*)<\/text>/g)].map(([, attrs, text]) => ({
    text,
    x: Number(/\sx="(-?[\d.]+)"/.exec(attrs)?.[1] ?? NaN),
    y: Number(/\sy="(-?[\d.]+)"/.exec(attrs)?.[1] ?? NaN),
    anchor: /text-anchor="([a-z]+)"/.exec(attrs)?.[1] ?? "start",
    fontSize: Number(/font-size:([\d.]+)px/.exec(attrs)?.[1] ?? 0),
  }));
}

/**
 * The graduations the gauge DREW, each with the band of page its ink occupies.
 *
 * Read entirely out of the markup: the `x` attribute plus `text-anchor` give the horizontal band,
 * and every one of these is emitted with `dominant-baseline="central"`, so `y` is the text's
 * VERTICAL CENTRE and the band is half a line either side of it. Nothing here is derived from the
 * implementation's own placement arithmetic, which is the point — see this file's header.
 */
function graduations(svg: string): Box[] {
  const family = familyOf(svg);
  return parseTexts(svg)
    .filter((t) => t.fontSize === LABEL_FONT_SIZE && t.text.trim() !== "")
    .map((t) => {
      const w = platformApi.measureText(t.text, `400 ${t.fontSize}px ${family}`).width;
      const x0 = t.anchor === "start" ? t.x : t.anchor === "end" ? t.x - w : t.x - w / 2;
      return { text: t.text, x0, x1: x0 + w, y0: t.y - t.fontSize / 2, y1: t.y + t.fontSize / 2 };
    });
}

/**
 * Every pair of graduations that fails to keep clear of the other, described so a failure names the
 * two strings.
 *
 * EVERY PAIR, not just neighbours along the arc: two labels on an arc can be far apart in angle and
 * on top of each other on the page. On this gauge the first pair to collide is precisely the two
 * OUTERMOST — 210° and -30° put them at the same height, anchored towards each other, and at the
 * live card's 24.25px label ring their anchors are 42px apart while `766,123.9` alone is 49.3.
 *
 * THE BAR IS CLEAR INK AND NOT MERE NON-OVERLAP, which is stronger than it first looks and is what
 * makes the shared gap constant load-bearing rather than decorative. `HORIZONTAL_TICKS_GAP` is
 * Metabase's minimum ink-free space between two neighbouring pieces of axis text, and the fit spends
 * it by inflating each label's box half a gap on every side — so two graduations that survive it are
 * separated by a whole gap in at least ONE dimension, never merely touching. Asserting only
 * `overlap > 0` would leave "demand no clear ink at all" as a change no case notices: it was tried
 * as a mutant, and it passed a version of this file that stopped at non-overlap.
 */
function crowdedPairs(boxes: Box[]): string[] {
  const bad: string[] = [];
  for (let i = 0; i < boxes.length; i++) {
    for (let j = i + 1; j < boxes.length; j++) {
      const a = boxes[i], b = boxes[j];
      // Positive is clear ink between them; negative is shared ink.
      const gapX = Math.max(a.x0, b.x0) - Math.min(a.x1, b.x1);
      const gapY = Math.max(a.y0, b.y0) - Math.min(a.y1, b.y1);
      const clear = Math.max(gapX, gapY);
      // The tolerance is float noise only: the SVG's own coordinates and the fit's are the same
      // arithmetic on the same measurer (zrender ignores the family and reads the px size), so the
      // two agree to the bit in every case here — which is why a whole gap can be demanded rather
      // than approximated.
      if (clear < HORIZONTAL_TICKS_GAP - 1e-6) {
        bad.push(`${a.text} / ${b.text} keep ${clear.toFixed(1)}px clear`);
      }
    }
  }
  return bad;
}

type Series = {
  splitNumber?: number;
  axisLabel: { show?: boolean; formatter: (v: number) => string };
  min: number; max: number;
};
const seriesOf = (opt: unknown) => (opt as { series: Series[] }).series[0];

const liveGauge = (size?: { width: number; height: number }) =>
  buildGaugeOption(cols, [[LIVE_VALUE]], { metric: "rev", metricTypes: { rev: "double" } }, size);
const offsetGauge = (size?: { width: number; height: number }) =>
  buildGaugeOption(cols, [[300_000]], { metric: "rev", metricTypes: { rev: "double" }, segments: OFFSET_SEGMENTS }, size);
const smallGauge = (size?: { width: number; height: number }) =>
  buildGaugeOption(cols, [[SMALL_VALUE]], { metric: "rev", metricTypes: { rev: "double" } }, size);
const midGauge = (size?: { width: number; height: number }) =>
  buildGaugeOption(cols, [[MID_VALUE]], { metric: "rev", metricTypes: { rev: "double" } }, size);
const FIXTURES = [
  ["live", liveGauge], ["offset", offsetGauge], ["small", smallGauge], ["mid", midGauge],
] as const;

/** The card widths a dashboard really takes, from the narrowest tile to a full-width one. Heights
 *  keep the live card's aspect, because a gauge's radius is scaled from `min(width, height)` and a
 *  sweep that held the height flat would stop testing the radius half way along. */
const WIDTHS = [166, 180, 200, 220, 250, 280, 300, 320, 400, 500, 640, 900];
const sized = (width: number) => ({ width, height: Math.round((width * 184) / 166) });

describe("ECharts' gauge axis, as it really behaves", () => {
  /**
   * THE FACT THE WHOLE FIX RESTS ON, and it is NOT the one the cartesian value axis taught. There,
   * `splitNumber` is a hint ECharts rounds to a nice interval and rounds DOWN — 3 and 4 both drew
   * five labels on a 250px row chart, which is why `intervalThatFits` names an interval instead.
   * A `series.gauge` is a different code path with no `nice()` in it: `_renderTicks` walks
   * `i = 0..splitNumber` and formats `min + i * (max - min) / splitNumber` verbatim. So the count is
   * exact, and the fit is entitled to ask for one.
   *
   * Pinned here rather than trusted, because everything below reads as sound and is wrong if this
   * ever stops holding — and an ECharts upgrade is exactly where it would stop.
   */
  it.each([1, 2, 3, 4, 5, 6, 7, 8])("draws exactly n+1 graduations for splitNumber %i", (n) => {
    const opt = liveGauge() as unknown as { series: Series[] };
    opt.series[0].splitNumber = n;
    expect(graduations(render(opt, CARD))).toHaveLength(n + 1);
  });

  /** And with no `splitNumber` at all it draws eleven, at a size where two of them overlap by 49px.
   *  This is the defect itself, still reproducible from the option the builder makes when it is
   *  given no size to decide from. */
  it("draws eleven graduations when nothing says otherwise", () => {
    expect(graduations(render(liveGauge(), CARD))).toHaveLength(11);
    expect(crowdedPairs(graduations(render(liveGauge(), CARD)))).not.toEqual([]);
  });

  /**
   * The lever a reader will reach for SECOND, and it does nothing. `axisLabel.showMinLabel` and
   * `showMaxLabel` are cartesian-axis options; `_renderTicks` never consults them, so all four
   * combinations draw the same four labels. Recorded so the next person to think "just hide the
   * ends" spends no time on it.
   */
  it.each([
    ["neither", {}],
    ["no min", { showMinLabel: false }],
    ["no max", { showMaxLabel: false }],
    ["neither end", { showMinLabel: false, showMaxLabel: false }],
  ])("ignores showMinLabel/showMaxLabel (%s)", (_what, flags) => {
    const opt = liveGauge() as unknown as { series: Series[] };
    opt.series[0].splitNumber = 3;
    Object.assign(opt.series[0].axisLabel, flags);
    expect(graduations(render(opt, CARD)).map((g) => g.text))
      .toEqual(["0", "255,374.633", "510,749.267", "766,123.9"]);
  });
});

describe("a gauge's graduations fit the arc that carries them", () => {
  /**
   * THE DEFECT. At 166x184 the label ring is 24.25px of radius — `0.75 * min(w, h) / 2`, less the
   * split-line length and the two distances — so the 240° arc offers about 100px of circumference to
   * share, and `766,123.9` alone measures 49.3. Eleven numbers is not a near miss there.
   */
  it("does not draw eleven graduations on a 166px card", () => {
    expect(graduations(render(liveGauge(CARD), CARD)).length).toBeLessThan(11);
  });

  /**
   * THE PROPERTY, stated over the widths a dashboard card really takes and over three scales. This
   * is what replaces the plan's `splitNumber <= 4`: a count is only a proxy, and the wrong count is
   * only wrong because of what it does to the ink.
   */
  it.each(WIDTHS)("draws no two graduations over each other at %ipx", (width) => {
    const size = sized(width);
    for (const [what, build] of FIXTURES) {
      const drawn = graduations(render(build(size), size));
      expect({ what, crowded: crowdedPairs(drawn) }).toEqual({ what, crowded: [] });
    }
  });

  /**
   * THE STRINGS THE FIT MEASURED ARE THE STRINGS THE GAUGE DREW.
   *
   * Every rung's labels are `min + i * (max - min) / n` formatted, and that set changes with `n` in
   * a way no single candidate stands in for — at `splitNumber: 3` this arc's thirds are WIDER than
   * its maximum. A fit that measured the extremes once, or measured the datum, or generated its
   * values a fraction differently from `_renderTicks`, would be deciding from strings the arc never
   * renders; that is the failure the temporal stride ladder shipped with and had to be rewritten
   * for. So the measurer is spied on, and every graduation the SVG contains has to be a string the
   * fit was handed.
   */
  it.each(WIDTHS)("measures the graduations it will draw, and not others, at %ipx", (width) => {
    const size = sized(width);
    const seen = new Set<string>();
    const measure = (text: string, style: { size: string | number; family: string; weight: string | number }) => {
      seen.add(text);
      const px = typeof style.size === "number" ? `${style.size}px` : style.size;
      return { width: platformApi.measureText(text, `${style.weight} ${px} ${style.family}`).width, height: 0 };
    };
    const opt = liveGauge(size);
    const s = seriesOf(opt);
    // Re-ask the fit with the spy in place; the builder's own call used the mocked module measurer,
    // and both are zrender's estimator, so the two runs are the same computation.
    gaugeSplitNumber({ size, min: s.min, max: s.max, format: s.axisLabel.formatter, measure });
    const drawn = graduations(render(opt, size)).map((g) => g.text);
    for (const text of drawn) expect([...seen]).toContain(text);
  });

  /**
   * …DOWN TO ECHARTS' OWN ROUNDING OF THE VALUE, which is the part of "the same strings" that the
   * live fixture cannot show. `_renderTicks` does not format `min + i * (max - min) / n`; it formats
   * `round(that, DEFAULT_PRECISION_FOR_ROUNDING_ERROR)`, and that constant is 14. With a formatter
   * that caps decimals the two are the same string and nothing is at stake — every case above uses
   * `formatValue`, which does cap them, so an implementation that skipped the rounding passed all of
   * them (verified as a mutant).
   *
   * `String` DOES NOT CAP THEM, and it is not a hypothetical formatter: `buildGaugeOption` falls
   * back to exactly that when the chart carries no metric. A third of 100 then formats
   * `33.333333333333336` unrounded against the `33.33333333333334` ECharts draws — one character
   * wider, and a fit deciding from the wrong one is deciding from a string the arc never renders.
   *
   * The ladder measures every rung from ten down until one fits, so rung 3's strings pass through the
   * spy whatever the fit finally answers; what the render then has to agree with is that rung.
   */
  it("mirrors ECharts' own rounding of a graduation's value", () => {
    const seen = new Set<string>();
    const measure = (text: string, style: { size: string | number; family: string; weight: string | number }) => {
      seen.add(text);
      const px = typeof style.size === "number" ? `${style.size}px` : style.size;
      return { width: platformApi.measureText(text, `${style.weight} ${px} ${style.family}`).width, height: 0 };
    };
    const format = (v: number) => String(v);
    gaugeSplitNumber({ size: CARD, min: 0, max: 100, format, measure });

    const opt = buildGaugeOption(cols, [[50]], {}, undefined) as unknown as { series: Series[] };
    opt.series[0].splitNumber = 3;
    opt.series[0].axisLabel.formatter = format;
    const drawn = graduations(render(opt, CARD)).map((g) => g.text);
    // Not vacuous: a repeating third has to be among them, or the case is about nothing.
    expect(drawn.some((t) => t.length > 10)).toBe(true);
    for (const text of drawn) expect([...seen]).toContain(text);
  });

  /**
   * THE HIDE ARM. Below a certain size not even the two ends clear each other — they sit at the same
   * height, anchored towards each other, 42px apart at the live card's ring against a 49.3px
   * maximum — and the honest answer is no number on the arc. Metabase's own gauge does exactly this,
   * at a threshold of its own: `showLabels = svgWidth > MIN_WIDTH_LABEL_THRESHOLD`, and that
   * constant is 250 (`visualizations/Gauge/constants.ts`).
   *
   * BOTH HALVES ARE ASSERTED so "it hid them" cannot be satisfied by a gauge that drew nothing at
   * all: the centre reading is still there, at 24px, which is where a reader gets the number when
   * the arc cannot carry one.
   */
  it("draws no graduations at all where not even the two ends fit", () => {
    const svg = render(liveGauge(CARD), CARD);
    expect(graduations(svg)).toEqual([]);
    expect(parseTexts(svg).filter((t) => t.fontSize === DETAIL_FONT_SIZE).map((t) => t.text))
      .toEqual(["383,061.95"]);
  });

  /**
   * …AND IT IS A DECISION ABOUT THE CARD, not a rule that quietly retired graduations for any wide
   * label: the same gauge on a 250px card has room for three, and draws them. The pair brackets the
   * threshold rather than asserting one anecdote.
   */
  it("draws the same gauge's graduations once the card is wide enough", () => {
    const size = sized(250);
    const drawn = graduations(render(liveGauge(size), size));
    expect(drawn.map((g) => g.text)).toEqual(["0", "383,061.95", "766,123.9"]);
    expect(crowdedPairs(drawn)).toEqual([]);
  });

  /**
   * THE OTHER DIRECTION, and the reason the fit is a CEILING rather than a count: a gauge with room
   * for ECharts' own eleven must keep them, and must keep them by saying NOTHING — no `splitNumber`,
   * no `axisLabel.show`. A fix that simply asked for fewer graduations everywhere would pass every
   * case above and quietly strip every roomy gauge down to three.
   */
  it("leaves a gauge that can afford them alone", () => {
    const size = sized(640);
    const s = seriesOf(smallGauge(size));
    expect(s.splitNumber).toBeUndefined();
    expect(s.axisLabel.show).toBeUndefined();
    expect(graduations(render(smallGauge(size), size))).toHaveLength(11);
  });

  /**
   * MONOTONICITY, and this is what replaces the plan's second snippet
   * (`large.splitNumber > small.splitNumber`). That one compares the numbers REQUESTED, and on a
   * roomy card there is no number to compare — the option deliberately carries none, so the
   * assertion is not merely weak but broken. What a reader actually means by it is that a bigger
   * card shows at least as much of the scale, which is a statement about graduations DRAWN and
   * holds across the whole sweep rather than at one arbitrary pair of sizes.
   */
  it.each(FIXTURES)(
    "never draws fewer graduations on a bigger card (%s)", (_what, build) => {
      const counts = WIDTHS.map((w) => graduations(render(build(sized(w)), sized(w))).length);
      expect(counts).toEqual([...counts].sort((a, b) => a - b));
      // Not vacuous: the sweep has to actually move, or a fit that returned one answer everywhere
      // would sort as sorted.
      expect(counts[counts.length - 1]).toBeGreaterThan(counts[0]);
    });

  /**
   * NO SIZE, NO DECISION. The builder is pure data with no DOM of its own; a caller that hands over
   * no measurement gets the gauge egret drew before this fit existed rather than one thinned against
   * a guessed radius. Same rule every other size-aware builder in this directory keeps.
   */
  it("says nothing when it is given no size", () => {
    const s = seriesOf(liveGauge());
    expect(s.splitNumber).toBeUndefined();
    expect(s.axisLabel.show).toBeUndefined();
  });

  /**
   * …AND WHERE IT DOES DECIDE, IT DECIDES ONE THING. `sizeAware.test.ts` owns this question for the
   * other four builders, and cannot own it for the gauge: that file runs on the real measurer, which
   * returns zero under jsdom, so the fit declines there and a sized gauge is trivially identical to
   * an unsized one. Here the measurer works, so the two builds really do differ — and they must
   * differ ONLY in the two keys the fit is entitled to write. The obvious way for a size to leak is
   * a formatter that closes over it (`(v) => truncate(v, widthFromSize)`), whose source text is the
   * same whatever it captured, so functions are fingerprinted by what they RETURN as well as by
   * their source. That is `sizeAware.test.ts`'s own argument, kept rather than repeated: this is the
   * environment where it can be applied to this builder.
   */
  it("reads the size, and reads it ONLY for the graduations", () => {
    const probe = (fn: (v: never) => unknown) =>
      [0, 1234.5, LIVE_VALUE].map((v) => { try { return String(fn(v as never)); } catch (e) { return `!${String(e)}`; } });
    const fingerprint = (v: unknown): unknown => {
      if (typeof v === "function") return { fn: String(v), probes: probe(v as (a: never) => unknown) };
      if (Array.isArray(v)) return v.map(fingerprint);
      if (v !== null && typeof v === "object") {
        return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, fingerprint(x)]));
      }
      return v;
    };
    const sized = fingerprint(liveGauge(CARD)) as { series: Record<string, Record<string, unknown>>[] };
    const unsized = fingerprint(liveGauge()) as typeof sized;
    // Pinned exactly, so a formatter that collapsed to a plain string could not take itself out of
    // the comparison unnoticed: tooltip, arc axis label, centre detail.
    expect(JSON.stringify(sized).match(/"fn":/g)).toHaveLength(3);
    expect(sized.series[0]).not.toStrictEqual(unsized.series[0]);

    delete sized.series[0].splitNumber;
    delete unsized.series[0].splitNumber;
    delete (sized.series[0].axisLabel as Record<string, unknown>).show;
    delete (unsized.series[0].axisLabel as Record<string, unknown>).show;
    // `formatter` is still in there, and so is every other field of every other key.
    expect(sized).toStrictEqual(unsized);
  });

  /**
   * …and nothing when the size cannot produce a measurement either. `measure-text.ts` returns
   * `{width: 0}` under jsdom by design, and a zero-width label clears any budget — so an unmeasured
   * environment would be told that eleven labels fit, which happens to be the answer it wants but by
   * accident. Declining explicitly keeps the contract "decide from a width, or decline", and the
   * `0` that means "draw none" stays out of reach of an environment that measured nothing.
   */
  it("declines rather than hiding when nothing can be measured", () => {
    expect(gaugeSplitNumber({
      size: CARD, min: 0, max: 766_123.9, format: (v) => String(v), measure: () => ({ width: 0, height: 0 }),
    })).toBeUndefined();
  });

  /** A size that is not a size is not a tiny card. `<= 0` is false for NaN, so the guards say `> 0`
   *  — an unmeasured input has to come back as "no answer" rather than as a `splitNumber` of NaN
   *  quietly reaching an option, the same rule `intervalThatFits` keeps. */
  it.each([
    ["a NaN width", { width: NaN, height: 184 }],
    ["a zero height", { width: 166, height: 0 }],
    ["a NaN bound", { width: 166, height: 184 }],
  ])("answers nothing rather than NaN for %s", (what, size) => {
    expect(gaugeSplitNumber({
      size, min: 0, max: what === "a NaN bound" ? NaN : 766_123.9, format: (v) => String(v),
    })).toBeUndefined();
  });
});
