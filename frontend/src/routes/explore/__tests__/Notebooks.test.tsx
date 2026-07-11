import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";

// ── Mock factories ────────────────────────────────────────────────────────────

const notebooksMock = vi.fn();
const trashMutateAsync = vi.fn();
const trashMutation = vi.fn();
const restoreMutateAsync = vi.fn();
const restoreMutation = vi.fn();
const addBookmarkMutateAsync = vi.fn();
const addBookmarkMutation = vi.fn();
const removeBookmarkMutateAsync = vi.fn();
const removeBookmarkMutation = vi.fn();
const createMutateAsync = vi.fn();
const createMutation = vi.fn();
const updateMutateAsync = vi.fn();
const updateMutation = vi.fn();
const deleteMutateAsync = vi.fn();
const deleteMutation = vi.fn();

vi.mock("@/api/notebooks", () => ({
  useNotebooks: (...args: unknown[]) => notebooksMock(...args),
  useCreateNotebook: () => createMutation(),
  useUpdateNotebook: () => updateMutation(),
  useDeleteNotebook: () => deleteMutation(),
  useTrashNotebook: () => trashMutation(),
  useRestoreNotebook: () => restoreMutation(),
  useAddNotebookBookmark: () => addBookmarkMutation(),
  useRemoveNotebookBookmark: () => removeBookmarkMutation(),
}));

vi.mock("@/api/folders", () => ({
  useFolders: () => ({ data: [] }),
  useSetFolder: () => ({ mutateAsync: vi.fn().mockResolvedValue({}) }),
  useFolderMutations: () => ({
    create: { mutateAsync: vi.fn().mockResolvedValue({ id: "f1", name: "F1" }) },
    rename: { mutateAsync: vi.fn().mockResolvedValue({}) },
    remove: { mutateAsync: vi.fn().mockResolvedValue(undefined) },
  }),
}));

// ShareDialog talks to the generated shares hooks + the principal picker; stub
// them so opening the dialog from the notebook row renders without a network.
vi.mock("@/api/generated/shares/shares", () => ({
  getSharesGrantListQueryKey: () => ["grants"],
  useSharesGrantList: () => ({ data: { data: [] }, isLoading: false, isError: false, error: null }),
  useSharesGrantPut: () => ({ mutate: vi.fn(), isPending: false }),
  useSharesGrantDelete: () => ({ mutate: vi.fn(), isPending: false }),
}));

vi.mock("@/api/policies", () => ({
  listPrincipals: vi.fn(() => Promise.resolve([])),
}));

import Notebooks from "@/routes/explore/Notebooks";

// ── Render helper ─────────────────────────────────────────────────────────────

function renderAt() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MemoryRouter initialEntries={["/explore/eng/notebooks"]}>
          <Routes>
            <Route path="/explore/:workspace/notebooks" element={<Notebooks />} />
            <Route path="/explore/:workspace/notebooks/:id" element={<div>NotebookPage</div>} />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

// ── Fixtures ──────────────────────────────────────────────────────────────────

const mockNb1 = {
  id: "nb-1",
  name: "My Analysis",
  description: "Some analysis",
  is_public: false,
  owner_user_id: "user-1",
  is_owner: true,
  updated_at: "2026-06-01T10:00:00Z",
  bookmarked: false,
  folder_id: null,
  deleted_at: null,
};

const mockNb2 = {
  id: "nb-2",
  name: "Shared Report",
  description: "",
  is_public: true,
  owner_user_id: "user-2",
  is_owner: false,
  updated_at: "2026-06-02T12:00:00Z",
  bookmarked: true,
  folder_id: null,
  deleted_at: null,
};

const createdNotebook = {
  id: "nb-new",
  name: "New Notebook",
  description: "",
  is_public: false,
  owner_user_id: "user-1",
  is_owner: true,
  updated_at: "2026-06-29T00:00:00Z",
  cells: [],
};

// ── Setup ─────────────────────────────────────────────────────────────────────

beforeEach(() => {
  notebooksMock.mockReset();
  trashMutation.mockReset();
  restoreMutation.mockReset();
  addBookmarkMutation.mockReset();
  removeBookmarkMutation.mockReset();
  createMutation.mockReset();
  updateMutation.mockReset();
  deleteMutation.mockReset();
  trashMutateAsync.mockReset();
  restoreMutateAsync.mockReset();
  addBookmarkMutateAsync.mockReset();
  removeBookmarkMutateAsync.mockReset();
  createMutateAsync.mockReset();
  updateMutateAsync.mockReset();
  deleteMutateAsync.mockReset();

  notebooksMock.mockReturnValue({
    data: [mockNb1, mockNb2],
    isLoading: false,
    isError: false,
    error: null,
  });

  trashMutation.mockReturnValue({ mutateAsync: trashMutateAsync, isPending: false });
  restoreMutation.mockReturnValue({ mutateAsync: restoreMutateAsync, isPending: false });
  addBookmarkMutation.mockReturnValue({ mutateAsync: addBookmarkMutateAsync, isPending: false });
  removeBookmarkMutation.mockReturnValue({ mutateAsync: removeBookmarkMutateAsync, isPending: false });
  createMutation.mockReturnValue({ mutateAsync: createMutateAsync, isPending: false });
  updateMutation.mockReturnValue({ mutateAsync: updateMutateAsync, isPending: false });
  deleteMutation.mockReturnValue({ mutateAsync: deleteMutateAsync, isPending: false });

  trashMutateAsync.mockResolvedValue(undefined);
  restoreMutateAsync.mockResolvedValue(undefined);
  addBookmarkMutateAsync.mockResolvedValue(undefined);
  removeBookmarkMutateAsync.mockResolvedValue(undefined);
  createMutateAsync.mockResolvedValue(createdNotebook);
  updateMutateAsync.mockResolvedValue({ ...mockNb1, name: "Renamed" });
  deleteMutateAsync.mockResolvedValue(undefined);
});

// ── Tests ─────────────────────────────────────────────────────────────────────

describe("Notebooks list", () => {
  // ── List rendering ──────────────────────────────────────────────────────────

  it("renders notebook names from mocked useNotebooks", () => {
    renderAt();
    expect(screen.getByText("My Analysis")).toBeInTheDocument();
    expect(screen.getByText("Shared Report")).toBeInTheDocument();
  });

  it("renders as a table with Name / Visibility / Updated headers (no Engine)", () => {
    renderAt();
    expect(screen.getByRole("table")).toBeInTheDocument();
    expect(screen.queryByText("Engine")).not.toBeInTheDocument();
    expect(screen.getByText("Visibility")).toBeInTheDocument();
    expect(screen.getByText("Updated")).toBeInTheDocument();
  });

  it("shows a Public badge on a public notebook and Private on a private one", () => {
    renderAt();
    expect(screen.getAllByText("Public").length).toBe(1);
    expect(screen.getByText("Private")).toBeInTheDocument();
  });

  it("filters rows by the search box", async () => {
    const user = userEvent.setup();
    renderAt();
    await user.type(screen.getByPlaceholderText(/search notebooks/i), "Shared");
    expect(screen.queryByText("My Analysis")).not.toBeInTheDocument();
    expect(screen.getByText("Shared Report")).toBeInTheDocument();
  });

  it("shows empty state when list is []", () => {
    notebooksMock.mockReturnValue({ data: [], isLoading: false, isError: false, error: null });
    renderAt();
    expect(screen.getByText("No notebooks yet.")).toBeInTheDocument();
  });

  it("does not render rows while isLoading", () => {
    notebooksMock.mockReturnValue({ data: undefined, isLoading: true, isError: false, error: null });
    renderAt();
    expect(screen.queryByText("My Analysis")).not.toBeInTheDocument();
  });

  it("shows filtered empty message when bookmarks filter has no matches", () => {
    notebooksMock.mockReturnValue({
      data: [{ ...mockNb1, bookmarked: false }],
      isLoading: false,
      isError: false,
      error: null,
    });
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: "Bookmarks" }));
    expect(screen.getByText("No bookmarked notebooks.")).toBeInTheDocument();
  });

  // ── Bookmark star toggle ────────────────────────────────────────────────────

  it("clicking the star on an unbookmarked notebook calls addBookmark", async () => {
    const user = userEvent.setup();
    renderAt();
    // nb1 is not bookmarked, nb2 is bookmarked; find nb1's star by aria-label
    const star = screen.getByRole("button", { name: /bookmark My Analysis/i });
    await user.click(star);
    await waitFor(() => {
      expect(addBookmarkMutateAsync).toHaveBeenCalledWith("nb-1");
    });
  });

  it("clicking the star on a bookmarked notebook calls removeBookmark", async () => {
    const user = userEvent.setup();
    renderAt();
    // nb2 is bookmarked
    const star = screen.getByRole("button", { name: /unbookmark Shared Report/i });
    await user.click(star);
    await waitFor(() => {
      expect(removeBookmarkMutateAsync).toHaveBeenCalledWith("nb-2");
    });
  });

  it("bookmarks filter toggle calls useNotebooks without trashed (trash starts false)", () => {
    renderAt();
    // useNotebooks is called with (ws, {trashed: false}) on mount
    expect(notebooksMock).toHaveBeenCalledWith("eng", { trashed: false });
  });

  // ── Trash toggle ────────────────────────────────────────────────────────────

  it("clicking Trash button calls useNotebooks with {trashed: true}", () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: "Trash" }));
    expect(notebooksMock).toHaveBeenCalledWith("eng", { trashed: true });
  });

  it("trash view shows 'Trash is empty.' when data is empty", () => {
    notebooksMock.mockImplementation((_ws: string, opts?: { trashed?: boolean }) => ({
      data: opts?.trashed ? [] : [mockNb1, mockNb2],
      isLoading: false,
    }));
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: "Trash" }));
    expect(screen.getByText("Trash is empty.")).toBeInTheDocument();
  });

  it("trash view shows Restore and Delete permanently actions in row menu", async () => {
    const trashedNb = { ...mockNb1, deleted_at: "2026-06-20T00:00:00Z" };
    notebooksMock.mockImplementation((_ws: string, opts?: { trashed?: boolean }) => ({
      data: opts?.trashed ? [trashedNb] : [mockNb1, mockNb2],
      isLoading: false,
    }));
    const user = userEvent.setup();
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: "Trash" }));
    await screen.findByText("My Analysis");
    await user.click(screen.getAllByRole("button", { name: "Actions" })[0]);
    expect(await screen.findByText("Restore")).toBeInTheDocument();
    expect(screen.getByText("Delete permanently")).toBeInTheDocument();
  });

  it("Restore action calls useRestoreNotebook", async () => {
    const trashedNb = { ...mockNb1, deleted_at: "2026-06-20T00:00:00Z" };
    notebooksMock.mockImplementation((_ws: string, opts?: { trashed?: boolean }) => ({
      data: opts?.trashed ? [trashedNb] : [mockNb1, mockNb2],
      isLoading: false,
    }));
    const user = userEvent.setup();
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: "Trash" }));
    await screen.findByText("My Analysis");
    await user.click(screen.getAllByRole("button", { name: "Actions" })[0]);
    await user.click(await screen.findByText("Restore"));
    await waitFor(() => {
      expect(restoreMutateAsync).toHaveBeenCalledWith("nb-1");
    });
  });

  // ── New notebook dialog ─────────────────────────────────────────────────────

  it("clicking '+ Notebook' opens dialog; submitting calls useCreateNotebook mutateAsync with {name}", async () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /\+ notebook/i }));

    await waitFor(() => screen.getByText(/Create notebook/i));

    const nameInput = screen.getByLabelText(/^name$/i);
    fireEvent.change(nameInput, { target: { value: "Test NB" } });

    fireEvent.click(screen.getByRole("button", { name: /^create$/i }));

    await waitFor(() => {
      expect(createMutateAsync).toHaveBeenCalledWith(
        expect.objectContaining({ name: "Test NB" }),
      );
    });
  });

  it("navigates to new notebook after create", async () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /\+ notebook/i }));

    await waitFor(() => screen.getByText(/Create notebook/i));

    fireEvent.click(screen.getByRole("button", { name: /^create$/i }));

    await waitFor(() => {
      expect(screen.getByText("NotebookPage")).toBeInTheDocument();
    });
  });

  // ── Row actions menu → Rename ───────────────────────────────────────────────

  it("rename via the row actions menu → calls useUpdateNotebook mutateAsync with {name}", async () => {
    const user = userEvent.setup();
    renderAt();

    await user.click(screen.getAllByRole("button", { name: "Actions" })[0]);
    await user.click(await screen.findByText("Rename"));

    const nameInput = await screen.findByRole("textbox", { name: /name/i });
    fireEvent.change(nameInput, { target: { value: "Renamed" } });

    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));

    await waitFor(() => {
      expect(updateMutateAsync).toHaveBeenCalledWith(
        expect.objectContaining({ name: "Renamed" }),
      );
    });
  });

  // ── Row actions menu → Move to Trash ───────────────────────────────────────

  it("row menu → Move to Trash calls useTrashNotebook", async () => {
    const user = userEvent.setup();
    renderAt();

    // Default sort is updated_at desc; nb-2 (2026-06-02) sorts before nb-1 (2026-06-01)
    await user.click(screen.getAllByRole("button", { name: "Actions" })[0]);
    await user.click(await screen.findByText("Move to Trash"));

    await waitFor(() => {
      expect(trashMutateAsync).toHaveBeenCalledWith("nb-2");
    });
  });

  // ── Row actions menu → Delete permanently ─────────────────────────────────

  it("permanent delete uses ConfirmDeleteDialog and calls useDeleteNotebook", async () => {
    const trashedNb = { ...mockNb1, deleted_at: "2026-06-20T00:00:00Z" };
    notebooksMock.mockImplementation((_ws: string, opts?: { trashed?: boolean }) => ({
      data: opts?.trashed ? [trashedNb] : [mockNb1, mockNb2],
      isLoading: false,
    }));
    const user = userEvent.setup();
    renderAt();
    // Switch to trash view
    fireEvent.click(screen.getByRole("button", { name: "Trash" }));
    await screen.findByText("My Analysis");
    await user.click(screen.getAllByRole("button", { name: "Actions" })[0]);
    await user.click(await screen.findByText("Delete permanently"));

    // ConfirmDeleteDialog should open
    const input = await screen.findByPlaceholderText("delete");
    fireEvent.change(input, { target: { value: "delete" } });
    fireEvent.click(screen.getByRole("button", { name: /^delete$/i }));

    await waitFor(() => {
      expect(deleteMutateAsync).toHaveBeenCalledWith("nb-1");
    });
  });

  // ── Row actions menu → Share ────────────────────────────────────────────────

  it("row menu → Share… opens the ShareDialog scoped to the notebook", async () => {
    const user = userEvent.setup();
    renderAt();

    // Default sort is updated_at desc; nb-2 "Shared Report" is the first row.
    await user.click(screen.getAllByRole("button", { name: "Actions" })[0]);
    await user.click(await screen.findByText("Share…"));

    // Dialog opens with the notebook name and notebook-scoped copy.
    expect(await screen.findByText('Share "Shared Report"')).toBeInTheDocument();
    expect(
      screen.getByText("Grant people and groups access to this notebook."),
    ).toBeInTheDocument();
  });

  // ── Header button cluster ───────────────────────────────────────────────────

  it("header shows Bookmarks, Bulk select (when data), Trash, New folder, and + Notebook buttons", () => {
    renderAt();
    expect(screen.getByRole("button", { name: "Bookmarks" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Bulk select" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Trash" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "New folder" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /\+ notebook/i })).toBeInTheDocument();
  });

  it("header title changes to 'Trash' when trash view is active", () => {
    renderAt();
    expect(screen.getByRole("heading", { name: "Notebooks" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Trash" }));
    expect(screen.getByRole("heading", { name: "Trash" })).toBeInTheDocument();
  });

  // ── Bulk select ─────────────────────────────────────────────────────────────

  it("clicking Bulk select shows checkboxes", async () => {
    const user = userEvent.setup();
    renderAt();
    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    const checkboxes = screen.getAllByRole("checkbox");
    // 1 select-all + 2 per row
    expect(checkboxes.length).toBeGreaterThanOrEqual(2);
  });

  // ── useNotebooks called with trashed: false on mount ────────────────────────

  it("passes {trashed: false} to useNotebooks on initial render", () => {
    renderAt();
    expect(notebooksMock).toHaveBeenCalledWith("eng", { trashed: false });
  });

  // ── Notebook settings gear button ──────────────────────────────────────────

  it("renders a 'Notebook settings' gear button in the header", () => {
    renderAt();
    expect(
      screen.getByRole("button", { name: "Notebook settings" }),
    ).toBeInTheDocument();
  });

  it("clicking 'Notebook settings' navigates to /explore/:workspace/notebook-settings", async () => {
    const qc = new QueryClient();
    render(
      <QueryClientProvider client={qc}>
        <TooltipProvider>
          <MemoryRouter initialEntries={["/explore/eng/notebooks"]}>
            <Routes>
              <Route path="/explore/:workspace/notebooks" element={<Notebooks />} />
              <Route
                path="/explore/:workspace/notebook-settings"
                element={<div>NotebookSettingsPage</div>}
              />
            </Routes>
          </MemoryRouter>
        </TooltipProvider>
      </QueryClientProvider>,
    );

    fireEvent.click(screen.getByRole("button", { name: "Notebook settings" }));
    await waitFor(() => {
      expect(screen.getByText("NotebookSettingsPage")).toBeInTheDocument();
    });
  });
});
