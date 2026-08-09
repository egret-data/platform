import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, within, act } from "@testing-library/react";
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

const detail = {
  id: "d1", name: "Sales", description: "", created_by: null, updated_at: "",
  parameters: [], tabs: [], width: "full",
  cards: [
    { id: "c1", chart_id: null, card_type: "heading", grid_col: 0, grid_row: 0, size_x: 6, size_y: 1, options: { text: "h1" }, tab_id: null },
    { id: "c2", chart_id: null, card_type: "heading", grid_col: 6, grid_row: 0, size_x: 6, size_y: 1, options: { text: "h2" }, tab_id: null },
  ],
};
vi.mock("@/api/dashboards", async (orig) => {
  const mod = await (orig as () => Promise<Record<string, unknown>>)();
  return { ...mod,
    useDashboard: () => ({ data: detail, isLoading: false }),
    useDashboardMutations: () => ({ update: { mutateAsync: vi.fn(), isPending: false } }),
  };
});

const renderView = () => {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/dashboards/d1"]}>
        <Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
};

const enterEdit = () => fireEvent.click(screen.getByRole("button", { name: "Edit" }));
const bannerCancel = () => within(screen.getByText(/You're editing this dashboard/).closest("div")!)
  .getByRole("button", { name: "Cancel" });

beforeEach(() => vi.clearAllMocks());

describe("DashboardView discard confirmation", () => {
  it("exits immediately when nothing was changed", () => {
    // The confirmation exists to stop data loss, not to add a click. A clean draft must not prompt.
    // This is also the vitest half of the compactType="vertical" hazard: RGL mounts, emits its
    // first onLayoutChange, and the draft must still fingerprint identically afterwards.
    renderView();
    enterEdit();
    fireEvent.click(bannerCancel());
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(screen.getByRole("button", { name: "Edit" })).toBeTruthy();
  });

  it("prompts on a dirty draft and Cancel-the-cancel keeps both the edit and the change", async () => {
    const user = userEvent.setup();
    renderView();
    enterEdit();
    const nameInput = screen.getByLabelText("dashboard name") as HTMLInputElement;
    await user.clear(nameInput);
    await user.type(nameInput, "Revenue");
    fireEvent.click(bannerCancel());
    const dialog = screen.getByRole("alertdialog");
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect((screen.getByLabelText("dashboard name") as HTMLInputElement).value).toBe("Revenue");
  });

  it("Discard changes leaves edit mode and drops the draft", async () => {
    const user = userEvent.setup();
    renderView();
    enterEdit();
    const nameInput = screen.getByLabelText("dashboard name") as HTMLInputElement;
    await user.clear(nameInput);
    await user.type(nameInput, "Revenue");
    fireEvent.click(bannerCancel());
    await user.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Discard changes" }));
    expect(screen.queryByLabelText("dashboard name")).toBeNull();
    expect(screen.getByRole("heading", { name: "Sales" })).toBeTruthy();
  });

  it("prompts when a card was removed, not only when the name changed", () => {
    // The name is the cheapest draft field to dirty; the one that actually cost a user their work
    // was a structural edit, so at least one structural case has to be covered here too.
    renderView();
    enterEdit();
    fireEvent.click(screen.getAllByLabelText("remove card")[0]);
    fireEvent.click(bannerCancel());
    expect(screen.getByRole("alertdialog")).toBeTruthy();
  });

  it("prompts when only the fixed/full width switch was flipped", async () => {
    // NOT in the plan: W3 merged `draftWidth` into the draft after this plan was written. Without
    // it in the fingerprint, the one edit W3 added would still be discarded with no prompt.
    const user = userEvent.setup();
    renderView();
    enterEdit();
    await user.click(screen.getByRole("button", { name: "dashboard actions" }));
    await user.click(await screen.findByRole("switch", { name: "Full width" }));
    fireEvent.click(bannerCancel());
    expect(screen.getByRole("alertdialog")).toBeTruthy();
  });
});

const toastCalls = () => (toast as unknown as { mock: { calls: unknown[][] } }).mock.calls;

describe("DashboardView undo toasts", () => {
  const undoLastToast = () => {
    const [, cfg] = toastCalls().at(-1)!;
    (cfg as { action: { onClick: () => void } }).action.onClick();
  };

  it("removing a card offers an Undo that puts it back", () => {
    renderView();
    enterEdit();
    expect(screen.getByTestId("card-c1")).toBeTruthy();
    fireEvent.click(screen.getAllByLabelText("remove card")[0]);
    expect(screen.queryByTestId("card-c1")).toBeNull();
    const [message] = toastCalls().at(-1)!;
    expect(message).toBe("Removed card");
    act(() => undoLastToast());
    expect(screen.getByTestId("card-c1")).toBeTruthy();
  });

  it("removing a card and undoing leaves the draft clean, so Cancel does not prompt", () => {
    // The strongest statement this suite can make about undo: it restores the draft so exactly
    // that the dirtiness check from Task 1 cannot tell the round trip happened.
    renderView();
    enterEdit();
    fireEvent.click(screen.getAllByLabelText("remove card")[0]);
    act(() => undoLastToast());
    fireEvent.click(bannerCancel());
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });
});
