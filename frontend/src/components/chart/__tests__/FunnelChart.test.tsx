import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { FunnelChart } from "../FunnelChart";

vi.mock("../echarts/EChartsRenderer", () => ({
  EChartsRenderer: ({ option }: { option: Record<string, unknown> }) => (
    <div data-testid="echarts" data-option={JSON.stringify(option)} />
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
