import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import { folderCollapseKey, UNCATEGORIZED_KEY } from "@/components/explore/folders";

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

const foldersMock = vi.fn(() => ({ data: [] as { id: string; name: string }[] }));

vi.mock("@/api/folders", () => ({
  useFolders: () => foldersMock(),
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
  foldersMock.mockReset();
  foldersMock.mockReturnValue({ data: [] });
  // `useCollapsed` persists collapse state in localStorage under
  // `folders:eng:notebooks`; without this a test that collapses a folder leaks
  // that collapse into every later test in the file.
  localStorage.clear();

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

  // ── Folder header menu → Share ──────────────────────────────────────────────

  it("folder header menu → Share… opens the ShareDialog scoped to the folder", async () => {
    foldersMock.mockReturnValue({ data: [{ id: "fold-1", name: "Quarterly" }] });
    const user = userEvent.setup();
    renderAt();

    await user.click(await screen.findByRole("button", { name: "Folder Quarterly actions" }));
    await user.click(await screen.findByText("Share…"));

    // Dialog opens scoped to the folder (kind="folder").
    expect(await screen.findByText('Share "Quarterly"')).toBeInTheDocument();
    expect(
      screen.getByText("Grant people and groups access to this folder."),
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

/**
 * Bulk select must only ever reach rows that are ON SCREEN. `useBulkSelect` was
 * fed the pre-grouping `rows`, so "Select all" swept in the rows of a collapsed
 * folder and "Move n to Trash" trashed notebooks the user had never seen.
 *
 * The fixture puts one notebook in each of TWO folders and collapses only one.
 * A fixture with everything inside the collapsed folder cannot tell a correct
 * fix from one that selects nothing at all — both end at an empty selection.
 */
describe("Notebooks — bulk select only reaches rows on screen", () => {
  const TWO_FOLDERS = [{ id: "f-rep", name: "Reporting" }, { id: "f-fin", name: "Finance" }];
  const hiddenNb = { ...mockNb1, id: "nb-hidden", name: "Hidden notebook", folder_id: "f-rep" };
  const shownNb = { ...mockNb2, id: "nb-shown", name: "Shown notebook", folder_id: "f-fin" };

  function arrangeTwoFolders() {
    notebooksMock.mockReturnValue({ data: [hiddenNb, shownNb], isLoading: false, isError: false, error: null });
    foldersMock.mockReturnValue({ data: TWO_FOLDERS });
  }

  it("leaves a collapsed folder's rows out of Select all and out of the trash call", async () => {
    arrangeTwoFolders();
    const user = userEvent.setup();
    renderAt();
    // Both folders start expanded, so both rows are genuinely on screen first.
    expect(screen.getByText("Hidden notebook")).toBeInTheDocument();
    expect(screen.getByText("Shown notebook")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Toggle Reporting" }));
    expect(screen.queryByText("Hidden notebook")).not.toBeInTheDocument();
    // Finance is still open — the fix has something left to select.
    expect(screen.getByText("Shown notebook")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    await user.click(screen.getByLabelText("Select all"));

    expect(screen.getByRole("button", { name: "Move 1 to Trash" })).toBeInTheDocument();
    expect(screen.queryByLabelText("Select Hidden notebook")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Move 1 to Trash" }));

    // The ids, not merely that it was called: `toHaveBeenCalled()` is satisfied
    // by trashing exactly the wrong notebook, which is the whole defect.
    await waitFor(() => expect(trashMutateAsync).toHaveBeenCalledWith("nb-shown"));
    expect(trashMutateAsync).not.toHaveBeenCalledWith("nb-hidden");
    expect(trashMutateAsync).toHaveBeenCalledTimes(1);
  });

  it("drops a row from the selection when its folder collapses after it was picked", async () => {
    arrangeTwoFolders();
    const user = userEvent.setup();
    renderAt();

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    // SELECT first, while it is legitimately on screen…
    await user.click(screen.getByLabelText("Select Hidden notebook"));
    await user.click(screen.getByLabelText("Select Shown notebook"));
    expect(screen.getByRole("button", { name: "Move 2 to Trash" })).toBeInTheDocument();

    // …then HIDE it. `toggleAll` is not involved: the id is already in
    // `selected`, and only `selectedIds`' re-filter can take it back out.
    await user.click(screen.getByRole("button", { name: "Toggle Reporting" }));
    expect(screen.getByRole("button", { name: "Move 1 to Trash" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Move 1 to Trash" }));

    await waitFor(() => expect(trashMutateAsync).toHaveBeenCalledWith("nb-shown"));
    expect(trashMutateAsync).not.toHaveBeenCalledWith("nb-hidden");
    expect(trashMutateAsync).toHaveBeenCalledTimes(1);
  });
});

/*
 * Item 1: the strip owns the singular. This page sets `singular` beside
 * `label`; the RULE (only the number one takes it) is tested once, on the
 * component, in `src/components/explore/__tests__/SummaryStrip.test.tsx`.
 * What is checked here is the WIRING — that the page passes the pair at all,
 * which is the half a component test cannot see and the half five pages had
 * missing.
 */
describe("Notebooks — the summary strip says `1 notebook`, not `1 notebooks`", () => {
  it("uses the singular for one row and one folder", () => {
    notebooksMock.mockReturnValue({ data: [mockNb1], isLoading: false, isError: false, error: null });
    foldersMock.mockReturnValue({ data: [{ id: "f1", name: "Revenue" }] });
    renderAt();
    expect(screen.getByText("notebook")).toBeInTheDocument();
    expect(screen.getByText("folder")).toBeInTheDocument();
    expect(screen.queryByText("notebooks")).not.toBeInTheDocument();
    expect(screen.queryByText("folders")).not.toBeInTheDocument();
  });

  it("goes back to the plural at two", () => {
    notebooksMock.mockReturnValue({ data: [mockNb1, mockNb2], isLoading: false, isError: false, error: null });
    foldersMock.mockReturnValue({ data: [{ id: "f1", name: "Revenue" }, { id: "f2", name: "Ops" }] });
    renderAt();
    expect(screen.getByText("notebooks")).toBeInTheDocument();
    expect(screen.getByText("folders")).toBeInTheDocument();
  });
});

/*
 * Item 2: the folderless-header gate, lifted from `dataapi/Endpoints.tsx` and
 * `sqllab/SavedTab.tsx` (the original precedent) into the five siblings that
 * headed the folderless bucket unconditionally.
 *
 * Both fixtures matter and neither is optional. Without folders is the case
 * being fixed; WITH a folder is what tells a working gate apart from one
 * wired permanently off, which the first test alone cannot see.
 */
describe("Notebooks — no folders, no folderless header", () => {
  it("draws a flat table when the workspace has no folders", () => {
    foldersMock.mockReturnValue({ data: [] });
    renderAt();
    expect(screen.getByText("My Analysis")).toBeInTheDocument();
    expect(screen.getByText("Shared Report")).toBeInTheDocument();
    // "Uncategorized" beside nothing labels the whole list after the one thing
    // it is not, and its collapse control's only effect is to hide everything.
    expect(screen.queryByRole("button", { name: "Toggle Uncategorized" })).not.toBeInTheDocument();
  });

  it("still heads the folderless bucket beside a real folder", () => {
    notebooksMock.mockReturnValue({ data: [{ ...mockNb1, folder_id: "f1" }, mockNb2], isLoading: false, isError: false, error: null });
    foldersMock.mockReturnValue({ data: [{ id: "f1", name: "Revenue" }] });
    renderAt();
    expect(screen.getByRole("button", { name: "Toggle Revenue" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Toggle Uncategorized" })).toBeInTheDocument();
  });

  it("does not strand rows behind a collapse state whose header is no longer rendered", async () => {
    // What a previous session left behind: Uncategorized collapsed, from back
    // when this workspace still had folders. `useCollapsed` reads that out of
    // localStorage at mount and it survives the folders being deleted.
    //
    // Both halves are DERIVED, never spelled out: `folderCollapseKey` is the
    // same function the page hands `useCollapsed`, `UNCATEGORIZED_KEY` the same
    // constant it collapses under. A hardcoded key would land the seed where
    // nothing reads it and this test would still pass, because a seed that
    // misses is indistinguishable from a page that correctly ignored it.
    //
    // Rendering the rows is only half of it. `visibleRows` — what bulk select
    // acts on — is a SEPARATE derivation over the same collapse state, so
    // gating the render loop alone leaves "Select all" sweeping zero rows while
    // two sit in front of the user with no header left anywhere to expand.
    localStorage.setItem(folderCollapseKey("eng", "notebooks"), JSON.stringify([UNCATEGORIZED_KEY]));
    foldersMock.mockReturnValue({ data: [] });
    const user = userEvent.setup();
    renderAt();
    expect(screen.getByText("My Analysis")).toBeInTheDocument();
    expect(screen.getByText("Shared Report")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    await user.click(screen.getByLabelText("Select all"));
    expect(screen.getByRole("button", { name: "Move 2 to Trash" })).toBeInTheDocument();
  });
});
