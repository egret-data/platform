import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import DashboardView from "../DashboardView";

// KI-065: a filter's slug (its name in URLs and in `{{slug}}`) was assigned once as `filter`,
// `filter_2`, … and could be neither seen nor changed; renaming the filter did not touch it.
vi.mock("@/components/chart/dashboard/DashboardCardView", () => ({ DashboardCardView: ({ card }: { card: { id: string } }) => <div data-testid={`disp-${card.id}`} /> }));
vi.mock("@/api/charts", async (orig) => ({ ...(await (orig as () => Promise<Record<string, unknown>>)()), useCharts: () => ({ data: [] }) }));
const update = { mutateAsync: vi.fn().mockResolvedValue({}), isPending: false };
const detail = {
  id: "d1", name: "D", description: "", created_by: null, updated_at: "", tabs: [], width: "full",
  parameters: [{ id: "p1", name: "Region", slug: "region", type: "category", operator: "is" }],
  cards: [{ id: "c1", chart_id: null, card_type: "heading", grid_col: 0, grid_row: 0, size_x: 6, size_y: 1, options: { text: "Sales in {{region}} ({{ region }}, {{region_2}})" }, tab_id: null }],
};
vi.mock("@/api/dashboards", async (orig) => {
  const m = await (orig as () => Promise<Record<string, unknown>>)();
  return { ...m, useDashboard: () => ({ data: detail, isLoading: false }), useDashboardMutations: () => ({ update }) };
});
const renderView = () => render(<QueryClientProvider client={new QueryClient()}><MemoryRouter initialEntries={["/explore/eng/dashboards/d1"]}><Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes></MemoryRouter></QueryClientProvider>);

type Saved = { parameters: { name: string; slug: string }[]; cards: { options: { text?: string } }[] };
const saved = () => update.mutateAsync.mock.calls.at(-1)![0] as Saved;

describe("DashboardView filter slug (KI-065)", () => {
  beforeEach(() => update.mutateAsync.mockClear());

  it("a new filter's URL name follows its name", async () => {
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.click(screen.getByRole("button", { name: "add filter" }));
    expect((screen.getByLabelText("parameter URL name") as HTMLInputElement).value).toBe("filter");
    fireEvent.change(screen.getByLabelText("parameter name"), { target: { value: "Order status" } });
    expect((screen.getByLabelText("parameter URL name") as HTMLInputElement).value).toBe("order_status");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(update.mutateAsync).toHaveBeenCalled());
    expect(saved().parameters.map((p) => p.slug)).toEqual(["region", "order_status"]);
  });

  it("renaming a saved filter keeps its URL name, so links and {{slug}} keep working", async () => {
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.click(screen.getByRole("button", { name: "Region" }));
    fireEvent.change(screen.getByLabelText("parameter name"), { target: { value: "Market" } });
    expect((screen.getByLabelText("parameter URL name") as HTMLInputElement).value).toBe("region");
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(update.mutateAsync).toHaveBeenCalled());
    expect(saved().parameters[0]).toMatchObject({ name: "Market", slug: "region" });
  });

  it("changing the URL name rewrites the {{slug}} references on the dashboard's cards", async () => {
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    fireEvent.click(screen.getByRole("button", { name: "Region" }));
    const slug = screen.getByLabelText("parameter URL name");
    fireEvent.change(slug, { target: { value: "Sales Market" } });
    fireEvent.blur(slug);
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(update.mutateAsync).toHaveBeenCalled());
    expect(saved().parameters[0].slug).toBe("sales_market");
    expect(saved().cards[0].options.text).toBe("Sales in {{sales_market}} ({{sales_market}}, {{region_2}})");
  });
});
