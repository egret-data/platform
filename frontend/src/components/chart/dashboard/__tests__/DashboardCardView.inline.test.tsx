import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import type { DashboardParameter } from "@/api/dashboards";

// DashboardCardView renders react-router <Link>s (the card title and the Edit-question pencil),
// which throw outside a Router.
function renderCard(ui: React.ReactElement) {
  return render(<MemoryRouter>{ui}</MemoryRouter>);
}

vi.mock("@/components/chart/ChartRenderer", () => ({
  ChartRenderer: () => <svg data-testid="chart-svg" />,
}));
const useChartData = vi.fn();
vi.mock("@/api/useChartData", () => ({ useChartData: (...a: unknown[]) => useChartData(...a) }));
vi.mock("@/api/useSavedChartData", () => ({ useDashboardCardData: (...a: unknown[]) => useChartData(...a) }));

import { DashboardCardView } from "../DashboardCardView";

const chart = { id: "c", dataset_id: "d", name: "Orders", description: "", created_by: null, updated_at: "", viz_type: "bar", params: { spec: { metrics: [], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [] } } } as never;
const region: DashboardParameter = { id: "p1", name: "Region", slug: "region", type: "category", operator: "is" };

const inline = {
  parameters: [region],
  values: {} as Record<string, unknown>,
  optionsBySlug: { region: ["EU"] },
  onChange: vi.fn(),
  activeId: null,
};

const chartCard = { id: "k", card_type: "chart", chart_id: "c", options: { inlineParameters: ["p1"] } } as never;
const headingCard = { id: "h", card_type: "heading", chart_id: null, options: { text: "Sales", inlineParameters: ["p1"] } } as never;

const loaded = () => useChartData.mockReturnValue({ columns: [{ name: "a", type: "x" }], rows: [["1"]], isLoading: false, error: null });

describe("a chart card hosting a filter", () => {
  it("draws the filter in the SAME row as the title", () => {
    loaded();
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={chartCard} chart={chart} parameters={[region]} paramValues={{}} inline={inline} />);
    const pill = screen.getByRole("button", { name: "Region" });
    const title = screen.getByRole("link", { name: "Orders" });
    const header = screen.getByTestId("dashcard-header");
    expect(header.contains(title)).toBe(true);
    expect(pill.closest("[data-testid='dashcard-header']")).toBe(header);
  });

  it("reserves the WHOLE hover cluster, which W5 made two buttons wide", () => {
    // The pencil and the export menu are both `absolute` inside `right-1`, so neither takes any
    // layout width. 4 + 36 + 2 + 36 = 78px → pr-20. This collision is only visible on hover, so
    // it survives every static review — which is why the class is asserted.
    loaded();
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={chartCard} chart={chart} parameters={[region]} paramValues={{}} inline={inline} />);
    expect(screen.getByRole("link", { name: "Edit question" })).toBeTruthy();
    expect(screen.getByRole("button", { name: /export chart/i })).toBeTruthy();
    expect(screen.getByTestId("dashcard-header").className).toContain("pr-20");
  });

  it("reserves only one button's width when only one is offered", () => {
    // Editing suppresses BOTH the pencil (dashcard-href.ts:24) and the export menu, so the row
    // needs no gutter at all; a card that is loading keeps its pencil and loses its export.
    loaded();
    const { unmount } = renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={chartCard} chart={chart} parameters={[region]} paramValues={{}} editing inline={inline} />);
    expect(screen.getByTestId("dashcard-header").className).toContain("pr-4");
    unmount();
    useChartData.mockReturnValue({ columns: [], rows: [], isLoading: true, error: null });
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={chartCard} chart={chart} parameters={[region]} paramValues={{}} inline={inline} />);
    expect(screen.getByTestId("dashcard-header").className).toContain("pr-10");
  });

  it("draws a header row for the filters even when the title is hidden", () => {
    // options.title === "" means "hide the header" (DashboardCardView.tsx). The filters still
    // need somewhere to live, so the row renders with an empty title.
    loaded();
    const hidden = { ...(chartCard as object), options: { title: "", inlineParameters: ["p1"] } } as never;
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={hidden} chart={chart} parameters={[region]} paramValues={{}} inline={inline} />);
    expect(screen.queryByText("Orders")).toBeNull();
    expect(screen.getByRole("button", { name: "Region" })).toBeTruthy();
  });

  it("keeps the filter reachable on a card whose chart has gone missing", () => {
    // Otherwise the filter is absent from the top bar (a card claims it) AND absent from the
    // board — unreachable until the dashboard is hand-edited.
    useChartData.mockReturnValue({ columns: [], rows: [], isLoading: false, error: null });
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={chartCard} chart={undefined} parameters={[region]} paramValues={{}} inline={inline} />);
    expect(screen.getByText("Chart not found")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Region" })).toBeTruthy();
  });

  it("renders exactly as before for a card that hosts nothing", () => {
    loaded();
    const plain = { ...(chartCard as object), options: {} } as never;
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={plain} chart={chart} parameters={[region]} paramValues={{}} />);
    expect(screen.getByRole("link", { name: "Orders" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Region" })).toBeNull();
    // W5's header is untouched on a card with no inline filters: no gutter is reserved beyond the
    // symmetric px-4 the row has always had.
    expect(screen.getByTestId("dashcard-header").className).toContain("pr-4");
  });

  it("keeps W5's description popover beside a hosted filter", () => {
    // The two share the row. A restructure that dropped the description would be invisible until
    // someone looked for it, because it is behind an affordance.
    loaded();
    const described = { ...(chartCard as object), options: { description: "Net of refunds.", inlineParameters: ["p1"] } } as never;
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={described} chart={chart} parameters={[region]} paramValues={{}} inline={inline} />);
    expect(screen.getByRole("button", { name: "Card description" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Region" })).toBeTruthy();
  });
});

describe("a heading card hosting a filter", () => {
  it("draws the heading text and the filter in one row, in view mode", () => {
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={headingCard} chart={undefined} parameters={[region]} paramValues={{}} inline={inline} />);
    expect(screen.getByText("Sales")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Region" })).toBeTruthy();
  });

  it("keeps the heading editable with a filter beside it", () => {
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={headingCard} chart={undefined} parameters={[region]} paramValues={{}} editing onEditText={vi.fn()} inline={{ ...inline, onSelect: vi.fn() }} />);
    expect(screen.getByLabelText("heading text")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Region" })).toBeTruthy();
  });

  it("leaves a heading that hosts nothing exactly as it was", () => {
    const plain = { ...(headingCard as object), options: { text: "Sales" } } as never;
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={plain} chart={undefined} parameters={[region]} paramValues={{}} />);
    expect(screen.getByText("Sales")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Region" })).toBeNull();
  });
});
