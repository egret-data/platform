import { describe, it, expect, vi } from "vitest";
import { render } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";

// DashboardCardView renders react-router <Link>s (the card title and the Edit-question
// affordance), which throw outside a Router.
function renderCard(ui: React.ReactElement) {
  return render(<MemoryRouter>{ui}</MemoryRouter>);
}

const seen: { vizType?: string; params?: unknown } = {};
vi.mock("@/components/chart/ChartRenderer", () => ({
  ChartRenderer: (props: { vizType: string; params: unknown }) => {
    seen.vizType = props.vizType;
    seen.params = props.params;
    return null;
  },
}));
vi.mock("@/api/useChartData", () => ({
  useChartData: () => ({ columns: [{ name: "rev", type: "number" }], rows: [[1]], isLoading: false, error: null }),
}));
// View-mode chart cards read through the object-scoped card-data hook.
vi.mock("@/api/useSavedChartData", () => ({
  useDashboardCardData: () => ({ columns: [{ name: "rev", type: "number" }], rows: [[1]], isLoading: false, error: null }),
}));

import { DashboardCardView } from "../DashboardCardView";

const chart = {
  id: "c", dataset_id: "d", name: "C", description: "", created_by: null, updated_at: "", viz_type: "bar",
  params: {
    version: 2,
    spec: { metrics: ["rev"], dimensions: ["region"], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [] },
    settings: { "cartesian.xAxis": "region", "cartesian.yMetrics": ["rev"], "cartesian.stacked": false },
  },
} as never;
const card = {
  id: "k", card_type: "chart", chart_id: "c",
  options: { vizOverride: { viz_type: "line", "cartesian.stacked": true } },
} as never;

describe("DashboardCardView vizOverride", () => {
  it("renders ChartRenderer with the overridden viz_type + merged display, spec intact", () => {
    renderCard(<DashboardCardView ws="eng" dashboardId="d1" card={card} chart={chart} refreshNonce={0} parameters={[]} paramValues={{}} />);
    expect(seen.vizType).toBe("line");
    expect((seen.params as { settings: Record<string, unknown> }).settings["cartesian.stacked"]).toBe(true);
    expect((seen.params as { spec: unknown }).spec).toEqual((chart as { params: { spec: unknown } }).params.spec);
  });
});
