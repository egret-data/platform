import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";
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

vi.mock("@/api/notebooks", () => ({
  useNotebook: () => notebookMock(),
  useUpdateNotebook: () => updateNotebookMutation(),
  useAddCell: () => addCellMutation(),
  useUpdateCell: () => updateCellMutation(),
  useDeleteCell: () => deleteCellMutation(),
  useReorderCells: () => reorderCellsMutation(),
  useNotebookRuntime: () => runtimeMock(),
}));

// ── Kernel mock (no real WebSocket in jsdom) ──────────────────────────────────
const kernelConnect = vi.fn();
const kernelExecute = vi.fn();
const kernelDisconnect = vi.fn();

vi.mock("@/lib/kernel/KernelConnection", () => ({
  KernelConnection: class {
    connect = kernelConnect;
    execute = kernelExecute;
    disconnect = kernelDisconnect;
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

// Drive the underlying Radix Select via its hidden native <select>.
function selectOption(value: string) {
  const native = Array.from(
    document.querySelectorAll<HTMLSelectElement>("select"),
  ).find((s) => Array.from(s.options).some((o) => o.value === value));
  if (!native) throw new Error(`option ${value} not found`);
  fireEvent.change(native, { target: { value } });
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
  sql_target: null,
};

const mockMarkdownCell = {
  id: "cell-2",
  cell_type: "markdown" as const,
  source: "# hi",
  cell_order: 1,
  execution_count: null,
  last_output: null,
  last_execution_time_ms: null,
  sql_target: null,
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
    // react-markdown should render "# hi" as an <h1>
    const heading = screen.getByRole("heading", { level: 1 });
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
    const runButtons = screen.getAllByRole("button", { name: /run/i });
    // First run button belongs to the code cell (cell_order 0) → enabled.
    expect(runButtons[0]).toBeEnabled();
    expect(runButtons[0]).not.toHaveAttribute("title", "Execution coming soon");
  });

  it("clicking Run executes the cell and renders its output", async () => {
    renderAt();
    const runButton = screen.getAllByRole("button", { name: /run/i })[0];

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

  it("a SQL cell's Run passes the %%egret_sql magic prefix to execute", async () => {
    notebookMock.mockReturnValue({
      data: {
        ...mockNotebook,
        cells: [
          {
            ...mockCodeCell,
            cell_type: "sql" as const,
            source: "select 1",
            sql_target: null,
          },
        ],
      },
      isLoading: false,
      isError: false,
      error: null,
    });
    renderAt();
    const runButton = screen.getAllByRole("button", { name: /run/i })[0];

    await act(async () => {
      fireEvent.click(runButton);
    });

    expect(kernelExecute).toHaveBeenCalledWith(
      "cell-1",
      expect.stringMatching(/^%%egret_sql --target daft\n/),
      expect.any(Object),
    );
  });

  // ── SQL target dropdown ─────────────────────────────────────────────────────

  function mockSqlNotebook() {
    notebookMock.mockReturnValue({
      data: {
        ...mockNotebook,
        cells: [
          {
            ...mockCodeCell,
            cell_type: "sql" as const,
            source: "select 1",
            sql_target: null,
          },
        ],
      },
      isLoading: false,
      isError: false,
      error: null,
    });
  }

  it("renders a SQL-target control defaulting to Daft for a null sql_target", () => {
    mockSqlNotebook();
    renderAt();
    const target = screen.getByLabelText("SQL target");
    expect(target).toBeInTheDocument();
    expect(target).toHaveTextContent(/Daft/);
  });

  it("selecting an engine calls useUpdateCell with {cid, sql_target}", () => {
    mockSqlNotebook();
    renderAt("nb-1", { inForm: true });

    selectOption("trino");

    expect(updateCellMutate).toHaveBeenCalledWith({
      cid: "cell-1",
      sql_target: "trino",
    });
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
});
