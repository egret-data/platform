import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import DashboardView from "../DashboardView";

vi.mock("sonner", () => ({ toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn() }) }));
import { toast } from "sonner";

vi.mock("@/components/chart/dashboard/DashboardCardView", () => ({
  DashboardCardView: ({ card }: { card: { id: string } }) => <div data-testid={`card-${card.id}`} />,
}));
vi.mock("@/api/charts", async (orig) => ({
  ...(await orig<typeof import("@/api/charts")>()),
  useCharts: () => ({ data: [] }),
}));

// The two-tab fixture from DashboardView.tabs.test.tsx: tabs A and B, one heading card on each.
const detail = {
  id: "d1", name: "D", description: "", created_by: null, updated_at: "", parameters: [], width: "full",
  tabs: [{ id: "t1", name: "A", position: 0 }, { id: "t2", name: "B", position: 1 }],
  cards: [
    { id: "c1", chart_id: null, card_type: "heading", grid_col: 0, grid_row: 0, size_x: 6, size_y: 1, options: { text: "h" }, tab_id: "t1" },
    { id: "c2", chart_id: null, card_type: "heading", grid_col: 0, grid_row: 1, size_x: 6, size_y: 1, options: { text: "h" }, tab_id: "t2" },
  ],
};
vi.mock("@/api/dashboards", async (orig) => {
  const mod = await (orig as () => Promise<Record<string, unknown>>)();
  return { ...mod,
    useDashboard: () => ({ data: detail, isLoading: false }),
    useDashboardMutations: () => ({ update: { mutateAsync: vi.fn(), isPending: false } }),
  };
});

const renderView = () => render(
  <QueryClientProvider client={new QueryClient()}>
    <MemoryRouter initialEntries={["/explore/eng/dashboards/d1"]}>
      <Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes>
    </MemoryRouter>
  </QueryClientProvider>,
);

const toastCalls = () => (toast as unknown as { mock: { calls: unknown[][] } }).mock.calls;
const lastToast = () => toastCalls().at(-1)!;

/** Pick the option by its LABEL. The `move card to tab` select's item values are draft tab keys
 *  (`tab-N`, minted fresh by `startTabsDraft` on every mount), not names — naming a display string
 *  where the code keys on a raw one is the failure this programme has hit six times. */
async function moveCardToTabNamed(name: string) {
  const user = userEvent.setup();
  await user.click(screen.getByRole("combobox", { name: "move card to tab" }));
  await user.click(await screen.findByRole("option", { name }));
}

beforeEach(() => vi.clearAllMocks());

describe("DashboardView tab delete undo", () => {
  it("deleting an empty tab offers an Undo naming the tab, and restores it", async () => {
    const user = userEvent.setup();
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    // Tab B's card is moved away first so the delete takes the no-dialog fast path.
    fireEvent.click(screen.getByRole("tab", { name: "B" }));
    fireEvent.click(screen.getAllByTestId("dashcard-body")[0]);
    await moveCardToTabNamed("A");
    await user.click(screen.getByRole("button", { name: "tab menu B" }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    expect(screen.queryByRole("tab", { name: "B" })).toBeNull();
    const [message, cfg] = lastToast();
    expect(message).toBe('Deleted "B"');
    act(() => (cfg as { action: { onClick: () => void } }).action.onClick());
    expect(screen.getByRole("tab", { name: "B" })).toBeTruthy();
  });

  it("reassign-and-delete offers the same Undo and restores the cards to their tab", async () => {
    const user = userEvent.setup();
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    await user.click(screen.getByRole("button", { name: "tab menu A" }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    await user.click(screen.getByRole("combobox", { name: "reassign target" }));
    await user.click((await screen.findAllByRole("option"))[0]);
    fireEvent.click(screen.getByRole("button", { name: "reassign and delete" }));
    expect(screen.queryByRole("tab", { name: "A" })).toBeNull();
    const [message, cfg] = lastToast();
    expect(message).toBe('Deleted "A"');
    act(() => (cfg as { action: { onClick: () => void } }).action.onClick());
    expect(screen.getByRole("tab", { name: "A" })).toBeTruthy();
    expect(screen.getByTestId("card-c1")).toBeTruthy();
  });

  it("delete-cards-too offers the same Undo and restores the cards it dropped", async () => {
    // The third delete path. Not in the plan's test list, but it is one of the three call sites
    // the plan requires to funnel through `deleteTabWithUndo`, and an untested third path is how
    // one of them keeps its own inline snapshot.
    const user = userEvent.setup();
    renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    await user.click(screen.getByRole("button", { name: "tab menu A" }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    fireEvent.click(screen.getByRole("button", { name: "delete cards too" }));
    expect(screen.queryByRole("tab", { name: "A" })).toBeNull();
    const [message, cfg] = lastToast();
    expect(message).toBe('Deleted "A"');
    act(() => (cfg as { action: { onClick: () => void } }).action.onClick());
    expect(screen.getByRole("tab", { name: "A" })).toBeTruthy();
    expect(screen.getByTestId("card-c1")).toBeTruthy();
  });
});
