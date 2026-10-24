import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";

// DashboardCardView renders react-router <Link>s (the card title and the Edit-question
// affordance), which throw outside a Router.
function renderCard(ui: React.ReactElement) {
  return render(<MemoryRouter>{ui}</MemoryRouter>);
}

// Mock ChartRenderer faithfully: chart viz renders an <svg> (ECharts), table renders an HTML
// table (no <svg>). This lets us assert the PNG item only appears when the *chart* svg exists —
// the export menu must read the render container, not the whole tile (whose affordance holds a
// Download icon <svg> that would otherwise make every card — even tables — offer PNG).
vi.mock("@/components/chart/ChartRenderer", () => ({
  ChartRenderer: ({ vizType }: { vizType: string }) =>
    vizType === "table" ? <table data-testid="cr-table" /> : <div data-chart-image=""><svg data-testid="chart-svg" /></div>,
}));
// One mock fn drives both the edit-mode (useChartData) and view-mode (useDashboardCardData) paths,
// so each test's mockReturnValue applies whichever mode it renders.
const useChartData = vi.fn();
vi.mock("@/api/useChartData", () => ({ useChartData: (...a: unknown[]) => useChartData(...a) }));
vi.mock("@/api/useSavedChartData", () => ({ useDashboardCardData: (...a: unknown[]) => useChartData(...a) }));

import { DashboardCardView } from "../DashboardCardView";

const chart = { id: "c", dataset_id: "d", name: "C", description: "", created_by: null, updated_at: "", viz_type: "bar", params: { version: 2, spec: { metrics: ["rev"], dimensions: ["region"], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [] }, settings: { "cartesian.xAxis": "region", "cartesian.yMetrics": ["rev"] } } } as never;
const tableChart = { ...(chart as object), viz_type: "table" } as never;
const card = { id: "k", card_type: "chart", chart_id: "c", options: {} } as never;

describe("DashboardCardView export affordance", () => {
  it("shows the export button for a loaded chart card in view mode", () => {
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={card} chart={chart} parameters={[]} paramValues={{}} />);
    expect(screen.getByRole("button", { name: /export chart/i })).toBeTruthy();
  });
  it("hides the export button while loading", () => {
    useChartData.mockReturnValue({ columns: [], rows: [], isLoading: true, error: null });
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={card} chart={chart} parameters={[]} paramValues={{}} />);
    expect(screen.queryByRole("button", { name: /export chart/i })).toBeNull();
  });
  it("hides the export button in edit mode", () => {
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={card} chart={chart} parameters={[]} paramValues={{}} editing />);
    expect(screen.queryByRole("button", { name: /export chart/i })).toBeNull();
  });

  it("offers .png for a chart card (chart svg present in the render container)", async () => {
    const user = userEvent.setup();
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={card} chart={chart} parameters={[]} paramValues={{}} />);
    await user.click(screen.getByRole("button", { name: /export chart/i }));
    expect(await screen.findByRole("radio", { name: ".csv" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: ".png" })).toBeInTheDocument();
  });

  // Regression: the export popover must read the render-only container, not the whole tile. The
  // tile's hover cluster now holds TWO icons — a Download and an Edit-question pencil — and if the
  // popover pointed at the tile, a table card would find one of them and wrongly offer .png,
  // exporting a 16×16 icon.
  it("does NOT offer .png for a table card (no chart svg), despite the pencil in the hover cluster", async () => {
    const user = userEvent.setup();
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={card} chart={tableChart} parameters={[]} paramValues={{}} />);
    await user.click(screen.getByRole("button", { name: /export chart/i }));
    expect(await screen.findByRole("radio", { name: ".csv" })).toBeInTheDocument();
    expect(screen.queryByRole("radio", { name: ".png" })).toBeNull();
  });
});

describe("DashboardCardView chart link", () => {
  it("titles the card with a link to its chart", () => {
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={card} chart={chart} parameters={[]} paramValues={{}} />);
    expect(screen.getByRole("link", { name: "C" })).toHaveAttribute("href", "/explore/eng/charts/c");
  });

  it("offers Edit question beside the export trigger", () => {
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={card} chart={chart} parameters={[]} paramValues={{}} />);
    expect(screen.getByRole("link", { name: "Edit question" })).toHaveAttribute("href", "/explore/eng/charts/c");
  });

  it("offers neither while editing", () => {
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={card} chart={chart} parameters={[]} paramValues={{}} editing />);
    expect(screen.queryByRole("link")).toBeNull();
  });

  it("offers neither when the title override hides the header", () => {
    const hidden = { ...(card as object), options: { title: "" } } as never;
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={hidden} chart={chart} parameters={[]} paramValues={{}} />);
    expect(screen.queryByRole("link")).toBeNull();
  });

  it("shows options.description behind the card's info affordance", async () => {
    const user = userEvent.setup();
    const described = { ...(card as object), options: { description: "Net of refunds." } } as never;
    useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={described} chart={chart} parameters={[]} paramValues={{}} />);
    await user.click(screen.getByRole("button", { name: "Card description" }));
    expect(await screen.findByText("Net of refunds.")).toBeInTheDocument();
  });
});
