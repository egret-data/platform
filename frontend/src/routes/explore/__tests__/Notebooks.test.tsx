import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
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

  it("renders as a table with Name / Engine / Visibility / Updated headers", () => {
    renderAt();
    expect(screen.getByRole("table")).toBeInTheDocument();
    expect(screen.getByText("Engine")).toBeInTheDocument();
    expect(screen.getByText("Visibility")).toBeInTheDocument();
    expect(screen.getByText("Updated")).toBeInTheDocument();
  });

  it("shows a Public badge on a public notebook and Private on a private one", () => {
    renderAt();
    expect(screen.getAllByText("Public").length).toBe(1);
    expect(screen.getByText("Private")).toBeInTheDocument();
  });

  it("shows engine badges", () => {
    renderAt();
    expect(screen.getByText("daft")).toBeInTheDocument();
    expect(screen.getByText("spark_connect")).toBeInTheDocument();
  });

  it("filters rows by the search box", async () => {
    const user = userEvent.setup();
    renderAt();
    await user.type(screen.getByPlaceholderText(/search notebooks/i), "Shared");
    expect(screen.queryByText("My Analysis")).not.toBeInTheDocument();
    expect(screen.getByText("Shared Report")).toBeInTheDocument();
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

  it("does not render rows while isLoading", () => {
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

  // ── Row actions menu → Delete ─────────────────────────────────────────────

  it("delete via the row actions menu → typed-confirm → calls useDeleteNotebook with the id", async () => {
    const user = userEvent.setup();
    renderAt();

    // Open the per-row actions menu, then choose Delete.
    await user.click(screen.getByRole("button", { name: /actions for My Analysis/i }));
    await user.click(await screen.findByRole("menuitem", { name: /^delete$/i }));

    await waitFor(() => {
      expect(screen.getByText(/Delete «My Analysis»/)).toBeInTheDocument();
    });

    const confirmInput = screen.getByPlaceholderText("delete");
    fireEvent.change(confirmInput, { target: { value: "delete" } });

    fireEvent.click(screen.getByRole("button", { name: /^delete$/i }));

    await waitFor(() => {
      expect(deleteMutate).toHaveBeenCalledWith("nb-1");
    });
  });

  // ── Row actions menu → Rename ─────────────────────────────────────────────

  it("rename via the row actions menu → calls useUpdateNotebook mutateAsync with {name}", async () => {
    const user = userEvent.setup();
    renderAt();

    await user.click(screen.getByRole("button", { name: /actions for My Analysis/i }));
    await user.click(await screen.findByRole("menuitem", { name: /^rename$/i }));

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
