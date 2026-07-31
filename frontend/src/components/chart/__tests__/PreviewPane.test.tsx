import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { PreviewPane } from "../PreviewPane";
import { EMPTY_SPEC } from "@/api/charts";

/** The click a test wants the mocked chart to report; set per test. */
let mockClickPayload: {
  name?: string;
  seriesName?: string;
  treePathInfo?: { name?: string }[];
  data?: unknown;
  x: number;
  y: number;
} = { name: "EU", seriesName: "rev", x: 3, y: 4 };

// The card's geometry is what most of these tests are about, not what ECharts draws inside it —
// but the drill tests below need the one thing the real renderer contributes to this seam: a
// click coming back OUT of the canvas. So the stand-in is a button that reports one.
vi.mock("@/components/chart/echarts/EChartsRenderer", () => ({
  EChartsRenderer: ({ onPointClick }: { onPointClick?: (p: unknown) => void }) => (
    <button data-testid="echarts" onClick={() => onPointClick?.(mockClickPayload)} />
  ),
}));

const base = {
  viz: "bar" as const,
  params: { spec: EMPTY_SPEC },
  columns: [],
  rows: [],
  metricTypes: {},
  columnSettings: {},
  conditionalFormatting: [],
  isRunning: false,
  error: null as string | null,
  finished: false,
  onRetry: vi.fn(),
  onCellDrill: vi.fn(),
  emptyTitle: "Set up your Bar",
  emptyHint: "Add: X-axis, a metric",
};

describe("PreviewPane", () => {
  beforeEach(() => {
    mockClickPayload = { name: "EU", seriesName: "rev", x: 3, y: 4 };
  });

  it("shows the empty state before a run", () => {
    render(<PreviewPane {...base} />);
    expect(screen.getByText("Set up your Bar")).toBeInTheDocument();
    expect(screen.getByText("Add: X-axis, a metric")).toBeInTheDocument();
  });

  it("shows the loading state while running, and no empty state with it", () => {
    render(<PreviewPane {...base} isRunning />);
    expect(screen.getByRole("status", { name: /loading chart/i })).toBeInTheDocument();
    expect(screen.queryByText("Set up your Bar")).not.toBeInTheDocument();
  });

  it("shows the error with a retry instead of the empty state", () => {
    render(<PreviewPane {...base} error="boom" />);
    expect(screen.getByText("boom")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /retry/i })).toBeInTheDocument();
    expect(screen.queryByText("Set up your Bar")).not.toBeInTheDocument();
  });

  it("renders anything passed as children above the preview", () => {
    render(<PreviewPane {...base}>{<p>chips</p>}</PreviewPane>);
    expect(screen.getByText("chips")).toBeInTheDocument();
  });

  // jsdom does no layout, so the pixel height itself can't be asserted here — what CAN be
  // pinned is the thing that decided it: the card sizes from the space its parent hands it
  // (flex-1 inside ChartExplore's flex column, see that route's own test) rather than from a
  // fraction of the viewport, which at 920px threw away ~230px of a 783px parent on every
  // chart type.
  it("sizes the chart card from its parent, not from a viewport fraction", () => {
    const { container } = render(<PreviewPane {...base} finished />);
    const card = container.querySelector("[data-chart-card]");
    expect(card).not.toBeNull();
    expect(card!.className).toContain("flex-1");
    // A floor so a short window still gets a usable chart instead of a collapsed sliver.
    expect(card!.className).toContain("min-h-[20rem]");
    expect(card!.className).not.toMatch(/\bh-\[\d+vh\]/);
  });

  it("renders the table surface only when the table props are supplied", () => {
    const columns = [{ name: "region", type: "varchar" }];
    const rows = [["North"]];
    const { rerender } = render(
      <PreviewPane {...base} viz="table" finished columns={columns} rows={rows}
        table={{ state: {}, onStateChange: vi.fn(), dimensionColumns: [] }} />,
    );
    expect(screen.getByText("region")).toBeInTheDocument();
    // Without the table props there is nothing viz-specific left to draw.
    rerender(<PreviewPane {...base} viz="table" finished columns={columns} rows={rows} />);
    expect(screen.queryByText("region")).not.toBeInTheDocument();
  });

  // THE SEAM THESE EXIST FOR: this component forwarded `onCellDrill` to its table and pivot
  // surfaces but handed ChartRenderer nothing, and ChartRenderer guards every chart type's click
  // handler on `onValueClick &&` — so no ECharts chart had ever been clickable in the chart
  // editor. Drill worked on dashboards (DashboardView passes it) and nowhere else, which is why
  // it went unnoticed. Tested HERE, at the prop boundary, so the prop cannot silently go missing
  // again; ChartRenderer's own tests can't see it, because they pass onValueClick themselves.
  describe("drill-through", () => {
    it("makes a cartesian chart clickable, not just the table surfaces", () => {
      const onCellDrill = vi.fn();
      render(
        <PreviewPane {...base} finished onCellDrill={onCellDrill}
          viz="bar"
          params={{ spec: EMPTY_SPEC, cartesian: { xAxis: "region", yMetrics: ["rev"] } }}
          columns={[{ name: "region", type: "varchar" }, { name: "rev", type: "double" }]}
          rows={[["EU", 10]]} />,
      );
      fireEvent.click(screen.getByTestId("echarts"));
      expect(onCellDrill).toHaveBeenCalledWith({ dims: [{ col: "region", value: "EU" }] }, 3, 4);
    });

    // The pie's own drill reads the datum's aggregation key, and nothing had ever exercised that
    // path from a real editor click — only from ChartRenderer directly.
    it("drills a pie slice on its whole ancestor path", () => {
      const onCellDrill = vi.fn();
      render(
        <PreviewPane {...base} finished onCellDrill={onCellDrill}
          viz="pie"
          params={{ spec: EMPTY_SPEC, pie: { dimensions: ["region", "category"], metric: "rev" } }}
          columns={[
            { name: "region", type: "varchar" }, { name: "category", type: "varchar" },
            { name: "rev", type: "double" },
          ]}
          rows={[["EU", "Widgets", 10]]} />,
      );
      // `␟` is options/pie.ts' KEY_SEP: this is the ring-1 slice "Widgets" under "EU".
      mockClickPayload = { name: "Widgets", data: { key: "EU␟Widgets" }, x: 5, y: 6 };
      fireEvent.click(screen.getByTestId("echarts"));
      expect(onCellDrill).toHaveBeenCalledWith(
        { dims: [{ col: "region", value: "EU" }, { col: "category", value: "Widgets" }] }, 5, 6,
      );
    });
  });
});
