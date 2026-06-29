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

vi.mock("@/api/notebooks", () => ({
  useNotebook: () => notebookMock(),
  useUpdateNotebook: () => updateNotebookMutation(),
  useAddCell: () => addCellMutation(),
  useUpdateCell: () => updateCellMutation(),
  useDeleteCell: () => deleteCellMutation(),
  useReorderCells: () => reorderCellsMutation(),
}));

import NotebookPage from "@/routes/explore/NotebookPage";

// ── Render helper ─────────────────────────────────────────────────────────────
function renderAt(id = "nb-1") {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[`/explore/eng/notebooks/${id}`]}>
        <Routes>
          <Route path="/explore/:workspace/notebooks/:id" element={<NotebookPage />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
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
  engine: "daft" as const,
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

  it("renders an engine select in the header", () => {
    renderAt();
    // Engine select — look for the native <select> or SelectTrigger text
    expect(screen.getByRole("combobox", { name: /engine/i })).toBeInTheDocument();
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

  // ── Run button disabled ───────────────────────────────────────────────────

  it("renders a Run button that is disabled with execution coming soon title", () => {
    renderAt();
    const runButtons = screen.getAllByRole("button", { name: /run/i });
    // All run buttons should be disabled
    runButtons.forEach((btn) => {
      expect(btn).toBeDisabled();
    });
    expect(runButtons[0]).toHaveAttribute("title", "Execution coming soon");
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
