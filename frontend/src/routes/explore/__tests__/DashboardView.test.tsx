import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import DashboardView from "../DashboardView";

// `dashState.current` is what useDashboard returns; tests can flip it to a loading
// state to exercise the loading→loaded transition (guards the Rules of Hooks).
const { updateSpy, dashState } = vi.hoisted(() => {
  const loaded = {
    data: {
      id: "d1", name: "Ops", description: "", created_by: null, updated_at: "",
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
  useDashboardMutations: () => ({ update: { mutateAsync: updateSpy, isPending: false } }),
}));

beforeEach(() => { dashState.current = dashState.loaded; });
vi.mock("@/api/charts", async (orig) => ({
  ...(await orig<typeof import("@/api/charts")>()),
  useCharts: () => ({ data: [{ id: "c1", dataset_id: "ds1", name: "Sales", viz_type: "table", description: "", params: { spec: { metrics: [], dimensions: [], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] } }, created_by: null, updated_at: "" }] }),
  useChart: () => ({ data: undefined }),
}));
vi.mock("@/api/useChartData", () => ({
  useChartData: () => ({ columns: [{ name: "rev", type: "double" }], rows: [[5]], isLoading: false, error: null }),
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
    const addBtn = screen.getByRole("button", { name: /add sales/i });
    fireEvent.click(addBtn);
    expect(screen.getAllByRole("button", { name: /remove card/i }).length).toBeGreaterThan(0);
  });

  // Regression: a newly added card must keep its default size in the save payload.
  // (A bug let RGL auto-place a card missing from the controlled layout at 1x1.)
  it("saves an added card at its default size, not collapsed", async () => {
    updateSpy.mockClear();
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    fireEvent.click(screen.getByRole("button", { name: /add sales/i }));
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
});

describe("DashboardView (Phase B — card variety)", () => {
  it("adds a text card and edits its body", () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    fireEvent.click(screen.getByRole("button", { name: /add text/i }));
    const editor = screen.getByLabelText(/text body/i);
    fireEvent.change(editor, { target: { value: "## Notes" } });
    expect((editor as HTMLTextAreaElement).value).toBe("## Notes");
  });
  it("adds a heading card", () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    fireEvent.click(screen.getByRole("button", { name: /add heading/i }));
    expect(screen.getByLabelText(/heading text/i)).toBeInTheDocument();
  });
  it("adds a link card with url + label", () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    fireEvent.click(screen.getByRole("button", { name: /add link/i }));
    expect(screen.getByLabelText(/link url/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/link label/i)).toBeInTheDocument();
  });
  it("offers a title input on a chart card in edit mode", () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
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
    fireEvent.click(screen.getByRole("button", { name: /^use sales$/i }));
    expect(screen.getAllByRole("button", { name: /remove card/i }).length).toBeGreaterThan(0);
  });
});

describe("DashboardView (Phase B — view controls)", () => {
  it("shows an auto-refresh control and a fullscreen toggle in view mode", () => {
    renderAt();
    expect(screen.getByLabelText(/auto-refresh/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /fullscreen/i })).toBeInTheDocument();
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
