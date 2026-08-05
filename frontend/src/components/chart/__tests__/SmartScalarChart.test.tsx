import { describe, it, expect, afterEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { SmartScalarChart } from "../SmartScalarChart";

const cols = [{ name: "mo", type: "varchar" }, { name: "rev", type: "double" }];

describe("SmartScalarChart", () => {
  it("shows the latest value, its period, and the % change + previous value", () => {
    render(<SmartScalarChart columns={cols} rows={[["Jan", 100], ["Feb", 120]]} dimension="mo" metric="rev" format={{ decimals: 0 }} />);
    expect(screen.getByText("120")).toBeInTheDocument();
    expect(screen.getByText("Feb")).toBeInTheDocument();            // current period label
    expect(screen.getByText(/20\.00%/)).toBeInTheDocument();
    expect(screen.getByText(/vs previous: 100/)).toBeInTheDocument(); // previous value
  });
  it("marks a decrease with a down arrow", () => {
    const { container } = render(<SmartScalarChart columns={cols} rows={[["Jan", 100], ["Feb", 80]]} dimension="mo" metric="rev" />);
    expect(container.textContent).toContain("▼");
  });
  it("shows just the value when there is no prior period", () => {
    render(<SmartScalarChart columns={cols} rows={[["Jan", 100]]} dimension="mo" metric="rev" format={{ decimals: 0 }} />);
    expect(screen.getByText("100")).toBeInTheDocument();
    expect(screen.queryByText(/vs/)).not.toBeInTheDocument();
  });
  it("renders an em dash when there is no data", () => {
    render(<SmartScalarChart columns={cols} rows={[]} dimension="mo" metric="rev" />);
    expect(screen.getByText("—")).toBeInTheDocument();
  });
});

describe("SmartScalarChart — Metabase parity", () => {
  const cols = [{ name: "d", type: "date" }, { name: "m", type: "double" }];
  const rows = [["2026-02-01", 36860], ["2026-03-01", 36513]];

  // Metabase writes "vs. previous month", not a bare "vs. previous" — a reader should not have to
  // remember which granularity the chart was grouped by to know what it is compared against.
  it("names the period it compares against", () => {
    render(<SmartScalarChart columns={cols} rows={rows} dimension="d" metric="m" unit="month" />);
    expect(screen.getByText(/vs previous month:/)).toBeInTheDocument();
  });

  it("falls back to a bare 'vs previous' when the dimension carries no unit", () => {
    render(<SmartScalarChart columns={cols} rows={rows} dimension="d" metric="m" />);
    expect(screen.getByText(/vs previous:/)).toBeInTheDocument();
  });

  // A metric where LESS is better — cost, churn, time to resolve — reads backwards otherwise:
  // green for the number going up. The same defect the gauge's fixed zones had before chart 04.
  it("can swap which direction is coloured good, without lying about the direction", () => {
    const { rerender, container } = render(
      <SmartScalarChart columns={cols} rows={rows} dimension="d" metric="m" />,
    );
    const trend = () => container.querySelector("[data-trend]") as HTMLElement;
    // The value fell, so by default it is red.
    const fell = trend().style.color;
    expect(trend().textContent).toContain("▼");

    rerender(<SmartScalarChart columns={cols} rows={rows} dimension="d" metric="m" switchPositiveNegative />);
    expect(trend().style.color).not.toBe(fell);
    // The ARROW still reports what happened — only the reading of it is inverted.
    expect(trend().textContent).toContain("▼");
  });

  it("compacts the numbers when asked", () => {
    render(<SmartScalarChart columns={cols} rows={rows} dimension="d" metric="m" compact />);
    expect(screen.queryByText("36,513")).toBeNull();
    expect(screen.getByText(/36\.5k/i)).toBeInTheDocument();
  });

  // A PRECEDENCE test, not a granularity one — there is no `unit` here, so nothing competes with
  // the format. Its comment used to say the label "should read at the grouping's granularity",
  // true while the format was the only thing that COULD set the granularity; since PR 03 the unit
  // sets it, and this case says which wins when both are present, except that it cannot, because
  // only one is. The case with both is "lets the dimension's own format win" below.
  it("formats the period label through the dimension's own format", () => {
    render(
      <SmartScalarChart columns={cols} rows={rows} dimension="d" metric="m"
        dimensionFormat={{ date_style: "MMMM YYYY" }} />,
    );
    expect(screen.getByText("March 2026")).toBeInTheDocument();
  });
});

/**
 * The label above the number names the PERIOD the latest row covers. It printed that period's first
 * DAY — "December 1, 2026" directly above a caption reading "vs previous month", which is the card
 * contradicting itself. Observed live on the ZZ Chart Parity harness.
 */
describe("SmartScalarChart — the label names the period, not its first day", () => {
  const cols = [{ name: "d", type: "date" }, { name: "m", type: "double" }];
  const rows = [["2026-02-01", 36860], ["2026-03-01", 36513]];

  it("names the period at the unit the caption beneath it already uses", () => {
    render(<SmartScalarChart columns={cols} rows={rows} dimension="d" metric="m" unit="month" />);
    expect(screen.getByText("March 2026")).toBeInTheDocument();
    expect(screen.queryByText("March 1, 2026")).toBeNull();
    // The two now agree about the granularity, which is the point.
    expect(screen.getByText(/vs previous month:/)).toBeInTheDocument();
  });

  it("names a quarter and a year the same way", () => {
    const { rerender } = render(
      <SmartScalarChart columns={cols} rows={[["2026-04-01", 1], ["2026-07-01", 2]]}
        dimension="d" metric="m" unit="quarter" />,
    );
    expect(screen.getByText("Q3 2026")).toBeInTheDocument();
    rerender(
      <SmartScalarChart columns={cols} rows={[["2025-01-01", 1], ["2026-01-01", 2]]}
        dimension="d" metric="m" unit="year" />,
    );
    expect(screen.getByText("2026")).toBeInTheDocument();
  });

  /**
   * A WEEK is named by its first day here, not by Metabase's span. Metabase's trend card runs this
   * label through `formatDateTimeRangeWithUnit(..., {compact: true})` (`SmartScalar/compute.ts:702`)
   * and gets "Jan 5–11, 2026" — and, at every OTHER unit, an abbreviated month ("Mar 2026" above,
   * not "March 2026"). That is a condensed-range formatter this codebase does not have; building it
   * is a change of its own, not an argument to `periodName`. Pinned so the divergence is a decision
   * on the record rather than something rediscovered.
   *
   * WHAT THIS TEST CANNOT DO, said plainly because the assertion looks stronger than it is: it
   * pins the STRING, not the code path. `periodName`'s week format and `formatValue`'s default
   * date style are the same "MMMM D, YYYY", so making this card ignore the unit entirely leaves it
   * green — verified by mutation, not assumed. It dies to a change in the week format itself,
   * which is what it is for. `temporal-period.test.ts` pins the path, on the namer directly.
   */
  it("names a week by its first day — the known, deliberate gap from Metabase's span form", () => {
    render(
      <SmartScalarChart columns={cols} rows={[["2025-12-29", 1], ["2026-01-05", 2]]}
        dimension="d" metric="m" unit="week" />,
    );
    expect(screen.getByText("January 5, 2026")).toBeInTheDocument();
  });

  // Same order as the x-axis: a stored format outranks the bucketing, because a setting the panel
  // offers, stores and then ignores is a defect this codebase has fixed three times already.
  it("lets the dimension's own format win over the period name", () => {
    render(
      <SmartScalarChart columns={cols} rows={rows} dimension="d" metric="m" unit="month"
        dimensionFormat={{ date_style: "YYYY-MM-DD" }} />,
    );
    expect(screen.getByText("2026-03-01")).toBeInTheDocument();
    expect(screen.queryByText("March 2026")).toBeNull();
  });

  /**
   * The same hazard the axis has, through the same writer. `styleColumnsFor` falls through to
   * `return resultColumns` for a smartscalar (`settings/SharedDisplaySections.tsx:52`), so the
   * dimension reaches `FormatSection`, whose `patch` spreads into `{...fmt, ...p}` (`:67`).
   * Renaming the date column stored `{column_title: "Month"}` — truthy, silent about dates — which
   * took the format arm and put "March 1, 2026" back above a caption reading "vs previous month".
   * A rename undoing this task's fix, with nothing in the panel to explain it.
   */
  it("is not displaced by a format that says nothing about dates", () => {
    const { rerender } = render(
      <SmartScalarChart columns={cols} rows={rows} dimension="d" metric="m" unit="month"
        dimensionFormat={{ column_title: "Month" }} />,
    );
    expect(screen.getByText("March 2026")).toBeInTheDocument();
    // A title typed and then deleted persists the key with no value.
    rerender(
      <SmartScalarChart columns={cols} rows={rows} dimension="d" metric="m" unit="month"
        dimensionFormat={{ column_title: undefined }} />,
    );
    expect(screen.getByText("March 2026")).toBeInTheDocument();
  });

  /**
   * The negative, and the one a wrong guard would break: with no unit the card is NOT bucketed, so
   * the label keeps `formatValue`'s default date style. Naming a period here would invent a
   * granularity; returning the raw "2026-03-01" would be a regression from what shipped.
   */
  it("keeps the default date style when the dimension carries no unit", () => {
    render(<SmartScalarChart columns={cols} rows={rows} dimension="d" metric="m" />);
    expect(screen.getByText("March 1, 2026")).toBeInTheDocument();
  });

  it("leaves a non-temporal dimension alone even under a unit", () => {
    // TWO guards defend this and only the outer one belongs to this site. `namesAPeriod`
    // ("day_of_week") is false, so arm 2 is skipped and `periodName` is never reached — widening
    // either guard alone leaves this green, and it takes both to turn a `day_of_week` of 3 into
    // "March 2001", which dayjs will happily do with it. The axis twin of this test is the one
    // that pins `periodName`'s own `isExtractUnit` guard, because there arm 2 is not gated.
    render(
      <SmartScalarChart columns={[{ name: "dow", type: "varchar" }, { name: "m", type: "double" }]}
        rows={[["2", 1], ["3", 2]]} dimension="dow" metric="m" unit="day_of_week" />,
    );
    expect(screen.getByText("3")).toBeInTheDocument();
  });
  /**
   * A NULL METRIC IS NOT A ZERO, and this chart treated it as one.
   *
   * `valid` filters the rows to those whose metric is usable, and it asked
   * `Number.isFinite(Number(r[mi]))`. `Number(null)` is `0` and `Number("")` is `0`, both finite —
   * so a trailing month with no data became the LATEST period, the big number read 0, and the
   * comparison read "down 100.00% vs previous month". A card saying revenue collapsed to nothing
   * when the truth is that the month has not been loaded yet is the same class of defect as a
   * drill that under-reports: wrong, confident, and with nothing on screen to say so.
   *
   * Found by the drill added in this unit, which pointed at that null row's period.
   */
  it("skips a trailing row whose metric is null rather than reading it as zero", () => {
    render(
      <SmartScalarChart columns={cols} rows={[...rows, ["2026-04-01", null]]}
        dimension="d" metric="m" unit="month" />,
    );
    expect(screen.getByText("March 2026")).toBeInTheDocument();
    expect(screen.queryByText("April 2026")).toBeNull();
    expect(screen.queryByText(/100.00%/)).toBeNull();
  });

  // …and an empty string is the same case: `Number("")` is also 0.
  it("skips a row whose metric is an empty string", () => {
    render(
      <SmartScalarChart columns={cols} rows={[...rows, ["2026-04-01", ""]]}
        dimension="d" metric="m" unit="month" />,
    );
    expect(screen.getByText("March 2026")).toBeInTheDocument();
  });
});

/**
 * THE NUMBER IS SIZED TO ITS CARD, instead of being drawn at a fixed 2.5rem whatever the card.
 *
 * MEASURED, in Chrome, on the "ZZ Chart Parity" dashboard where every card is 182px wide with a
 * 166px content box: the trend's value `24,730.43` occupied **203.75px** and its box ran from
 * x=444 to x=648 against a content box of 463–629. So the number hung 19px past BOTH edges of its
 * own card, over the neighbours on either side, and the screenshot shows it clipped mid-digit at
 * each end. Pre-existing — #413 pinned that the drill button it added contributes 0px.
 *
 * THE FIX IS THE SCALAR'S, NOT A SECOND ONE. `BigNumber` solved this exact problem: compact the
 * value against the box's width (`compactifyValue`), then pick the largest font at which the
 * RESULT still fits (`findSize` over canvas `measureText`), and put the exact figure in a tooltip
 * whenever compaction changed it. Every piece of that is already shared, already ported from
 * Metabase and already tested; this card reuses it whole rather than growing a second sizer.
 *
 * WHAT THESE TESTS DO AND DO NOT PROVE. **jsdom runs no layout**, so `measure-text.ts`
 * deliberately returns `{0, 0}` with no 2D context — which makes every candidate font "fit" and
 * sends `findSize` straight to its ceiling. So NOTHING here proves the chosen size is *right*, or
 * that the number fits: those claims belong to a browser and rest on the screenshot at the 166px
 * card width. What IS provable without layout is everything that is arithmetic over the box rather
 * than over the glyphs — whether the value compacts (`shouldCompact`, pure over string length and
 * width) and what ceiling the fit is allowed to reach (`maxFontSizeRem`, pure over the card's width
 * and grid share). The BOX is therefore stubbed below rather than left at jsdom's 0×0: a size the
 * test chose is a size the assertions can reason about, and a 0×0 box makes the sizer's answer
 * (2.5rem, its narrow-card ceiling) coincidentally equal to the constant it replaced — which is to
 * say, indistinguishable from the defect.
 */
describe("SmartScalarChart — the number is sized to its card", () => {
  const cols = [{ name: "d", type: "date" }, { name: "m", type: "double" }];
  // The live values off the parity harness, and the exact pair the measurement above came from.
  const rows = [["2026-11-01", 22577.58], ["2026-12-01", 24730.43]];

  /** Give every element a fixed box, the way the ECharts specs under `echarts/__tests__` do.
   *  `useElementSize` reads `getBoundingClientRect` on mount, so this is what the sizer sees. */
  const withBox = (width: number, height: number) => {
    Object.defineProperty(HTMLElement.prototype, "getBoundingClientRect", {
      configurable: true,
      value: () => ({ width, height, top: 0, left: 0, right: width, bottom: height, x: 0, y: 0, toJSON() {} }),
    });
  };
  afterEach(() => {
    delete (HTMLElement.prototype as unknown as Record<string, unknown>).getBoundingClientRect;
  });

  /** The harness's card: 182px wide, 166px of content. */
  const NARROW = 166;

  it("compacts a value too long for its card instead of letting it overflow", () => {
    withBox(NARROW, NARROW);
    render(<SmartScalarChart columns={cols} rows={rows} dimension="d" metric="m" unit="month" />);
    // 166px of card less `SCALAR_PADDING` is 134, under Metabase's 250px compaction threshold, and
    // nine characters is over its six-character floor. So this one cannot be drawn in full.
    expect(screen.queryByText("24,730.43")).toBeNull();
    expect(screen.getByText(/24\.7k/i)).toBeInTheDocument();
  });

  it("draws the same value in full when the card is wide enough for it", () => {
    // THE OTHER HALF OF THE RULE, and the one that fails if compaction is applied unconditionally:
    // same number, same format, wider box. Compaction costs precision and is only worth paying for
    // when the alternative is a number hanging off the card.
    withBox(600, 400);
    render(<SmartScalarChart columns={cols} rows={rows} dimension="d" metric="m" unit="month" />);
    expect(screen.getByText("24,730.43")).toBeInTheDocument();
    expect(screen.queryByText(/24\.7k/i)).toBeNull();
  });

  it("still speaks the exact figure it stopped drawing", () => {
    // A screen reader has no use for "24.7k" when the number is 24,730.43 — `BigNumber`'s rule,
    // and the reason compaction is safe to apply without the user having asked for it.
    withBox(NARROW, NARROW);
    render(<SmartScalarChart columns={cols} rows={rows} dimension="d" metric="m" unit="month" />);
    expect(screen.getByRole("img", { name: /24,730\.43/ })).toBeInTheDocument();
  });

  it("leaves a short value alone even on the narrowest card", () => {
    // The negative, and the one an over-eager compaction breaks: six characters or fewer is
    // Metabase's `COMPACT_MIN_LENGTH` floor, below which compacting saves nothing and only costs
    // precision.
    withBox(NARROW, NARROW);
    render(<SmartScalarChart columns={cols} rows={[["2026-11-01", 100], ["2026-12-01", 120]]}
      dimension="d" metric="m" format={{ decimals: 0 }} />);
    expect(screen.getByText("120")).toBeInTheDocument();
  });

  it("caps the font at the narrow-card ceiling on a narrow card", () => {
    // `maxFontSizeRem` returns `NARROW_CARD_MAX_SIZE` (2.5rem) under 240px — Metabase's own
    // constant, ported verbatim. jsdom measures every candidate as fitting, so what this pins is
    // the CEILING the fit is allowed to reach, which is the part that is arithmetic over the box.
    withBox(NARROW, NARROW);
    const { container } = render(
      <SmartScalarChart columns={cols} rows={rows} dimension="d" metric="m" unit="month" />,
    );
    expect(container.querySelector<HTMLElement>(".tabular-nums")!.style.fontSize).toBe("2.5rem");
  });

  it("lets a roomier card reach a bigger ceiling", () => {
    // The same assertion read the other way, and the one that dies if the size goes back to a
    // constant: off the grid, a card past the narrow threshold may reach `NO_GRID_MAX_SIZE`.
    withBox(600, 400);
    const { container } = render(
      <SmartScalarChart columns={cols} rows={rows} dimension="d" metric="m" unit="month" />,
    );
    const value = container.querySelector<HTMLElement>(".tabular-nums")!;
    expect(value.style.fontSize).toBe("4rem");
    expect(value.className).not.toContain("text-[2.5rem]");
  });

  it("earns a bigger ceiling from the card's share of the dashboard grid", () => {
    // `gridCols` has to be THREADED, not merely accepted: `ChartRenderer` passes the card's width
    // in grid columns, exactly as it does for the scalar. A full-width card is `MAX_SIZE_LARGE`.
    withBox(600, 400);
    const { container } = render(
      <SmartScalarChart columns={cols} rows={rows} dimension="d" metric="m" unit="month" gridCols={24} />,
    );
    expect(container.querySelector<HTMLElement>(".tabular-nums")!.style.fontSize).toBe("7rem");
  });

  it("measures the whole card without taking part in its layout", () => {
    // `BigNumber`'s recorded scar: a first attempt gave the measuring element `flex-1 basis-0`, it
    // swallowed the card's spare height, and the captions ended up ~300px below the number. Every
    // numeric check passed; only the screenshot showed it. jsdom cannot see the gap either — what
    // it CAN see is the structure that caused it.
    const { container } = render(
      <SmartScalarChart columns={cols} rows={rows} dimension="d" metric="m" unit="month" />,
    );
    const sizer = container.querySelector("[aria-hidden]");
    expect(sizer, "the measuring element should exist").not.toBeNull();
    expect(sizer!.className).toContain("absolute");
    expect(sizer!.className).not.toContain("flex-1");
  });

  it("keeps the drill on the value, with the exact figure in its name", () => {
    // #413 wired this button; compaction must not cost it the precision it announces.
    render(<SmartScalarChart columns={cols} rows={rows} dimension="d" metric="m" unit="month"
      onValueClick={() => {}} />);
    expect(screen.getByRole("button", { name: /24,730\.43.*Show records/ })).toBeInTheDocument();
  });
});
