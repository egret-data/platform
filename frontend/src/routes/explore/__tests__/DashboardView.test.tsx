import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import DashboardView from "../DashboardView";

// `dashState.current` is what useDashboard returns; tests can flip it to a loading
// state to exercise the loading→loaded transition (guards the Rules of Hooks).
const { updateSpy, dashState } = vi.hoisted(() => {
  const loaded = {
    data: {
      id: "d1", name: "Ops", description: "", created_by: null, updated_at: "",
      parameters: [],
      cards: [{ id: "k1", chart_id: "c1", card_type: "chart", grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: {} }],
    },
    isLoading: false,
  };
  return {
    updateSpy: vi.fn((_b: { id: string; name: string; description?: string; cards: Array<{ size_x: number; size_y: number }> }) => Promise.resolve({})),
    dashState: { current: loaded as { data: unknown; isLoading: boolean }, loaded: loaded as { data: unknown; isLoading: boolean } },
  };
});

vi.mock("@/api/dashboards", () => ({
  useDashboard: () => dashState.current,
  useDashboardMutations: () => ({ update: { mutateAsync: updateSpy, isPending: false }, revert: { mutateAsync: vi.fn(), isPending: false } }),
  // The info sidebar's History tab query; disabled until the tab is opened, so an empty result is
  // the right stub for every test that never opens it.
  useDashboardRevisions: () => ({ data: [], isLoading: false }),
  // The view-mode "…" menu reads the dashboard's folder from the LIST query. (W8 added folder_id
  // to the detail DTO for the info sidebar's "Saved in" row, but the menu still reads the list.)
  // An empty list leaves it folderless.
  useDashboards: () => ({ data: [] }),
}));

beforeEach(() => { dashState.current = dashState.loaded; });
vi.mock("@/api/charts", async (orig) => ({
  ...(await orig<typeof import("@/api/charts")>()),
  useCharts: () => ({ data: [{ id: "c1", dataset_id: "ds1", name: "Sales", viz_type: "table", description: "", params: { spec: { metrics: [], dimensions: ["region"], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] } }, created_by: null, updated_at: "" }] }),
  useChart: () => ({ data: undefined }),
}));
vi.mock("@/api/useChartData", () => ({
  useChartData: () => ({ columns: [{ name: "rev", type: "double" }], rows: [[5]], isLoading: false, error: null }),
}));
vi.mock("@/api/useSavedChartData", () => ({
  useDashboardCardData: () => ({ columns: [{ name: "rev", type: "double" }], rows: [[5]], isLoading: false, error: null }),
}));
vi.mock("@/api/useColumnValues", () => ({
  // fresh array each call — would loop without the idempotent guard
  useColumnValues: () => ({ values: ["new", "paid"].slice(), isLoading: false, error: null }),
}));

function renderAt() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/dashboards/d1"]}>
        <Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("DashboardView (view mode)", () => {
  it("renders the dashboard name and a card with its chart title", () => {
    renderAt();
    expect(screen.getByText("Ops")).toBeInTheDocument();
    expect(screen.getByText("Sales")).toBeInTheDocument();
  });
  it("shows an Edit button (not editing by default)", () => {
    renderAt();
    expect(screen.getByRole("button", { name: /edit/i })).toBeInTheDocument();
  });
});

describe("DashboardView (edit mode)", () => {
  it("adds a card from the saved-charts sidebar and removes it", () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    // Sidebar is hidden on enter-edit; open the Add panel via the toolbar button
    fireEvent.click(screen.getByRole("button", { name: "add chart" }));
    const addBtn = screen.getByRole("button", { name: "Sales" });
    fireEvent.click(addBtn);
    expect(screen.getAllByRole("button", { name: /remove card/i }).length).toBeGreaterThan(0);
  });

  // Regression: a newly added card must keep its default size in the save payload.
  // (A bug let RGL auto-place a card missing from the controlled layout at 1x1.)
  it("saves an added card at its default size, not collapsed", async () => {
    updateSpy.mockClear();
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    // Open the Add panel via the toolbar button, then pick the chart
    fireEvent.click(screen.getByRole("button", { name: "add chart" }));
    fireEvent.click(screen.getByRole("button", { name: "Sales" }));
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));
    expect(updateSpy).toHaveBeenCalledTimes(1);
    const payload = updateSpy.mock.calls[0][0];
    expect(payload.cards).toHaveLength(2);           // existing + added
    // every card (including the freshly added one) is a real size, never 1x1
    for (const c of payload.cards) {
      expect(c.size_x).toBeGreaterThanOrEqual(6);
      expect(c.size_y).toBeGreaterThanOrEqual(4);
    }
  });

  it("saves the edited dashboard name", () => {
    updateSpy.mockClear();
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    const nameInput = screen.getByLabelText("dashboard name");
    fireEvent.change(nameInput, { target: { value: "Renamed Dash" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(updateSpy).toHaveBeenCalledWith(expect.objectContaining({ name: "Renamed Dash" }));
  });
});

describe("DashboardView (Phase B — card variety)", () => {
  it("adds a text card and edits its body inline", async () => {
    const user = userEvent.setup();
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    await user.click(screen.getByRole("button", { name: "add text or heading" }));
    await user.click(await screen.findByRole("menuitem", { name: "Text" }));
    const editor = screen.getByLabelText("text body") as HTMLTextAreaElement;
    fireEvent.change(editor, { target: { value: "## Notes" } });
    expect(editor.value).toBe("## Notes");
  });
  it("adds a heading card and edits it inline", async () => {
    const user = userEvent.setup();
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    await user.click(screen.getByRole("button", { name: "add text or heading" }));
    await user.click(await screen.findByRole("menuitem", { name: "Heading" }));
    const editor = screen.getByLabelText("heading text") as HTMLInputElement;
    fireEvent.change(editor, { target: { value: "Sales" } });
    expect(editor.value).toBe("Sales");
  });
  // The bare "add link" button became a "add a link or iframe" menu (Metabase pairs the two
  // behind one trigger), so this now opens the menu first — the same shape as the text/heading
  // cases above, and for the same Radix reason: the trigger opens on pointerdown, which
  // fireEvent.click does not dispatch.
  it("adds a link card with url + label settings", async () => {
    const user = userEvent.setup();
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    const before = screen.getAllByRole("button", { name: /remove card/i }).length;
    await user.click(screen.getByRole("button", { name: "add a link or iframe" }));
    await user.click(await screen.findByRole("menuitem", { name: "Link" }));
    const after = screen.getAllByRole("button", { name: /remove card/i }).length;
    expect(after).toBe(before + 1);
    const bodies = screen.getAllByTestId("dashcard-body");
    fireEvent.click(bodies[bodies.length - 1]);
    expect(screen.getByLabelText(/link url/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/link label/i)).toBeInTheDocument();
  });
  it("adds an iframe card, whose settings panel opens by itself", async () => {
    // Unlike text and heading, an iframe card has no inline editor — the URL is entered in the
    // settings panel — so `addVirtualCard` opens the panel on add. No dashcard-body click here:
    // needing one would mean the auto-open never fired.
    const user = userEvent.setup();
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    const before = screen.getAllByRole("button", { name: /remove card/i }).length;
    await user.click(screen.getByRole("button", { name: "add a link or iframe" }));
    await user.click(await screen.findByRole("menuitem", { name: "Iframe" }));
    expect(screen.getAllByRole("button", { name: /remove card/i }).length).toBe(before + 1);
    expect(screen.getByLabelText("iframe url or embed code")).toHaveValue("");
  });
  it("offers a title input on a chart card in edit mode", () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    fireEvent.click(screen.getAllByTestId("dashcard-body")[0]);
    expect(screen.getByLabelText(/card title/i)).toBeInTheDocument();
  });
});

describe("DashboardView (Phase B — duplicate/replace)", () => {
  it("duplicates a card", () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    const before = screen.getAllByRole("button", { name: /remove card/i }).length;
    fireEvent.click(screen.getAllByRole("button", { name: /duplicate card/i })[0]);
    const after = screen.getAllByRole("button", { name: /remove card/i }).length;
    expect(after).toBe(before + 1);
  });
  it("replaces a chart card's chart via the picker", () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    fireEvent.click(screen.getAllByRole("button", { name: /replace card/i })[0]);
    fireEvent.click(screen.getByRole("button", { name: /^sales$/i }));
    expect(screen.getAllByRole("button", { name: /remove card/i }).length).toBeGreaterThan(0);
  });
});

describe("DashboardView (Phase B — view controls)", () => {
  it("shows an auto-refresh control and a fullscreen toggle in view mode", () => {
    renderAt();
    // W4 replaced the native <select aria-label="auto-refresh"> with a popover trigger.
    expect(screen.getByRole("button", { name: "Auto Refresh" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /fullscreen/i })).toBeInTheDocument();
  });
});

describe("DashboardView (Phase C — parameters)", () => {
  it("adds a filter parameter in edit mode", () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    fireEvent.click(screen.getByRole("button", { name: "add filter" }));
    expect(screen.getByLabelText(/parameter name/i)).toBeInTheDocument();
  });
  it("renders the filter bar in view mode when the dashboard has parameters", () => {
    dashState.current = { data: { ...(dashState.loaded.data as object), parameters: [{ id: "p1", name: "Region", slug: "region", type: "text", operator: "is" }] }, isLoading: false };
    renderAt();
    expect(screen.getByLabelText("Region")).toBeInTheDocument();
  });
});

describe("DashboardView (Phase C — mapping + injection)", () => {
  it("shows an in-card column-to-filter overlay when a filter is selected in edit mode", async () => {
    const user = userEvent.setup();
    dashState.current = { data: { ...(dashState.loaded.data as object), parameters: [{ id: "p1", name: "Region", slug: "region", type: "text", operator: "is" }] }, isLoading: false };
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    expect(screen.queryByLabelText("column to filter on")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Region" }));
    expect(screen.getAllByLabelText("column to filter on").length).toBeGreaterThan(0);
    // Picking a real column must persist through updateCardOptions and read back as
    // the (controlled) select's value on re-render — the load-bearing round-trip.
    await user.click(screen.getAllByRole("combobox", { name: "column to filter on" })[0]);
    await user.click(await screen.findByRole("option", { name: "region" }));
    expect(screen.getAllByRole("combobox", { name: "column to filter on" })[0]).toHaveTextContent("region");
  });
});

describe("DashboardView (Phase C — category options no render loop)", () => {
  it("renders with a category param mapped to a card without exceeding update depth", () => {
    dashState.current = {
      data: {
        ...(dashState.loaded.data as object),
        parameters: [{ id: "p1", name: "Status", slug: "status", type: "category", operator: "is" }],
        cards: [{ id: "k1", chart_id: "c1", card_type: "chart", grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: { paramMappings: { p1: "region" } } }],
      },
      isLoading: false,
    };
    expect(() => renderAt()).not.toThrow();
    expect(screen.getByLabelText("Status")).toBeInTheDocument();
  });
});

describe("DashboardView (Rules of Hooks)", () => {
  // Regression: every hook must run before the early loading return, so the
  // loading→loaded transition doesn't change the hook count ("Rendered more hooks").
  it("survives the loading -> loaded transition without a hooks-order error", () => {
    const qc = new QueryClient();
    const tree = () => (
      <QueryClientProvider client={qc}>
        <MemoryRouter initialEntries={["/explore/eng/dashboards/d1"]}>
          <Routes><Route path="/explore/:workspace/dashboards/:id" element={<DashboardView />} /></Routes>
        </MemoryRouter>
      </QueryClientProvider>
    );
    dashState.current = { data: undefined, isLoading: true };
    const { rerender } = render(tree());
    expect(screen.getByText("Loading…")).toBeInTheDocument();
    dashState.current = dashState.loaded;
    expect(() => rerender(tree())).not.toThrow();
    expect(screen.getByText("Ops")).toBeInTheDocument();
  });
});

describe("DashboardView (Phase D1 — on-click config)", () => {
  it("sets a card's click behaviour to a custom URL in edit mode", async () => {
    const user = userEvent.setup();
    dashState.current = {
      data: { ...(dashState.loaded.data as object),
        cards: [{ id: "k1", chart_id: "c1", card_type: "chart", grid_col: 0, grid_row: 0, size_x: 6, size_y: 4, options: {} }] },
      isLoading: false,
    };
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    fireEvent.click(screen.getAllByTestId("dashcard-body")[0]);
    await user.click(screen.getByRole("combobox", { name: /on click/i }));
    await user.click(await screen.findByRole("option", { name: "Custom URL" }));
    const tpl = screen.getByLabelText(/url template/i);
    fireEvent.change(tpl, { target: { value: "https://x.com/{{region}}" } });
    expect((tpl as HTMLInputElement).value).toBe("https://x.com/{{region}}");
  });
});
