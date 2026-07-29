import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { PreviewPane } from "../PreviewPane";
import { EMPTY_SPEC } from "@/api/charts";

// The card's geometry is what these tests are about, not what ECharts draws inside it.
vi.mock("@/components/chart/echarts/EChartsRenderer", () => ({
  EChartsRenderer: () => <div data-testid="echarts" />,
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
});
