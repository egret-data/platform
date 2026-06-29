import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// ── Mock factories ────────────────────────────────────────────────────────────
const notebooksMock = vi.fn();
const createMutateAsync = vi.fn();
const createMutation = vi.fn();
const updateMutateAsync = vi.fn();
const updateMutation = vi.fn();
const deleteMutate = vi.fn();
const deleteMutation = vi.fn();

vi.mock("@/api/notebooks", () => ({
  useNotebooks: () => notebooksMock(),
  useCreateNotebook: () => createMutation(),
  useUpdateNotebook: () => updateMutation(),
  useDeleteNotebook: () => deleteMutation(),
}));

import Notebooks from "@/routes/explore/Notebooks";

// ── Render helper ─────────────────────────────────────────────────────────────
function renderAt() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/notebooks"]}>
        <Routes>
          <Route path="/explore/:workspace/notebooks" element={<Notebooks />} />
          <Route path="/explore/:workspace/notebooks/:id" element={<div>NotebookPage</div>} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

// ── Fixtures ──────────────────────────────────────────────────────────────────
const mockNb1 = {
  id: "nb-1",
  name: "My Analysis",
  description: "Some analysis",
  engine: "daft" as const,
  is_public: false,
  owner_user_id: "user-1",
  is_owner: true,
  updated_at: "2026-06-01T10:00:00Z",
};

const mockNb2 = {
  id: "nb-2",
  name: "Shared Report",
  description: "",
  engine: "spark_connect" as const,
  is_public: true,
  owner_user_id: "user-2",
  is_owner: false,
  updated_at: "2026-06-02T12:00:00Z",
};

const createdNotebook = {
  id: "nb-new",
  name: "New Notebook",
  description: "",
  engine: "daft" as const,
  is_public: false,
  owner_user_id: "user-1",
  is_owner: true,
  updated_at: "2026-06-29T00:00:00Z",
  cells: [],
};

// ── Setup ─────────────────────────────────────────────────────────────────────
beforeEach(() => {
  notebooksMock.mockReset();
  createMutation.mockReset();
  updateMutation.mockReset();
  deleteMutation.mockReset();
  createMutateAsync.mockReset();
  updateMutateAsync.mockReset();
  deleteMutate.mockReset();

  notebooksMock.mockReturnValue({
    data: [mockNb1, mockNb2],
    isLoading: false,
    isError: false,
    error: null,
  });

  createMutation.mockReturnValue({
    mutateAsync: createMutateAsync,
    isPending: false,
  });
  updateMutation.mockReturnValue({
    mutateAsync: updateMutateAsync,
    isPending: false,
  });
  deleteMutation.mockReturnValue({ mutate: deleteMutate, isPending: false });

  createMutateAsync.mockResolvedValue(createdNotebook);
  updateMutateAsync.mockResolvedValue({ ...mockNb1, name: "Renamed" });
  deleteMutate.mockResolvedValue(undefined);
});

describe("Notebooks", () => {
  // ── List rendering ────────────────────────────────────────────────────────

  it("renders notebook names from mocked useNotebooks", () => {
    renderAt();
    expect(screen.getByText("My Analysis")).toBeInTheDocument();
    expect(screen.getByText("Shared Report")).toBeInTheDocument();
  });

  it("shows a Public badge on a public notebook but not on a private one", () => {
    renderAt();
    const publicBadges = screen.getAllByText("Public");
    expect(publicBadges.length).toBe(1);
    // The public badge should be near "Shared Report"
    expect(publicBadges[0]).toBeInTheDocument();
  });

  it("shows engine badges", () => {
    renderAt();
    expect(screen.getByText("daft")).toBeInTheDocument();
    expect(screen.getByText("spark_connect")).toBeInTheDocument();
  });

  it("shows empty state when list is []", () => {
    notebooksMock.mockReturnValue({
      data: [],
      isLoading: false,
      isError: false,
      error: null,
    });
    renderAt();
    expect(screen.getByText("No notebooks yet.")).toBeInTheDocument();
  });

  it("does not render cards while isLoading", () => {
    notebooksMock.mockReturnValue({
      data: undefined,
      isLoading: true,
      isError: false,
      error: null,
    });
    renderAt();
    expect(screen.queryByText("My Analysis")).not.toBeInTheDocument();
  });

  // ── New notebook dialog ───────────────────────────────────────────────────

  it("clicking New opens dialog; submitting calls useCreateNotebook mutateAsync with {name, engine}", async () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /new notebook/i }));

    await waitFor(() => screen.getByText(/Create notebook/i));

    const nameInput = screen.getByLabelText(/^name$/i);
    fireEvent.change(nameInput, { target: { value: "Test NB" } });

    // Engine defaults to daft; change to spark_connect via the native select
    const native = Array.from(
      document.querySelectorAll<HTMLSelectElement>("select"),
    ).find((s) =>
      Array.from(s.options).some(
        (o) => o.value === "daft" || o.value === "spark_connect",
      ),
    );
    if (native) {
      fireEvent.change(native, { target: { value: "spark_connect" } });
    }

    fireEvent.click(screen.getByRole("button", { name: /^create$/i }));

    await waitFor(() => {
      expect(createMutateAsync).toHaveBeenCalledWith(
        expect.objectContaining({ name: "Test NB" }),
      );
    });
  });

  it("navigates to new notebook after create", async () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /new notebook/i }));

    await waitFor(() => screen.getByText(/Create notebook/i));

    fireEvent.click(screen.getByRole("button", { name: /^create$/i }));

    await waitFor(() => {
      expect(screen.getByText("NotebookPage")).toBeInTheDocument();
    });
  });

  // ── Delete typed-confirm ─────────────────────────────────────────────────

  it("delete typed-confirm → calls useDeleteNotebook with the id", async () => {
    renderAt();

    // Find and click the delete button for the first notebook
    const deleteBtn = screen.getByRole("button", { name: /delete My Analysis/i });
    fireEvent.click(deleteBtn);

    await waitFor(() => {
      expect(screen.getByText(/Delete «My Analysis»/)).toBeInTheDocument();
    });

    const confirmInput = screen.getByPlaceholderText("delete");
    fireEvent.change(confirmInput, { target: { value: "delete" } });

    const confirmBtn = screen.getByRole("button", { name: /^delete$/i });
    fireEvent.click(confirmBtn);

    await waitFor(() => {
      expect(deleteMutate).toHaveBeenCalledWith("nb-1");
    });
  });

  // ── Rename dialog ─────────────────────────────────────────────────────────

  it("rename dialog → calls useUpdateNotebook mutateAsync with {name}", async () => {
    renderAt();

    const renameBtn = screen.getByRole("button", { name: /rename My Analysis/i });
    fireEvent.click(renameBtn);

    await waitFor(() => screen.getByText(/Rename notebook/i));

    const nameInput = screen.getByDisplayValue("My Analysis");
    fireEvent.change(nameInput, { target: { value: "Renamed" } });

    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));

    await waitFor(() => {
      expect(updateMutateAsync).toHaveBeenCalledWith(
        expect.objectContaining({ name: "Renamed" }),
      );
    });
  });
});
