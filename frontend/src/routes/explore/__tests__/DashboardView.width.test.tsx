import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import DashboardView from "../DashboardView";

vi.mock("@/components/chart/dashboard/DashboardCardView", () => ({ DashboardCardView: () => <div /> }));
vi.mock("@/api/charts", async (orig) => ({
  ...(await orig<typeof import("@/api/charts")>()),
  useCharts: () => ({ data: [] }),
}));

const update = { mutateAsync: vi.fn().mockResolvedValue({}), isPending: false };
const base = {
  id: "d1", name: "D", description: "", created_by: null, updated_at: "", parameters: [], tabs: [],
  cards: [{ id: "c1", chart_id: null, card_type: "heading", grid_col: 0, grid_row: 0, size_x: 6, size_y: 1, options: { text: "h" }, tab_id: null }],
  width: "full",
};
let detail: Record<string, unknown> = { ...base };
vi.mock("@/api/dashboards", async (orig) => {
  const mod = await (orig as () => Promise<Record<string, unknown>>)();
  return { ...mod, useDashboard: () => ({ data: detail, isLoading: false }), useDashboardMutations: () => ({ update, revert: { mutateAsync: vi.fn(), isPending: false }, setCertification: { mutateAsync: vi.fn(), isPending: false } }) };
});

const renderView = () => render(
  <QueryClientProvider client={new QueryClient()}>
    <MemoryRouter initialEntries={["/explore/eng/dashboards/d1"]}>
      <Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes>
    </MemoryRouter>
  </QueryClientProvider>,
);

const FIXED = ".max-w-\\[1048px\\]";

/** Open edit mode and flip the Full width switch. */
async function toggleFullWidth(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole("button", { name: "Edit" }));
  await user.click(screen.getByRole("button", { name: "dashboard actions" }));
  await user.click(await screen.findByRole("switch", { name: "Full width" }));
}

describe("DashboardView width", () => {
  it("view mode 'fixed' wraps content in a centered 1048px container", () => {
    detail = { ...base, width: "fixed" };
    const { container } = renderView();
    expect(container.querySelector(FIXED)).toBeTruthy();
  });

  it("view mode 'full' has no fixed wrapper", () => {
    detail = { ...base, width: "full" };
    const { container } = renderView();
    expect(container.querySelector(FIXED)).toBeNull();
  });

  it("edit mode: entering edit snapshots the STORED width, and the canvas then follows the DRAFT", async () => {
    // Two things at once, because removing either one is a live defect and the plan's mutation
    // list assumed the "cancel" test below covered the first. It does not — cancel never
    // re-enters edit mode, so a stale draft is unobservable from there. It is observable HERE.
    //
    //  - The snapshot: entering edit mode must not change what is on screen. Without
    //    `setDraftWidth(dashboard.width)` in startEdit(), the canvas jumps to the draft state's
    //    initial value the instant Edit is pressed and the switch misreports the stored width.
    //  - The draft: without one the canvas would keep rendering the stored width and the switch
    //    would look inert until Save — which is how the toggle reads as broken even when the
    //    payload is right.
    detail = { ...base, width: "fixed" };
    const user = userEvent.setup();
    const { container } = renderView();
    expect(container.querySelector(FIXED)).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Edit" }));
    expect(container.querySelector(FIXED)).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "dashboard actions" }));
    const sw = await screen.findByRole("switch", { name: "Full width" });
    expect(sw).toHaveAttribute("aria-checked", "false");

    await user.click(sw);
    expect(container.querySelector(FIXED)).toBeNull();
  });

  it("edit mode: saving sends the toggled width, not the stored one", async () => {
    detail = { ...base, width: "fixed" };
    update.mutateAsync.mockClear();
    const user = userEvent.setup();
    renderView();
    await toggleFullWidth(user);
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(update.mutateAsync).toHaveBeenCalledWith(expect.objectContaining({ width: "full" }));
  });

  it("cancelling an edit discards the width change", async () => {
    detail = { ...base, width: "fixed" };
    const user = userEvent.setup();
    const { container } = renderView();
    await toggleFullWidth(user);
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    // W2 put a discard confirmation in front of Cancel, and flipping this switch is a real unsaved
    // change — so the width edit now has to be discarded THROUGH the dialog. Confirming it is the
    // fix; weakening `isDraftDirty` so that a width flip reads as clean would put the silent
    // data-loss hole back, from the other side.
    await user.click(screen.getByRole("button", { name: "Discard changes" }));
    expect(container.querySelector(FIXED)).toBeTruthy();
  });

  it("the description-only save keeps sending the STORED width", async () => {
    // saveDescription runs in VIEW mode, where there is no draft. If it ever starts reading the
    // draft width it will write back a width the user cancelled.
    detail = { ...base, width: "fixed" };
    update.mutateAsync.mockClear();
    const user = userEvent.setup();
    renderView();
    await user.click(screen.getByRole("button", { name: "dashboard info" }));
    const box = await screen.findByLabelText("description");
    fireEvent.change(box, { target: { value: "notes" } });
    fireEvent.blur(box);
    expect(update.mutateAsync).toHaveBeenCalledWith(
      expect.objectContaining({ description: "notes", width: "fixed" }),
    );
  });

  // KI-068: opening the edit panel on a fixed dashboard must not take its width from the canvas.
  it("edit mode: opening a side panel widens the fixed wrapper by the panel", () => {
    detail = { ...base, width: "fixed" };
    const { container } = renderView();
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    expect(container.querySelector(FIXED)).toBeTruthy();
    fireEvent.click(screen.getAllByTestId("dashcard-body")[0]);
    expect(container.querySelector(FIXED)).toBeNull();
    expect(container.querySelector(".max-w-\\[1404px\\]")).toBeTruthy();
  });
});
