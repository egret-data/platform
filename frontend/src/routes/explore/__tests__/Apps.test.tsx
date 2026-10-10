import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route, useLocation } from "react-router-dom";
import { TooltipProvider } from "@/components/ui/tooltip";

const useAppsMock = vi.fn();
const createMutateAsync = vi.fn();
const m = {
  update: vi.fn(),
  trash: vi.fn(),
  restore: vi.fn(),
  remove: vi.fn(),
  duplicate: vi.fn(),
  addBookmark: vi.fn(),
  removeBookmark: vi.fn(),
};
const setFolderMock = vi.fn();
const capsMock = vi.fn();
let folders: { id: string; name: string }[] = [];

vi.mock("@/api/apps", () => ({
  useApps: (...a: unknown[]) => useAppsMock(...a),
  useAppCapabilities: (...a: unknown[]) => capsMock(...a),
  useCreateApp: () => ({ mutateAsync: createMutateAsync, isPending: false }),
  useAppMutations: () => ({
    update: { mutateAsync: m.update, isPending: false },
    trash: { mutateAsync: m.trash },
    restore: { mutateAsync: m.restore },
    remove: { mutateAsync: m.remove },
    duplicate: { mutateAsync: m.duplicate },
    addBookmark: { mutateAsync: m.addBookmark },
    removeBookmark: { mutateAsync: m.removeBookmark },
  }),
}));
vi.mock("@/api/folders", () => ({
  useFolders: () => ({ data: folders }),
  useSetFolder: () => ({ mutateAsync: setFolderMock }),
  useFolderMutations: () => ({
    create: { mutateAsync: vi.fn().mockResolvedValue({ id: "f-new", name: "New" }) },
    rename: { mutateAsync: vi.fn().mockResolvedValue({}) },
    remove: { mutateAsync: vi.fn().mockResolvedValue(undefined) },
  }),
}));
// The real dialog talks to the shares API; what this page owns is WHICH object it is opened on.
vi.mock("@/components/sharing/ShareDialog", () => ({
  ShareDialog: (p: { kind: string; id: string; name: string; folderId?: string | null }) => (
    <div data-testid="share-dialog">{`${p.kind}:${p.id}:${p.name}:${p.folderId ?? "none"}`}</div>
  ),
}));
vi.mock("sonner", () => ({ toast: { error: vi.fn(), success: vi.fn() } }));

import { toast } from "sonner";
import { ApiError } from "@/api/client";
import Apps from "@/routes/explore/Apps";

function Where() {
  return <div data-testid="where">{useLocation().pathname}</div>;
}

function renderApps() {
  return render(
    <TooltipProvider>
      <MemoryRouter initialEntries={["/explore/acme/apps"]}>
        <Routes>
          <Route path="/explore/:workspace/apps" element={<Apps />} />
          <Route path="/explore/:workspace/apps/:id/edit" element={<Where />} />
          <Route path="/explore/:workspace/apps/:id" element={<Where />} />
        </Routes>
      </MemoryRouter>
    </TooltipProvider>,
  );
}

// An author's row by default: the per-row flags are what most cases below vary.
const app = (over: Record<string, unknown>) => ({
  id: "a1",
  name: "Revenue",
  description: "",
  updated_at: "2026-10-01T00:00:00Z",
  latest_version_no: null,
  draft_revision: 1,
  is_owner: true,
  bookmarked: false,
  folder_id: null,
  can_write: true,
  can_manage: true,
  ...over,
});
const READ_ONLY = { can_write: false, can_manage: false };

/** Serve `live` for the list and `trashed` for the trash bin, keyed on what the page asks for. */
function serve(live: unknown[], trashed: unknown[] = []) {
  useAppsMock.mockImplementation((_ws: string, opts?: { trashed?: boolean }) => ({
    data: opts?.trashed ? trashed : live,
    isLoading: false,
    isError: false,
  }));
}

const rowOf = (name: string) => screen.getByText(name).closest("tr")!;
async function openMenu(name: string) {
  await userEvent.click(within(rowOf(name)).getByRole("button", { name: "Actions" }));
}
const menuItems = () => screen.getAllByRole("menuitem").map((el) => el.textContent);

beforeEach(() => {
  vi.clearAllMocks();
  useAppsMock.mockReset();
  createMutateAsync.mockReset();
  for (const fn of Object.values(m)) fn.mockReset().mockResolvedValue(undefined);
  setFolderMock.mockReset().mockResolvedValue({});
  // An author by default; the workspace-level tests override it.
  capsMock.mockReset().mockReturnValue({ data: { can_create: true } });
  folders = [];
  // `useCollapsed` persists collapse state in localStorage; without this a test that collapses
  // a folder leaks that collapse into every later test in the file.
  localStorage.clear();
});

describe("Apps list", () => {
  it("says whether each app is a draft or which version is live", () => {
    serve([app({ id: "a1", name: "Draft app" }), app({ id: "a2", name: "Live app", latest_version_no: 3 })]);
    renderApps();
    expect(screen.getByText("Draft only")).toBeTruthy();
    expect(screen.getByText("Published v3")).toBeTruthy();
  });

  it("renders exactly one h1, and it is the page title", () => {
    serve([app({})]);
    renderApps();
    const h1s = screen.getAllByRole("heading", { level: 1 });
    expect(h1s).toHaveLength(1);
    expect(h1s[0]).toHaveTextContent("Apps");
  });

  it("explains an empty workspace", () => {
    serve([]);
    renderApps();
    expect(screen.getByText(/No apps yet/)).toBeTruthy();
  });

  it("creates an app and goes straight to its editor", async () => {
    serve([]);
    createMutateAsync.mockResolvedValue({ id: "new-id", name: "Fresh" });
    renderApps();
    fireEvent.click(screen.getByRole("button", { name: "+ App" }));
    const input = await screen.findByLabelText("Name");
    fireEvent.change(input, { target: { value: "  Fresh  " } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(createMutateAsync).toHaveBeenCalledWith({ name: "Fresh" }));
    await waitFor(() => expect(screen.getByTestId("where").textContent).toBe("/explore/acme/apps/new-id/edit"));
  });

  it("will not create an app with no name", async () => {
    serve([]);
    renderApps();
    fireEvent.click(screen.getByRole("button", { name: "+ App" }));
    await screen.findByLabelText("Name");
    expect((screen.getByRole("button", { name: "Create" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("shows a load error", () => {
    useAppsMock.mockReturnValue({ data: undefined, isLoading: false, isError: true, error: new Error("boom") });
    renderApps();
    expect(screen.getByText("boom")).toBeTruthy();
  });

  it("shows the error, not the cached rows, when a refetch fails", () => {
    useAppsMock.mockReturnValue({ data: [app({ name: "Cached" })], isLoading: false, isError: true, error: new Error("boom") });
    renderApps();
    expect(screen.getByText("boom")).toBeTruthy();
    expect(screen.queryByText("Cached")).toBeNull();
  });

  it("filters by name", async () => {
    serve([app({ id: "a1", name: "Revenue" }), app({ id: "a2", name: "Churn" })]);
    renderApps();
    await userEvent.type(screen.getByPlaceholderText("Search apps…"), "chu");
    expect(screen.queryByText("Revenue")).toBeNull();
    expect(screen.getByText("Churn")).toBeTruthy();
  });
});

describe("Apps list — where the name leads", () => {
  // Published wins over everything; the editor is only for someone who may edit; a reader of a
  // never-published app has nowhere to go, so there is no link to refuse them.
  it.each([
    { label: "published, writable", over: { latest_version_no: 2 }, href: "/explore/acme/apps/a9" },
    { label: "published, read-only", over: { latest_version_no: 2, ...READ_ONLY }, href: "/explore/acme/apps/a9" },
    { label: "draft only, writable", over: {}, href: "/explore/acme/apps/a9/edit" },
    { label: "draft only, read-only", over: { ...READ_ONLY }, href: null },
  ])("$label → $href", ({ over, href }) => {
    serve([app({ id: "a9", name: "The app", ...over })]);
    renderApps();
    if (href === null) {
      expect(screen.getByText("The app")).toBeTruthy();
      expect(screen.queryByRole("link", { name: "The app" })).toBeNull();
    } else {
      expect(screen.getByRole("link", { name: "The app" }).getAttribute("href")).toBe(href);
    }
  });
});

describe("Apps list — row menu", () => {
  it("has no per-row Open / Edit buttons any more: they live in the menu", async () => {
    serve([app({ id: "a9", name: "Draft app" })]);
    renderApps();
    expect(screen.queryByRole("button", { name: /Open/ })).toBeNull();
    expect(screen.queryByRole("link", { name: /Edit/ })).toBeNull();

    await openMenu("Draft app");
    // A draft-only app's Open stays a disabled promise.
    expect(screen.getByRole("menuitem", { name: "Open" })).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(screen.getByRole("menuitem", { name: "Edit" }));
    expect(screen.getByTestId("where").textContent).toBe("/explore/acme/apps/a9/edit");
  });

  it("a published app's Open goes to the viewer page", async () => {
    serve([app({ id: "a9", name: "Live app", latest_version_no: 2 })]);
    renderApps();
    await openMenu("Live app");
    await userEvent.click(screen.getByRole("menuitem", { name: "Open" }));
    expect(screen.getByTestId("where").textContent).toBe("/explore/acme/apps/a9");
  });

  it("gates each row on ITS OWN flags, not on its neighbour's", async () => {
    // One list, three callers' worth of rows: the page must not read row 0 and apply it to all.
    serve([
      app({ id: "a1", name: "Mine", latest_version_no: 1 }),
      app({ id: "a2", name: "Shared to edit", latest_version_no: 1, can_manage: false }),
      app({ id: "a3", name: "Read only", latest_version_no: 1, ...READ_ONLY }),
    ]);
    renderApps();

    await openMenu("Read only");
    expect(menuItems()).toEqual(["Open"]);
    await userEvent.keyboard("{Escape}");

    await openMenu("Shared to edit");
    expect(menuItems()).toEqual(["Open", "Edit", "Rename…", "Duplicate", "Move to new folder…", "Move to Trash"]);
    await userEvent.keyboard("{Escape}");

    await openMenu("Mine");
    expect(menuItems()).toEqual(["Open", "Edit", "Rename…", "Duplicate", "Move to new folder…", "Share…", "Move to Trash"]);
  });

  it("Move to Trash trashes that app and says so", async () => {
    serve([app({ id: "a1", name: "One" }), app({ id: "a2", name: "Two" })]);
    renderApps();
    await openMenu("Two");
    await userEvent.click(screen.getByRole("menuitem", { name: "Move to Trash" }));
    await waitFor(() => expect(m.trash).toHaveBeenCalledWith("a2"));
    expect(m.trash).toHaveBeenCalledTimes(1);
    expect(m.remove).not.toHaveBeenCalled();
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Moved to Trash"));
  });

  it("Duplicate goes to the copy's editor", async () => {
    serve([app({ id: "src", name: "Source" })]);
    m.duplicate.mockResolvedValue({ id: "copy1", name: "Copy of Source" });
    renderApps();
    await openMenu("Source");
    await userEvent.click(screen.getByRole("menuitem", { name: "Duplicate" }));
    await waitFor(() => expect(m.duplicate).toHaveBeenCalledWith("src"));
    await waitFor(() => expect(screen.getByTestId("where").textContent).toBe("/explore/acme/apps/copy1/edit"));
  });

  it("a Duplicate refused with 403 says what is missing, and stays on the list", async () => {
    // The share-only collaborator: `can_write` on the row is true, and the server still wants
    // workspace-level write. The server's whole answer is the word "forbidden" (seen live), which
    // tells them nothing — so the page says which permission the copy needed.
    serve([app({ id: "src", name: "Source", can_manage: false })]);
    m.duplicate.mockRejectedValue(new ApiError(403, "forbidden"));
    renderApps();
    await openMenu("Source");
    await userEvent.click(screen.getByRole("menuitem", { name: "Duplicate" }));
    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith(
        "You can edit this app, but making a copy needs permission to create apps in this workspace.",
      ),
    );
    expect(screen.queryByTestId("where")).toBeNull();
    expect(screen.getByText("Source")).toBeTruthy();
  });

  it("a Duplicate that fails for any other reason shows the server's words", async () => {
    serve([app({ id: "src", name: "Source" })]);
    m.duplicate.mockRejectedValue(new ApiError(409, "app is in the trash"));
    renderApps();
    await openMenu("Source");
    await userEvent.click(screen.getByRole("menuitem", { name: "Duplicate" }));
    await waitFor(() => expect(toast.error).toHaveBeenCalledWith("app is in the trash"));
  });

  it("Rename… opens the dialog on that app and saves through PATCH", async () => {
    serve([app({ id: "a1", name: "One" }), app({ id: "a2", name: "Two", description: "second" })]);
    m.update.mockResolvedValue({ id: "a2" });
    renderApps();
    await openMenu("Two");
    await userEvent.click(screen.getByRole("menuitem", { name: "Rename…" }));
    const name = (await screen.findByLabelText("Name")) as HTMLInputElement;
    expect(name.value).toBe("Two");
    fireEvent.change(name, { target: { value: "Two again" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(m.update).toHaveBeenCalledWith({ id: "a2", name: "Two again", description: "second" }));
    await waitFor(() => expect(screen.queryByRole("button", { name: "Save" })).toBeNull());
  });

  it("Share… opens the share dialog on the app, carrying its folder", async () => {
    folders = [{ id: "f1", name: "Finance" }];
    serve([app({ id: "a7", name: "Shared one", folder_id: "f1" })]);
    renderApps();
    await openMenu("Shared one");
    await userEvent.click(screen.getByRole("menuitem", { name: "Share…" }));
    expect(screen.getByTestId("share-dialog").textContent).toBe("app:a7:Shared one:f1");
  });

  it("moving to a folder sets that app's folder", async () => {
    folders = [{ id: "f1", name: "Finance" }];
    serve([app({ id: "a7", name: "Loose" })]);
    renderApps();
    await openMenu("Loose");
    await userEvent.click(screen.getByRole("menuitem", { name: 'Move to "Finance"' }));
    await waitFor(() => expect(setFolderMock).toHaveBeenCalledWith({ id: "a7", folder_id: "f1" }));
  });
});

describe("Apps list — bookmarks", () => {
  it("offers the star on every row, read-only ones included, and toggles the right way", async () => {
    serve([
      app({ id: "a1", name: "Plain", ...READ_ONLY }),
      app({ id: "a2", name: "Starred", bookmarked: true, ...READ_ONLY }),
    ]);
    renderApps();
    await userEvent.click(screen.getByRole("button", { name: "bookmark Plain" }));
    await userEvent.click(screen.getByRole("button", { name: "unbookmark Starred" }));
    expect(m.addBookmark).toHaveBeenCalledWith("a1");
    expect(m.addBookmark).toHaveBeenCalledTimes(1);
    expect(m.removeBookmark).toHaveBeenCalledWith("a2");
    expect(m.removeBookmark).toHaveBeenCalledTimes(1);
  });

  it("the Bookmarks filter keeps only bookmarked apps and reports its state", async () => {
    serve([app({ id: "a1", name: "Plain" }), app({ id: "a2", name: "Starred", bookmarked: true })]);
    renderApps();
    const btn = screen.getByRole("button", { name: "Bookmarks" });
    expect(btn).toHaveAttribute("aria-pressed", "false");
    await userEvent.click(btn);
    expect(btn).toHaveAttribute("aria-pressed", "true");
    expect(screen.queryByText("Plain")).toBeNull();
    expect(screen.getByText("Starred")).toBeTruthy();
  });

  it("says so when nothing is bookmarked", async () => {
    serve([app({ id: "a1", name: "Plain" })]);
    renderApps();
    await userEvent.click(screen.getByRole("button", { name: "Bookmarks" }));
    expect(screen.getByText("No bookmarked apps.")).toBeTruthy();
  });
});

describe("Apps list — trash view", () => {
  it("asks the server for the trash, not a filter of the live list", async () => {
    serve([app({ id: "a1", name: "Live one" })], [app({ id: "t1", name: "Binned one", deleted_at: "2026-10-02T00:00:00Z" })]);
    renderApps();
    expect(useAppsMock).toHaveBeenLastCalledWith("acme", { trashed: false });
    expect(screen.getByText("Live one")).toBeTruthy();

    await userEvent.click(screen.getByRole("button", { name: "Trash" }));

    expect(useAppsMock).toHaveBeenLastCalledWith("acme", { trashed: true });
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Trash");
    expect(screen.getByText("Binned one")).toBeTruthy();
    expect(screen.queryByText("Live one")).toBeNull();
    // No star, no bulk select, no folder creation in the bin.
    expect(screen.queryByRole("button", { name: /bookmark Binned one/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "Bulk select" })).toBeNull();
    expect(screen.queryByRole("button", { name: "New folder" })).toBeNull();
  });

  it("keeps the Trash switch when the live list is empty, and says when the bin is too", async () => {
    serve([], []);
    renderApps();
    await userEvent.click(screen.getByRole("button", { name: "Trash" }));
    expect(screen.getByText("Trash is empty.")).toBeTruthy();
    // Still there: it is the only way back out.
    expect(screen.getByRole("button", { name: "Trash" })).toHaveAttribute("aria-pressed", "true");
  });

  it("Restore restores that app", async () => {
    serve([], [app({ id: "t1", name: "Binned one" })]);
    renderApps();
    await userEvent.click(screen.getByRole("button", { name: "Trash" }));
    await openMenu("Binned one");
    await userEvent.click(screen.getByRole("menuitem", { name: "Restore" }));
    await waitFor(() => expect(m.restore).toHaveBeenCalledWith("t1"));
    expect(m.remove).not.toHaveBeenCalled();
  });

  it("Delete permanently deletes only after the typed confirmation", async () => {
    serve([], [app({ id: "t1", name: "Binned one" })]);
    renderApps();
    await userEvent.click(screen.getByRole("button", { name: "Trash" }));
    await openMenu("Binned one");
    await userEvent.click(screen.getByRole("menuitem", { name: "Delete permanently" }));

    // The dialog names the app, and nothing has been deleted by opening it.
    const input = await screen.findByPlaceholderText("delete");
    expect(screen.getByRole("alertdialog").textContent).toContain('"Binned one"');
    expect(m.remove).not.toHaveBeenCalled();

    // The confirm button does nothing until the word is typed.
    fireEvent.click(screen.getByRole("button", { name: /^Delete$/ }));
    expect(m.remove).not.toHaveBeenCalled();

    fireEvent.change(input, { target: { value: "delete" } });
    fireEvent.click(screen.getByRole("button", { name: /^Delete$/ }));
    await waitFor(() => expect(m.remove).toHaveBeenCalledWith("t1"));
    expect(m.remove).toHaveBeenCalledTimes(1);
    // The hard delete, not the trash/restore path.
    expect(m.trash).not.toHaveBeenCalled();
    expect(m.restore).not.toHaveBeenCalled();
  });

  it("gives each binned row only what its caller may do — and no trigger for neither", async () => {
    serve([], [
      app({ id: "t1", name: "Can restore", can_manage: false }),
      app({ id: "t2", name: "Can do both" }),
      app({ id: "t3", name: "Can do nothing", ...READ_ONLY }),
    ]);
    renderApps();
    await userEvent.click(screen.getByRole("button", { name: "Trash" }));

    expect(within(rowOf("Can do nothing")).queryByRole("button", { name: "Actions" })).toBeNull();

    await openMenu("Can restore");
    expect(menuItems()).toEqual(["Restore"]);
    await userEvent.keyboard("{Escape}");

    await openMenu("Can do both");
    expect(menuItems()).toEqual(["Restore", "Delete permanently"]);
  });
});

/**
 * Bulk select over rows the caller may and may not write. The count beside "Move n to Trash"
 * must be the number of rows the server will accept, so a read-only row can be neither ticked
 * nor swept in by Select all.
 *
 * Writable rows sit on BOTH sides of the read-only one, so a fix that stopped at the first
 * read-only row, or selected nothing, cannot pass.
 */
describe("Apps list — bulk select over mixed-permission rows", () => {
  const MIXED = [
    app({ id: "w1", name: "Writable one", updated_at: "2026-10-03T00:00:00Z" }),
    app({ id: "r1", name: "Read only", updated_at: "2026-10-02T00:00:00Z", ...READ_ONLY }),
    app({ id: "w2", name: "Writable two", updated_at: "2026-10-01T00:00:00Z", can_manage: false }),
  ];

  it("draws a checkbox only on writable rows", async () => {
    serve(MIXED);
    renderApps();
    await userEvent.click(screen.getByRole("button", { name: "Bulk select" }));
    expect(screen.getByLabelText("Select Writable one")).toBeTruthy();
    expect(screen.getByLabelText("Select Writable two")).toBeTruthy();
    expect(screen.queryByLabelText("Select Read only")).toBeNull();
    // The row itself is still listed — only its checkbox is withheld.
    expect(screen.getByText("Read only")).toBeTruthy();
  });

  it("Select all counts and trashes exactly the writable rows", async () => {
    serve(MIXED);
    renderApps();
    await userEvent.click(screen.getByRole("button", { name: "Bulk select" }));
    await userEvent.click(screen.getByLabelText("Select all"));

    expect(screen.getByRole("button", { name: "Move 2 to Trash" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Move 2 to folder…" })).toBeTruthy();
    // All the selectable rows are ticked, so the header box reads as fully on.
    expect(screen.getByLabelText("Select all")).toHaveAttribute("aria-checked", "true");

    await userEvent.click(screen.getByRole("button", { name: "Move 2 to Trash" }));

    // The ids, not merely the count: trashing the wrong two would satisfy a call count.
    await waitFor(() => expect(m.trash).toHaveBeenCalledTimes(2));
    expect(m.trash.mock.calls.map((c) => c[0]).sort()).toEqual(["w1", "w2"]);
    expect(m.trash).not.toHaveBeenCalledWith("r1");
  });

  const selectAllThenOpenMove = async (label: string) => {
    await userEvent.click(screen.getByRole("button", { name: "Bulk select" }));
    await userEvent.click(screen.getByLabelText("Select all"));
    await userEvent.click(screen.getByRole("button", { name: label }));
  };

  it("moves exactly the selected writable rows to the folder picked from the list", async () => {
    folders = [{ id: "f1", name: "Finance" }, { id: "f2", name: "Ops" }];
    serve(MIXED);
    renderApps();
    await selectAllThenOpenMove("Move 2 to folder…");
    // A picker of what exists, not a name to type: the old dialog created a folder for any
    // name it did not recognise, including a typo of an existing one.
    expect(menuItems()).toEqual(["Finance", "Ops", "Uncategorized", "New folder…"]);
    await userEvent.click(screen.getByRole("menuitem", { name: "Ops" }));

    await waitFor(() => expect(setFolderMock).toHaveBeenCalledTimes(2));
    expect(setFolderMock.mock.calls.map((c) => c[0])).toEqual(
      expect.arrayContaining([{ id: "w1", folder_id: "f2" }, { id: "w2", folder_id: "f2" }]),
    );
    expect(setFolderMock).not.toHaveBeenCalledWith(expect.objectContaining({ id: "r1" }));
  });

  it("Uncategorized takes the selected rows out of their folders", async () => {
    folders = [{ id: "f1", name: "Finance" }];
    serve([app({ id: "w1", name: "One", folder_id: "f1" }), app({ id: "w2", name: "Two", folder_id: "f1" })]);
    renderApps();
    await selectAllThenOpenMove("Move 2 to folder…");
    await userEvent.click(screen.getByRole("menuitem", { name: "Uncategorized" }));
    await waitFor(() => expect(setFolderMock).toHaveBeenCalledTimes(2));
    expect(setFolderMock.mock.calls.map((c) => c[0])).toEqual(
      expect.arrayContaining([{ id: "w1", folder_id: null }, { id: "w2", folder_id: null }]),
    );
  });

  it("New folder… creates the folder, then moves the rows into it", async () => {
    folders = [{ id: "f1", name: "Finance" }];
    serve(MIXED);
    renderApps();
    await selectAllThenOpenMove("Move 2 to folder…");
    await userEvent.click(screen.getByRole("menuitem", { name: "New folder…" }));
    fireEvent.change(await screen.findByPlaceholderText("Folder name"), { target: { value: "Brand new" } });
    fireEvent.click(screen.getByRole("button", { name: "Move" }));
    await waitFor(() => expect(setFolderMock).toHaveBeenCalledTimes(2));
    // "f-new" is what the mocked create returns: the rows went to the folder just made.
    expect(setFolderMock.mock.calls.map((c) => c[0])).toEqual(
      expect.arrayContaining([{ id: "w1", folder_id: "f-new" }, { id: "w2", folder_id: "f-new" }]),
    );
  });

  it("offers a collaborator who may not create folders only the folders that exist", async () => {
    // They used to get a name box that created a folder for any new name, and a refusal.
    capsMock.mockReturnValue({ data: { can_create: false } });
    folders = [{ id: "f1", name: "Finance" }];
    serve([app({ id: "w1", name: "Shared with me", can_manage: false })]);
    renderApps();
    await selectAllThenOpenMove("Move 1 to folder…");
    expect(menuItems()).toEqual(["Finance", "Uncategorized"]);
    await userEvent.click(screen.getByRole("menuitem", { name: "Finance" }));
    await waitFor(() => expect(setFolderMock).toHaveBeenCalledWith({ id: "w1", folder_id: "f1" }));
  });

  it("offers no move at all when there is no folder and none can be made", async () => {
    capsMock.mockReturnValue({ data: { can_create: false } });
    folders = [];
    serve([app({ id: "w1", name: "Shared with me", can_manage: false })]);
    renderApps();
    await userEvent.click(screen.getByRole("button", { name: "Bulk select" }));
    await userEvent.click(screen.getByLabelText("Select all"));
    expect(screen.queryByRole("button", { name: /to folder/ })).toBeNull();
    expect(screen.getByRole("button", { name: "Move 1 to Trash" })).toBeTruthy();
  });

  it("says how many moved, and reports a refusal without claiming the rest", async () => {
    folders = [{ id: "f1", name: "Finance" }];
    serve(MIXED);
    setFolderMock.mockReset().mockResolvedValueOnce({}).mockRejectedValueOnce(new Error("forbidden"));
    renderApps();
    await selectAllThenOpenMove("Move 2 to folder…");
    await userEvent.click(screen.getByRole("menuitem", { name: "Finance" }));
    await waitFor(() => expect(toast.error).toHaveBeenCalledWith("forbidden"));
    expect(toast.success).toHaveBeenCalledWith("Moved 1 app");
  });

  it("does not claim success when every trash call is refused", async () => {
    serve(MIXED);
    m.trash.mockRejectedValue(new Error("forbidden"));
    renderApps();
    await userEvent.click(screen.getByRole("button", { name: "Bulk select" }));
    await userEvent.click(screen.getByLabelText("Select Writable one"));
    await userEvent.click(screen.getByRole("button", { name: "Move 1 to Trash" }));
    await waitFor(() => expect(toast.error).toHaveBeenCalledWith("forbidden"));
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("is not offered at all when no row on screen is writable", () => {
    serve([app({ id: "r1", name: "Read only", ...READ_ONLY })]);
    renderApps();
    expect(screen.queryByRole("button", { name: "Bulk select" })).toBeNull();
  });

  it("leaves a collapsed folder's rows out of Select all and out of the trash call", async () => {
    folders = [{ id: "f-rep", name: "Reporting" }, { id: "f-fin", name: "Finance" }];
    serve([
      app({ id: "hidden", name: "Hidden app", folder_id: "f-rep" }),
      app({ id: "shown", name: "Shown app", folder_id: "f-fin" }),
      app({ id: "shown-ro", name: "Shown read only", folder_id: "f-fin", ...READ_ONLY }),
    ]);
    renderApps();
    await userEvent.click(screen.getByRole("button", { name: "Toggle Reporting" }));
    expect(screen.queryByText("Hidden app")).toBeNull();

    await userEvent.click(screen.getByRole("button", { name: "Bulk select" }));
    await userEvent.click(screen.getByLabelText("Select all"));
    await userEvent.click(screen.getByRole("button", { name: "Move 1 to Trash" }));

    await waitFor(() => expect(m.trash).toHaveBeenCalledWith("shown"));
    expect(m.trash).toHaveBeenCalledTimes(1);
  });
});

/**
 * `+ App`, New folder and the folder section's own actions are not about one app, and the
 * response carries no workspace-level flag — so they follow "can this caller write anything
 * here", with the empty list (no row to ask) defaulting to offered.
 */
describe("Apps list — workspace-level affordances", () => {
  const cannotCreate = () => capsMock.mockReturnValue({ data: { can_create: false } });
  const folderHeader = () => screen.getByRole("button", { name: "Toggle Finance" }).closest("tr")!;

  it("come from the server's workspace answer, asked for this workspace", () => {
    serve([]);
    renderApps();
    expect(capsMock).toHaveBeenCalledWith("acme");
  });

  it("are offered to someone who may create, even with no writable row on screen", () => {
    folders = [{ id: "f1", name: "Finance" }];
    serve([app({ id: "r1", name: "Read only", folder_id: "f1", ...READ_ONLY })]);
    renderApps();
    expect(screen.getByRole("button", { name: "+ App" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "New folder" })).toBeTruthy();
    expect(within(folderHeader()).getAllByRole("button").length).toBeGreaterThan(1);
  });

  it("are withheld from a share-only collaborator, who keeps the menu of the row they can write", async () => {
    // The case the page used to guess wrong: a writable row was read as "may create", so this
    // collaborator was shown + App and New folder and refused on use.
    cannotCreate();
    folders = [{ id: "f1", name: "Finance" }];
    serve([app({ id: "w1", name: "Shared with me", folder_id: "f1", can_manage: false })]);
    renderApps();
    expect(screen.queryByRole("button", { name: "+ App" })).toBeNull();
    expect(screen.queryByRole("button", { name: "New folder" })).toBeNull();
    // The folder header still collapses; it has lost only its own actions menu.
    expect(within(folderHeader()).getAllByRole("button")).toHaveLength(1);
    await openMenu("Shared with me");
    expect(menuItems()).toContain("Rename…");
    // Moving to an EXISTING folder is theirs (it changes only their app); a new folder is not.
    expect(menuItems()).toContain('Move to "Finance"');
    expect(menuItems()).not.toContain("Move to new folder…");
  });

  it("are withheld on an empty list from someone who may not create", () => {
    // An empty list has no row to read a flag from; the old page defaulted to showing Create.
    cannotCreate();
    serve([]);
    renderApps();
    expect(screen.queryByRole("button", { name: "+ App" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Create your first app" })).toBeNull();
    expect(screen.queryByRole("button", { name: "New folder" })).toBeNull();
    expect(screen.getByText("No apps yet.")).toBeTruthy();
  });

  it("are offered on an empty list to someone who may create", () => {
    serve([]);
    renderApps();
    expect(screen.getByRole("button", { name: "+ App" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Create your first app" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "New folder" })).toBeTruthy();
  });

  it("are withheld until the server has answered, and if it cannot", () => {
    // No answer is not a yes: a control that appears and is then refused is the defect.
    serve([app({ id: "w1", name: "Writable" })]);
    capsMock.mockReturnValue({ data: undefined, isLoading: true });
    const first = renderApps();
    expect(screen.queryByRole("button", { name: "+ App" })).toBeNull();
    first.unmount();
    capsMock.mockReturnValue({ data: undefined, isError: true });
    renderApps();
    expect(screen.queryByRole("button", { name: "+ App" })).toBeNull();
    expect(screen.queryByRole("button", { name: "New folder" })).toBeNull();
  });

  it("survive a search that matches nothing", async () => {
    serve([app({ id: "w1", name: "Writable" })]);
    renderApps();
    await userEvent.type(screen.getByPlaceholderText("Search apps…"), "zzz");
    expect(screen.getByText("No apps match the current filters.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "+ App" })).toBeTruthy();
  });
});

describe("Apps list — dragging to a folder", () => {
  it("makes only writable rows draggable, and none in the trash", async () => {
    const pair = [app({ id: "w1", name: "Writable" }), app({ id: "r1", name: "Read only", ...READ_ONLY })];
    serve(pair, pair);
    renderApps();
    expect(rowOf("Writable").getAttribute("draggable")).toBe("true");
    expect(rowOf("Read only").getAttribute("draggable")).toBe("false");

    await userEvent.click(screen.getByRole("button", { name: "Trash" }));
    expect(rowOf("Writable").getAttribute("draggable")).toBe("false");
  });

  it("a collaborator who may not create can still drop their writable row on a folder", () => {
    // Moving an app is a mutation of THAT app (its own `can_write`, which is what makes the row
    // draggable at all), not of the workspace.
    capsMock.mockReturnValue({ data: { can_create: false } });
    folders = [{ id: "f1", name: "Finance" }];
    serve([app({ id: "w1", name: "Shared with me", can_manage: false })]);
    renderApps();
    const header = screen.getByRole("button", { name: "Toggle Finance" }).closest("tr")!;
    fireEvent.drop(header, { dataTransfer: { getData: () => "w1" } });
    expect(setFolderMock).toHaveBeenCalledWith({ id: "w1", folder_id: "f1" });
  });

  it("dropping a row on a folder header moves it there", () => {
    folders = [{ id: "f1", name: "Finance" }];
    serve([app({ id: "w1", name: "Writable" })]);
    renderApps();
    const header = screen.getByRole("button", { name: "Toggle Finance" }).closest("tr")!;
    fireEvent.drop(header, { dataTransfer: { getData: () => "w1" } });
    expect(setFolderMock).toHaveBeenCalledWith({ id: "w1", folder_id: "f1" });
  });
});

describe("Apps list — folders and the summary strip", () => {
  it("draws a flat table when the workspace has no folders", () => {
    serve([app({ id: "a1", name: "One" }), app({ id: "a2", name: "Two" })]);
    renderApps();
    expect(screen.queryByRole("button", { name: "Toggle Uncategorized" })).toBeNull();
  });

  it("heads the folderless bucket beside a real folder", () => {
    folders = [{ id: "f1", name: "Finance" }];
    serve([app({ id: "a1", name: "One", folder_id: "f1" }), app({ id: "a2", name: "Two" })]);
    renderApps();
    expect(screen.getByRole("button", { name: "Toggle Finance" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Toggle Uncategorized" })).toBeTruthy();
  });

  it("uses the singular for one app and one folder", () => {
    folders = [{ id: "f1", name: "Finance" }];
    serve([app({ id: "a1", name: "One" })]);
    renderApps();
    expect(screen.getByText("app")).toBeTruthy();
    expect(screen.getByText("folder")).toBeTruthy();
    expect(screen.queryByText("apps")).toBeNull();
  });
});
