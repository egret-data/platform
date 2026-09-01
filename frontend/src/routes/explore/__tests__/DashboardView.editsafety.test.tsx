import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, within, act, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import DashboardView from "../DashboardView";
import { HINT_HEADING_CARD } from "@/components/chart/dashboard/card-map-state";

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

/**
 * A button inside the discard dialog, once the dialog can actually be clicked.
 *
 * RADIX OPENS THIS DIALOG IN TWO COMMITS, AND THE FIRST ONE IS UNCLICKABLE. `DismissableLayer`
 * sets `document.body { pointer-events: none }` in an effect and registers itself in a context
 * `Set`; only the re-render that the registration schedules gives the content its own
 * `pointer-events: auto`. Between the two, `pointer-events: none` is what the buttons inherit —
 * so `getByRole("alertdialog")` finds a dialog that `user-event` refuses to click with
 * "Unable to perform pointer interaction as the element has `pointer-events: none`".
 *
 * MEASURED, because the obvious reading of this ("a slow CI, a flake") is wrong and the fix that
 * follows from it would be wrong too. Printing the inline/computed chain for the Cancel button
 * straight after `fireEvent.click(bannerCancel())`:
 *
 *   node 20 and node 24, synchronously   content div inline=[]      button computed=[none]
 *   node 24, after one awaited tick      content div inline=[auto]  button computed=[auto]
 *   node 20, after one awaited tick      content div inline=[]      button computed=[none]
 *   node 20, after `waitFor`             content div inline=[auto]  button computed=[auto]   (2 polls)
 *
 * So the dialog is unclickable on BOTH engines at the moment the old code clicked it; node 24
 * merely happened to flush the second commit inside `user.click`'s own await and node 20 did not.
 * The test was racing on every machine and passing on some — which is why it went red the first
 * time CI ran it (CI is node 20; `.github/workflows/ci.yml` pins it) and not before.
 *
 * `await screen.findByRole("alertdialog")` — the wait `control-plane/__tests__/Policies.test.tsx`
 * uses for its confirm dialog — is NOT enough here and the table above is why: it resolves as soon
 * as the ROLE exists, which is the first commit. It works there because that tree settles in one
 * tick; it does not work in this one. Waiting on the property that was actually being raced is the
 * only wait that holds on both engines, so that is what this does — and it still asserts the click
 * is real, unlike swapping to `fireEvent` (which skips the pointer-events check entirely, the way
 * `sqllab/__tests__/SavedTab.test.tsx` does) and would have made this green while asserting less.
 */
async function dialogButton(name: string): Promise<HTMLElement> {
  const dialog = await screen.findByRole("alertdialog");
  const button = within(dialog).getByRole("button", { name });
  await waitFor(() => expect(window.getComputedStyle(button).pointerEvents).not.toBe("none"));
  return button;
}

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
    await user.click(await dialogButton("Cancel"));
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
    await user.click(await dialogButton("Discard changes"));
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

describe("the filter-mapping overlay reaches every card kind", () => {
  it("explains itself on a heading card, which used to render no overlay at all", () => {
    // The old JSX condition was `activeParamId && c.card_type === "chart" && baseChart`, so every
    // non-chart card beside a chart got no explanation whatsoever. The guard now lives inside
    // `cardMapState`, which returns `{ kind: "none" }` for the cards that really have nothing to
    // say. This test exists specifically because restoring that JSX guard breaks NOTHING else in
    // the route suite — the plan predicted that, and it was true when checked.
    renderView();
    enterEdit();
    fireEvent.click(screen.getByLabelText("add filter"));
    expect(screen.getAllByText(HINT_HEADING_CARD).length).toBe(2);
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
