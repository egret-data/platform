import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { FunnelChart } from "../FunnelChart";

// A size-aware option (a function of the measured size) is resolved here the way the real renderer
// resolves it, against a 1440px editor's chart column.
vi.mock("../echarts/EChartsRenderer", () => ({
  EChartsRenderer: ({ option }: { option: Record<string, unknown> | ((size: { width: number; height: number }) => Record<string, unknown>) }) => (
    <div data-testid="echarts" data-option={JSON.stringify(typeof option === "function" ? option({ width: 700, height: 600 }) : option)} />
  ),
}));

const cols = [{ name: "stage", type: "varchar" }, { name: "cnt", type: "double" }];
const rows = [["Visited", 100], ["Signed up", 40], ["Paid", 10]];

const draw = (extra: Record<string, unknown> = {}) =>
  render(<FunnelChart columns={cols} rows={rows} dimension="stage" metric="cnt" {...extra} />);

describe("FunnelChart", () => {
  // The reason this stopped being an ECharts `funnel` series: that series can only draw its labels
  // INSIDE the shape, where a narrow step truncates them. Metabase puts the name above the column
  // and the value below it, outside.
  it("puts the names and the values outside the shape", () => {
    const { container } = draw();
    const bands = container.querySelectorAll("[data-funnel-band]");
    expect(bands.length).toBeGreaterThan(0);
    for (const band of bands) {
      expect(band.textContent).toBe("");
    }
    expect(screen.getByText("Visited")).toBeInTheDocument();
    expect(screen.getByText("100")).toBeInTheDocument();
  });

  // A funnel's FIRST step is the whole flow — there is nothing narrowing yet — so Metabase gives it
  // an info column and draws only the N-1 bands ARRIVING at the later steps. Drawing a band for
  // step 1 and a flat sliver for step N, as this once did, puts the slab at the wrong end.
  it("draws a band per step after the first, not per step", () => {
    const { container } = draw();
    expect(container.querySelectorAll("[data-funnel-band]")).toHaveLength(2);
  });

  // A trapezoid's edges run from `value/max`, centred, and join step i to step i+1. For 100 then 40
  // out of a max of 100 that is a left edge spanning the full height and a right edge 0.3 to 0.7.
  it("shapes each trapezoid from the two values it joins", () => {
    const { container } = draw();
    const pts = [...container.querySelectorAll("[data-funnel-band] polygon")]
      .map((p) => p.getAttribute("points"));
    expect(pts).toEqual(["0,0 1,0.3 1,0.7 0,1", "0,0.3 1,0.45 1,0.55 0,0.7"]);
  });

  // One colour for the whole funnel; the sequence is carried by a falling opacity, which is what
  // makes a Metabase funnel read left to right instead of as one blue shape. Two bands:
  // 1 - i * (0.9/3).
  it("keeps one colour and steps the opacity down", () => {
    const { container } = draw();
    const polys = [...container.querySelectorAll("polygon")];
    expect(new Set(polys.map((p) => p.getAttribute("fill"))).size).toBe(1);
    expect(polys.map((p) => p.getAttribute("opacity"))).toEqual(["1", "0.7"]);
  });

  // Metabase prints the metric's name under the first value — what the whole funnel is counting.
  it("names the metric under the first step only", () => {
    draw();
    expect(screen.getAllByText("cnt")).toHaveLength(1);
  });

  // KI-292: the metric line and the chart's name said `cnt` where the Metric picker shows the
  // dataset's display name (or the chart's own Column title), which reach here as `column_title`.
  it("names the metric and the dimension by their titles, not their database names", () => {
    const { container } = draw({ columnSettings: { cnt: { column_title: "Visitors" }, stage: { column_title: "Stage" } } });
    expect(screen.getByText("Visitors")).toBeInTheDocument();
    expect(screen.queryByText("cnt")).toBeNull();
    const named = container.querySelector("[aria-label^='Funnel chart']")!.getAttribute("aria-label")!;
    expect(named).toContain("Funnel chart of Visitors by Stage.");
  });

  it("titles the bar mode's value axis by the metric's title", () => {
    draw({ type: "bar", columnSettings: { cnt: { column_title: "Visitors" } } });
    const opt = JSON.parse(screen.getByTestId("echarts").getAttribute("data-option")!);
    const yAxis = Array.isArray(opt.yAxis) ? opt.yAxis[0] : opt.yAxis;
    expect(yAxis.name).toBe("Visitors");
  });

  it("shows each later step's share of the first", () => {
    draw();
    expect(screen.getByText("40.00 %")).toBeInTheDocument();
    expect(screen.getByText("10.00 %")).toBeInTheDocument();
  });

  // A step worth zero is the most important thing a funnel can report. Metabase keeps the row.
  it("keeps a step whose value is zero", () => {
    render(
      <FunnelChart columns={cols} rows={[["Visited", 100], ["Paid", 0]]} dimension="stage" metric="cnt" />,
    );
    expect(screen.getByText("Paid")).toBeInTheDocument();
    expect(screen.getByText("0.00 %")).toBeInTheDocument();
  });

  // Metabase's `funnel.type: "bar"` swaps in a genuine bar chart of the same steps, axes and all —
  // not a hand-drawn row of blocks. Each step is its OWN series, which is what gives it its own
  // colour and legend entry, and they are stacked so the four land in four x positions rather than
  // being grouped side by side inside each one.
  // KI-164: built without the measured size, the bar mode left the x axis to ECharts' `interval:
  // "auto"`, which dropped every other stage name at 1440px. With the size the shared label fit
  // decides, and a decided axis draws every name (upright or rotated) or none — never a subset.
  it("lets the x-axis label fit decide in bar mode, so no step loses its name", () => {
    draw({ type: "bar" });
    const opt = JSON.parse(screen.getByTestId("echarts").getAttribute("data-option")!);
    const xAxis = Array.isArray(opt.xAxis) ? opt.xAxis[0] : opt.xAxis;
    expect(xAxis.axisLabel.interval).toBe(0);
  });

  it("renders a real bar chart, one series per step, in bar mode", () => {
    const { container } = draw({ type: "bar" });
    expect(container.querySelectorAll("polygon")).toHaveLength(0);
    const opt = JSON.parse(screen.getByTestId("echarts").getAttribute("data-option")!);
    expect(opt.series.map((s: { name: string }) => s.name)).toEqual(["Visited", "Signed up", "Paid"]);
    expect(new Set(opt.series.map((s: { type: string }) => s.type))).toEqual(new Set(["bar"]));
    // One shared, DEFINED stack. Asserting only that the values agree would pass on `undefined`
    // everywhere, which is exactly the unstacked chart this is meant to rule out.
    const stacks = opt.series.map((s: { stack?: string }) => s.stack);
    expect(stacks.every((v: unknown) => typeof v === "string" && v === stacks[0])).toBe(true);
    const xAxis = Array.isArray(opt.xAxis) ? opt.xAxis[0] : opt.xAxis;
    expect(xAxis.data).toEqual(["Visited", "Signed up", "Paid"]);
    // A step contributes to its own column only; the rest of its row is empty.
    expect(opt.series[0].data.filter((v: unknown) => v != null)).toHaveLength(1);
  });

  it("says so rather than drawing an empty frame when there is nothing to draw", () => {
    render(<FunnelChart columns={cols} rows={[]} dimension="stage" metric="cnt" />);
    expect(screen.getByText("No data")).toBeInTheDocument();
  });

  /**
   * BECOMING CLICKABLE MUST NOT CHANGE WHAT IS DRAWN. The step columns turn from `div` into
   * `button` when a drill handler arrives, and a button carries UA styling a div does not — so this
   * pins that the wrapper contributes no box (no padding, margin, width or border of its own) and
   * that every child inside it is byte-identical. The funnel's typography is measured to the
   * hundredth of a pixel elsewhere in this suite; a stray `p-1` here would move all of it, on the
   * dashboard only, where none of those specs render.
   */
  it("draws the identical columns whether or not it is clickable", () => {
    const plain = render(<FunnelChart columns={cols} rows={rows} dimension="stage" metric="cnt" />);
    const live = render(
      <FunnelChart columns={cols} rows={rows} dimension="stage" metric="cnt" onStepClick={() => {}} />,
    );
    const steps = (c: HTMLElement) => [...c.querySelectorAll("[data-funnel-step]")];
    const a = steps(plain.container);
    const b = steps(live.container);
    expect(b).toHaveLength(a.length);
    a.forEach((el, i) => {
      expect(el.tagName).toBe("DIV");
      expect(b[i].tagName).toBe("BUTTON");
      // The drawn contents, unchanged.
      expect(b[i].innerHTML).toBe(el.innerHTML);
      // Every class the div had, still there — plus only affordance classes, none of them boxy.
      for (const c of el.className.split(" ")) expect(b[i].classList.contains(c)).toBe(true);
      const extra = b[i].className.split(" ").filter((c) => !el.className.split(" ").includes(c));
      expect(extra.join(" ")).toBe("cursor-pointer text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring");
    });
  });
});

/**
 * THE BAR MODE'S IDENTITY IS THE RAW CELL; ONLY ITS LABELS ARE FORMATTED.
 *
 * `funnelSteps` draws each step through the dimension's own `column_settings`, and in bar mode the
 * step name is doing three jobs at once — the x category, the series id, and the string
 * `onPointClick` matches back to decide which step was clicked. Two raw values can format to ONE
 * label ("MMMM YYYY" collapses every day of a month onto "January 2026"), so keying the option on
 * the drawn name would make the second of them unreachable and answer its click with the first.
 *
 * FOUND BY MUTATION, not by review: reverting `keys` to `steps.map((s) => s.name)` left the whole
 * funnel suite green. That is a fixture that could not reach the arm rather than a weak assertion —
 * nothing here had ever drawn a bar-mode funnel over a formatted dimension — so the fixture is what
 * this adds. Its two rows fall in different months on purpose, so the formatted labels are DISTINCT
 * and the assertions cannot pass by both sides collapsing to the same string.
 */
describe("FunnelChart — bar mode over a formatted dimension", () => {
  const dated = [{ name: "d", type: "timestamp" }, { name: "cnt", type: "double" }];
  const datedRows = [["2026-01-01", 100], ["2026-02-01", 40]];
  const optionOf = () => JSON.parse(screen.getByTestId("echarts").getAttribute("data-option")!);

  it("keys the categories and the series on the raw cells", () => {
    render(
      <FunnelChart columns={dated} rows={datedRows} dimension="d" metric="cnt" type="bar"
        columnSettings={{ d: { date_style: "MMMM YYYY" } }} />,
    );
    const opt = optionOf();
    const xAxis = Array.isArray(opt.xAxis) ? opt.xAxis[0] : opt.xAxis;
    expect(xAxis.data).toEqual(["2026-01-01", "2026-02-01"]);
    expect(opt.series.map((s: { __metric?: string; name: string }) => s.__metric ?? s.name))
      .toEqual(["2026-01-01", "2026-02-01"]);
  });

  // The formatted string still has to reach the reader — this is the half that would pass if the
  // fix were "never format anything in bar mode", which is the other way to get the identity right
  // and the labels wrong.
  it("labels those series, and the axis ticks, with the formatted names", () => {
    render(
      <FunnelChart columns={dated} rows={datedRows} dimension="d" metric="cnt" type="bar"
        columnSettings={{ d: { date_style: "MMMM YYYY" } }} />,
    );
    const opt = optionOf();
    expect(opt.series.map((s: { name: string }) => s.name)).toEqual(["January 2026", "February 2026"]);
    const xAxis = Array.isArray(opt.xAxis) ? opt.xAxis[0] : opt.xAxis;
    // `axisLabel.formatter` is a function and does not survive `JSON.stringify`, so the tick text is
    // checked through the builder that produced it rather than off the serialised option — see
    // `echarts/__tests__/compactXLabels.test.ts`, which owns that formatter's own rules.
    expect(xAxis.data).toEqual(["2026-01-01", "2026-02-01"]);
  });

  // With no stored format the two are the same string, which is the state every funnel shipped in
  // and the one a too-eager fix would break.
  it("leaves an unformatted dimension exactly as it was", () => {
    render(<FunnelChart columns={cols} rows={rows} dimension="stage" metric="cnt" type="bar" />);
    const opt = optionOf();
    const xAxis = Array.isArray(opt.xAxis) ? opt.xAxis[0] : opt.xAxis;
    expect(xAxis.data).toEqual(["Visited", "Signed up", "Paid"]);
    expect(opt.series.map((s: { name: string }) => s.name)).toEqual(["Visited", "Signed up", "Paid"]);
  });
});
