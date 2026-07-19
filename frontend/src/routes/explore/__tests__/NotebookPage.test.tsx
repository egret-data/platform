import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// ── Monaco mock (jsdom cannot load Monaco) ────────────────────────────────────
vi.mock("@monaco-editor/react", () => ({
  default: ({ value, onChange }: { value: string; onChange: (v: string) => void }) => (
    <textarea
      data-testid="monaco"
      value={value}
      onChange={(e) => onChange(e.target.value)}
    />
  ),
}));

// ── Mock factories ────────────────────────────────────────────────────────────
const notebookMock = vi.fn();
const updateNotebookMutate = vi.fn();
const updateNotebookMutation = vi.fn();
const addCellMutate = vi.fn();
const addCellMutation = vi.fn();
const updateCellMutate = vi.fn();
const updateCellMutation = vi.fn();
const deleteCellMutate = vi.fn();
const deleteCellMutation = vi.fn();
const reorderCellsMutate = vi.fn();
const reorderCellsMutation = vi.fn();

const runtimeMock = vi.fn();
const activeExecutionsMock = vi.fn();

vi.mock("@/api/notebooks", () => ({
  useNotebook: () => notebookMock(),
  useUpdateNotebook: () => updateNotebookMutation(),
  useAddCell: () => addCellMutation(),
  useUpdateCell: () => updateCellMutation(),
  useDeleteCell: () => deleteCellMutation(),
  useReorderCells: () => reorderCellsMutation(),
  useNotebookRuntime: () => runtimeMock(),
  useActiveExecutions: () => activeExecutionsMock(),
}));

// ── Kernel mock (no real WebSocket in jsdom) ──────────────────────────────────
const kernelConnect = vi.fn();
const kernelExecute = vi.fn();
const kernelDisconnect = vi.fn();
const kernelInterrupt = vi.fn();
const kernelRestart = vi.fn();

vi.mock("@/lib/kernel/KernelConnection", () => ({
  KernelConnection: class {
    connect = kernelConnect;
    execute = kernelExecute;
    disconnect = kernelDisconnect;
    interrupt = kernelInterrupt;
    restart = kernelRestart;
    status = "idle" as const;
    onStatusChange = (cb: (s: string) => void) => {
      cb("idle");
      return () => {};
    };
  },
}));

import NotebookPage from "@/routes/explore/NotebookPage";

// ── Render helper ─────────────────────────────────────────────────────────────
// `inForm` wraps the tree in a <form> so Radix Select renders its hidden native
// <select>, which tests drive via fireEvent.change (portal is unreliable in jsdom).
function renderAt(id = "nb-1", { inForm = false }: { inForm?: boolean } = {}) {
  const qc = new QueryClient();
  const tree = (
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[`/explore/eng/notebooks/${id}`]}>
        <Routes>
          <Route path="/explore/:workspace/notebooks/:id" element={<NotebookPage />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );
  return render(inForm ? <form>{tree}</form> : tree);
}

// ── Fixtures ──────────────────────────────────────────────────────────────────
const mockCodeCell = {
  id: "cell-1",
  cell_type: "code" as const,
  source: "print(1)",
  cell_order: 0,
  execution_count: null,
  last_output: null,
  last_execution_time_ms: null,
};

const mockMarkdownCell = {
  id: "cell-2",
  cell_type: "markdown" as const,
  source: "# hi",
  cell_order: 1,
  execution_count: null,
  last_output: null,
  last_execution_time_ms: null,
};

const mockNotebook = {
  id: "nb-1",
  name: "Test Notebook",
  description: "A test notebook",
  is_public: false,
  owner_user_id: "user-1",
  is_owner: true,
  updated_at: "2026-06-29T10:00:00Z",
  cells: [mockCodeCell, mockMarkdownCell],
};

// ── Setup ─────────────────────────────────────────────────────────────────────
beforeEach(() => {
  vi.useFakeTimers();

  notebookMock.mockReset();
  activeExecutionsMock.mockReset();
  activeExecutionsMock.mockReturnValue({ data: { running: [] } });
  runtimeMock.mockReset();
  runtimeMock.mockReturnValue({
    data: {
      username: "alice",
      engines: [{ id: "e1", name: "trino", engine: "trino", enabled: true }],
      engine_jwt: null,
      engine_jwt_expires_at: null,
    },
  });
  updateNotebookMutation.mockReset();
  addCellMutation.mockReset();
  updateCellMutation.mockReset();
  deleteCellMutation.mockReset();
  reorderCellsMutation.mockReset();
  updateNotebookMutate.mockReset();
  addCellMutate.mockReset();
  updateCellMutate.mockReset();
  deleteCellMutate.mockReset();
  reorderCellsMutate.mockReset();

  notebookMock.mockReturnValue({
    data: mockNotebook,
    isLoading: false,
    isError: false,
    error: null,
  });

  updateNotebookMutation.mockReturnValue({
    mutate: updateNotebookMutate,
    mutateAsync: updateNotebookMutate,
    isPending: false,
  });
  addCellMutation.mockReturnValue({
    mutate: addCellMutate,
    isPending: false,
  });
  updateCellMutation.mockReturnValue({
    mutate: updateCellMutate,
    isPending: false,
  });
  deleteCellMutation.mockReturnValue({
    mutate: deleteCellMutate,
    isPending: false,
  });
  reorderCellsMutation.mockReturnValue({
    mutate: reorderCellsMutate,
    isPending: false,
  });

  kernelConnect.mockReset();
  kernelExecute.mockReset();
  kernelDisconnect.mockReset();
  kernelInterrupt.mockReset();
  kernelRestart.mockReset();
  kernelInterrupt.mockResolvedValue(undefined);
  kernelRestart.mockResolvedValue(undefined);
  kernelConnect.mockResolvedValue(undefined);
  kernelExecute.mockImplementation(
    (
      _cellId: string,
      _code: string,
      h: {
        onOutput: (o: unknown) => void;
        onStatus: (s: "busy" | "idle") => void;
        onExecutionCount: (n: number) => void;
      },
    ) => {
      h.onOutput({ type: "stream", name: "stdout", text: "ran\n" });
      h.onStatus("idle");
    },
  );

  updateNotebookMutate.mockResolvedValue(mockNotebook);
  addCellMutate.mockResolvedValue({ ...mockNotebook, cells: [...mockNotebook.cells] });
  updateCellMutate.mockResolvedValue(undefined);
  deleteCellMutate.mockResolvedValue(undefined);
  reorderCellsMutate.mockResolvedValue(undefined);
});

afterEach(() => {
  vi.useRealTimers();
});

describe("NotebookPage", () => {
  // ── IDE shell ─────────────────────────────────────────────────────────────

  it("renders the sidebar rail and a kernel status badge", () => {
    renderAt();
    expect(
      screen.getByRole("button", { name: /table of contents/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /data sources/i }),
    ).toBeInTheDocument();
    // KernelStatusBadge shows "Ready" for the mocked "idle" status.
    expect(screen.getByText(/ready/i)).toBeInTheDocument();
  });

  // ── Header ────────────────────────────────────────────────────────────────

  it("shows the notebook name in the header", () => {
    renderAt();
    expect(screen.getByDisplayValue("Test Notebook")).toBeInTheDocument();
  });

  it("renders a public toggle in the header", () => {
    renderAt();
    // Switch / checkbox labelled "Public"
    expect(screen.getByLabelText(/public/i)).toBeInTheDocument();
  });

  // ── Code cell ─────────────────────────────────────────────────────────────

  it("renders a monaco editor for the code cell", () => {
    renderAt();
    // The mocked Monaco renders a <textarea data-testid="monaco">
    const editors = screen.getAllByTestId("monaco");
    expect(editors.length).toBeGreaterThanOrEqual(1);
    // First editor should have the code cell source
    expect(editors[0]).toHaveValue("print(1)");
  });

  it("calls useUpdateCell with {cid, source} after debounce when editing a code cell", async () => {
    renderAt();

    const editors = screen.getAllByTestId("monaco");
    fireEvent.change(editors[0], { target: { value: "print(2)" } });

    // Before debounce fires, mutate should not be called
    expect(updateCellMutate).not.toHaveBeenCalled();

    // Advance fake timers past debounce (~600ms)
    await act(async () => {
      vi.advanceTimersByTime(700);
    });

    expect(updateCellMutate).toHaveBeenCalledWith(
      expect.objectContaining({ cid: "cell-1", source: "print(2)" }),
    );
  });

  // ── Add cell ──────────────────────────────────────────────────────────────

  it("clicking Add cell calls useAddCell", () => {
    renderAt();

    const addBtn = screen.getByRole("button", { name: /add cell/i });
    fireEvent.click(addBtn);

    expect(addCellMutate).toHaveBeenCalledWith(
      expect.objectContaining({ cell_type: "code" }),
    );
  });

  // ── Markdown cell ─────────────────────────────────────────────────────────

  it("renders a markdown cell with rendered HTML (h1 for # hi)", () => {
    renderAt();
    // react-markdown should render "# hi" as an <h1> (scoped by name to skip the
    // sr-only page-title heading that carries the notebook name).
    const heading = screen.getByRole("heading", { level: 1, name: "hi" });
    expect(heading).toHaveTextContent("hi");
  });

  // ── Delete cell ───────────────────────────────────────────────────────────

  it("clicking delete on a cell calls useDeleteCell with the cell id", () => {
    renderAt();

    const deleteButtons = screen.getAllByRole("button", { name: /delete cell/i });
    fireEvent.click(deleteButtons[0]);

    expect(deleteCellMutate).toHaveBeenCalledWith("cell-1");
  });

  // ── Run a cell ────────────────────────────────────────────────────────────

  it("renders an ENABLED Run button for a code cell (no 'coming soon' title)", () => {
    renderAt();
    const runButtons = screen.getAllByRole("button", { name: /^run$/i });
    // First run button belongs to the code cell (cell_order 0) → enabled.
    expect(runButtons[0]).toBeEnabled();
    expect(runButtons[0]).not.toHaveAttribute("title", "Execution coming soon");
  });

  it("clicking Run executes the cell and renders its output", async () => {
    renderAt();
    const runButton = screen.getAllByRole("button", { name: /^run$/i })[0];

    await act(async () => {
      fireEvent.click(runButton);
    });

    expect(kernelExecute).toHaveBeenCalledWith(
      "cell-1",
      expect.any(String),
      expect.objectContaining({ onOutput: expect.any(Function) }),
    );
    expect(screen.getByText(/ran/)).toBeInTheDocument();
  });

  it("restores persisted last_output on initial load (no click)", () => {
    notebookMock.mockReturnValue({
      data: {
        ...mockNotebook,
        cells: [
          {
            ...mockCodeCell,
            last_output: [{ type: "stream", name: "stdout", text: "restored" }],
          },
        ],
      },
      isLoading: false,
      isError: false,
      error: null,
    });
    renderAt();
    expect(screen.getByText(/restored/)).toBeInTheDocument();
  });

  it("Run on a code cell sends the raw cell source (no %%egret_sql prefix)", async () => {
    renderAt();
    const runButton = screen.getAllByRole("button", { name: /^run$/i })[0];

    await act(async () => {
      fireEvent.click(runButton);
    });

    // cell-1 is the code cell with source "print(1)"; it is passed through unmodified.
    expect(kernelExecute).toHaveBeenCalledWith(
      "cell-1",
      "print(1)",
      expect.any(Object),
    );
  });

  it("Run on a SQL cell sends the %%egret_sql magic-prefixed code", async () => {
    const sqlCell = {
      id: "cell-sql",
      cell_type: "sql" as const,
      source: "SELECT 1",
      cell_order: 0,
      execution_count: null,
      last_output: null,
      last_execution_time_ms: null,
      sql_target: "e1",
      sql_into: "df",
    };
    notebookMock.mockReturnValue({
      data: { ...mockNotebook, cells: [sqlCell] },
      isLoading: false,
      isError: false,
      error: null,
    });
    renderAt();
    const runButton = screen.getAllByRole("button", { name: /^run$/i })[0];

    await act(async () => {
      fireEvent.click(runButton);
    });

    const code = kernelExecute.mock.calls[0][1] as string;
    expect(code).toMatch(/^%%egret_sql/);
    expect(code).toContain("--into df");
    expect(code.endsWith("SELECT 1")).toBe(true);
  });

  it("renders a binding chip for a SQL cell with a variable and outputs", () => {
    const sqlCell = {
      id: "cell-sql",
      cell_type: "sql" as const,
      source: "SELECT 1",
      cell_order: 0,
      execution_count: null,
      // Seeds ManagedCell's outputs state on load → chip renders without a run.
      last_output: [{ type: "stream", name: "stdout", text: "ok" }],
      last_execution_time_ms: null,
      sql_target: "e1",
      sql_into: "df",
    };
    notebookMock.mockReturnValue({
      data: { ...mockNotebook, cells: [sqlCell] },
      isLoading: false,
      isError: false,
      error: null,
    });
    renderAt();
    // sql_into is wrapped in a <code>, so assert on the chip's full text content
    // (which spans the element boundary) rather than a single text node.
    const chip = screen.getByText(/Daft DataFrame/);
    expect(chip).toHaveTextContent(/df · Daft DataFrame/);
  });

  it("does not render a SQL target control", () => {
    renderAt();
    expect(screen.queryByLabelText("SQL target")).not.toBeInTheDocument();
  });

  // ── Loading state ─────────────────────────────────────────────────────────

  it("shows a loading skeleton while fetching", () => {
    notebookMock.mockReturnValue({
      data: undefined,
      isLoading: true,
      isError: false,
      error: null,
    });
    renderAt();
    expect(screen.queryByDisplayValue("Test Notebook")).not.toBeInTheDocument();
  });

  // ── Not found ─────────────────────────────────────────────────────────────

  it("shows not found when data is null after loading", () => {
    notebookMock.mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: { status: 404 },
    });
    renderAt();
    expect(screen.getByText(/not found/i)).toBeInTheDocument();
  });

  // ── Toolbar: Run all / Interrupt / Restart / Clear ──────────────────────────

  it("clicking Run all executes each NON-markdown cell once and skips markdown", async () => {
    // Fixture has a code cell (cell-1) and a markdown cell (cell-2).
    renderAt();

    const runAll = screen.getByRole("button", { name: /run all/i });
    await act(async () => {
      fireEvent.click(runAll);
    });

    // Only the code cell executes; the markdown cell is skipped.
    expect(kernelExecute).toHaveBeenCalledTimes(1);
    expect(kernelExecute).toHaveBeenCalledWith(
      "cell-1",
      expect.any(String),
      expect.any(Object),
    );
  });

  it("clicking Interrupt calls kernel.interrupt", () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /interrupt/i }));
    expect(kernelInterrupt).toHaveBeenCalledTimes(1);
  });

  it("clicking Restart calls kernel.restart", () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /restart/i }));
    expect(kernelRestart).toHaveBeenCalledTimes(1);
  });

  it("clicking Clear outputs empties a cell's rendered outputs", () => {
    notebookMock.mockReturnValue({
      data: {
        ...mockNotebook,
        cells: [
          {
            ...mockCodeCell,
            last_output: [{ type: "stream", name: "stdout", text: "keepme" }],
          },
        ],
      },
      isLoading: false,
      isError: false,
      error: null,
    });
    renderAt();
    expect(screen.getByText(/keepme/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /clear outputs/i }));

    expect(screen.queryByText(/keepme/)).not.toBeInTheDocument();
  });

  it("shows a running indicator for a cell listed in active-executions on load", () => {
    activeExecutionsMock.mockReturnValue({ data: { running: ["cell-1"] } });
    notebookMock.mockReturnValue({
      data: { ...mockNotebook, cells: [mockCodeCell] },
      isLoading: false,
      isError: false,
      error: null,
    });
    renderAt();
    // The code cell's Run button is disabled (running) without any click.
    const runButtons = screen.getAllByRole("button", { name: /^run$/i });
    expect(runButtons[0]).toBeDisabled();
  });

  // ── Per-cell run above / run below ────────────────────────────────────────────

  const cellA = {
    id: "cell-a",
    cell_type: "code" as const,
    source: "a",
    cell_order: 0,
    execution_count: null,
    last_output: null,
    last_execution_time_ms: null,
  };
  const cellB = {
    id: "cell-b",
    cell_type: "code" as const,
    source: "b",
    cell_order: 1,
    execution_count: null,
    last_output: null,
    last_execution_time_ms: null,
  };
  const cellC = {
    id: "cell-c",
    cell_type: "code" as const,
    source: "c",
    cell_order: 2,
    execution_count: null,
    last_output: null,
    last_execution_time_ms: null,
  };

  function renderThreeCells() {
    notebookMock.mockReturnValue({
      data: { ...mockNotebook, cells: [cellA, cellB, cellC] },
      isLoading: false,
      isError: false,
      error: null,
    });
    // DropdownMenu portal uses findByText (async) which needs real timers.
    vi.useRealTimers();
    const user = userEvent.setup();
    renderAt();
    return { user };
  }

  it("'Run all above' executes only cells before the target", async () => {
    const { user } = renderThreeCells();
    // Three code cells → three "Run options" triggers
    const triggers = screen.getAllByRole("button", { name: "Run options" });
    // Open middle cell's (index 1 = cell-b) dropdown
    await user.click(triggers[1]);
    // Click the menu item
    await user.click(await screen.findByText("Run all above"));
    // Only cell-a should have been executed (the one above cell-b)
    expect(kernelExecute).toHaveBeenCalledTimes(1);
    expect(kernelExecute).toHaveBeenCalledWith("cell-a", expect.any(String), expect.any(Object));
  });

  it("'Run all below' executes only cells after the target", async () => {
    const { user } = renderThreeCells();
    const triggers = screen.getAllByRole("button", { name: "Run options" });
    // Open middle cell's (index 1 = cell-b) dropdown
    await user.click(triggers[1]);
    // Click the menu item
    await user.click(await screen.findByText("Run all below"));
    // Only cell-c should have been executed (the one below cell-b)
    expect(kernelExecute).toHaveBeenCalledTimes(1);
    expect(kernelExecute).toHaveBeenCalledWith("cell-c", expect.any(String), expect.any(Object));
  });

  it("'Run all above' item is disabled for the first cell", async () => {
    const { user } = renderThreeCells();
    const triggers = screen.getAllByRole("button", { name: "Run options" });
    // Open first cell's (index 0 = cell-a) dropdown
    await user.click(triggers[0]);
    const item = await screen.findByText("Run all above");
    // Radix sets data-disabled on disabled DropdownMenuItems
    expect(item).toHaveAttribute("data-disabled");
  });

  it("'Run all below' item is disabled for the last cell", async () => {
    const { user } = renderThreeCells();
    const triggers = screen.getAllByRole("button", { name: "Run options" });
    // Open last cell's (index 2 = cell-c) dropdown
    await user.click(triggers[2]);
    const item = await screen.findByText("Run all below");
    // Radix sets data-disabled on disabled DropdownMenuItems
    expect(item).toHaveAttribute("data-disabled");
  });
});
