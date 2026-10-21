import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import DashboardView from "../DashboardView";

// KI-059: the edit-mode card wrapper is a `role="button"` that opens the card's settings on Enter
// and Space. The heading input and the text textarea live INSIDE it, so their keydown bubbled up
// and the wrapper's `preventDefault` swallowed every space and newline the author typed.
//
// The real `DashboardCardView` is rendered on purpose: the bug is the interaction between the
// wrapper in DashboardView and the editable element in HeadingCard/TextCard, so mocking either
// side away would test nothing.

vi.mock("sonner", () => ({ toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn() }) }));
vi.mock("@/api/charts", async (orig) => ({
  ...(await orig<typeof import("@/api/charts")>()),
  useCharts: () => ({ data: [] }),
}));

const detail = {
  id: "d1", name: "Sales", description: "", created_by: null, updated_at: "",
  parameters: [], tabs: [], width: "full",
  cards: [
    { id: "c1", chart_id: null, card_type: "heading", grid_col: 0, grid_row: 0, size_x: 6, size_y: 1, options: { text: "" }, tab_id: null },
    { id: "c2", chart_id: null, card_type: "text", grid_col: 6, grid_row: 0, size_x: 6, size_y: 3, options: { text: "" }, tab_id: null },
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
/** The card settings panel is open: its "show background" switch exists only there. */
const cardPanelOpen = () => screen.queryByLabelText("show background") !== null;

beforeEach(() => vi.clearAllMocks());

describe("DashboardView edit-mode card keys (KI-059)", () => {
  it("typing into a heading card keeps every space", async () => {
    const user = userEvent.setup();
    renderView();
    enterEdit();
    const input = screen.getByLabelText("heading text") as HTMLInputElement;
    await user.type(input, "Weekly sales review");
    expect(input.value).toBe("Weekly sales review");
  });

  it("typing into a text card keeps spaces and Enter", async () => {
    const user = userEvent.setup();
    renderView();
    enterEdit();
    const area = screen.getByLabelText("text body") as HTMLTextAreaElement;
    await user.type(area, "Weekly sales{Enter}second line");
    expect(area.value).toBe("Weekly sales\nsecond line");
  });

  it("a key typed in the editable field does not open the card settings", async () => {
    const user = userEvent.setup();
    renderView();
    enterEdit();
    // Focus without clicking: the click on the field is the wrapper's own click handler and
    // opens the panel by design; this asserts only that the KEYS no longer do.
    (screen.getByLabelText("heading text") as HTMLInputElement).focus();
    await user.keyboard(" {Enter}");
    expect(cardPanelOpen()).toBe(false);
  });

  it.each([["Enter", "{Enter}"], ["Space", " "]])("%s on the focused card itself still opens its settings (FE-A11Y-3)", async (_name, key) => {
    const user = userEvent.setup();
    renderView();
    enterEdit();
    const card = screen.getAllByTestId("dashcard-body")[0];
    card.focus();
    expect(cardPanelOpen()).toBe(false);
    await user.keyboard(key);
    expect(cardPanelOpen()).toBe(true);
  });
});
