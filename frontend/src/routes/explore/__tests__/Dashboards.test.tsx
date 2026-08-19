import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import Dashboards from "../Dashboards";

type DashFixture = { id: string; name: string; description: string; created_by: null; card_count: number; updated_at: string; folder_id: string | null };
const DEFAULT_DASHBOARDS: DashFixture[] = [
  { id: "d1", name: "Ops", description: "", created_by: null, card_count: 3, updated_at: "2026-06-21T00:00:00Z", folder_id: null },
];
// Read inside the mock factories' closures, never at module scope, so a test
// can swap the fixture between renders.
let dashboards: DashFixture[] = DEFAULT_DASHBOARDS;
let folders: { id: string; name: string }[] = [];
const trashMock = vi.fn().mockResolvedValue(undefined);

vi.mock("@/api/dashboards", () => ({
  useDashboards: () => ({ data: dashboards, isLoading: false }),
  useDashboardMutations: () => ({
    create: { mutateAsync: vi.fn().mockResolvedValue({ id: "x" }), isPending: false },
    remove: { mutateAsync: vi.fn().mockResolvedValue(undefined) },
    duplicate: { mutateAsync: vi.fn().mockResolvedValue({ id: "x" }) },
    trash: { mutateAsync: trashMock },
    restore: { mutateAsync: vi.fn().mockResolvedValue(undefined) },
    addBookmark: { mutateAsync: vi.fn().mockResolvedValue(undefined) },
    removeBookmark: { mutateAsync: vi.fn().mockResolvedValue(undefined) },
  }),
}));
vi.mock("@/api/folders", () => ({
  useFolders: () => ({ data: folders }),
  useSetFolder: () => ({ mutateAsync: vi.fn().mockResolvedValue({}) }),
  useFolderMutations: () => ({
    create: { mutateAsync: vi.fn().mockResolvedValue({ id: "f1", name: "F1" }) },
    rename: { mutateAsync: vi.fn().mockResolvedValue({}) },
    remove: { mutateAsync: vi.fn().mockResolvedValue(undefined) },
  }),
}));

function renderAt() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MemoryRouter initialEntries={["/explore/eng/dashboards"]}>
          <Routes><Route path="/explore/:workspace/dashboards" element={<Dashboards />} /></Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  dashboards = DEFAULT_DASHBOARDS;
  folders = [];
  // `useCollapsed` persists collapse state in localStorage under
  // `folders:eng:dashboards`; without this a test that collapses a folder
  // leaks that collapse into every later test in the file.
  localStorage.clear();
});

describe("Dashboards list", () => {
  it("lists dashboards with their card counts", () => {
    renderAt();
    expect(screen.getByText("Ops")).toBeInTheDocument();
    // Card-count cell — exact match, so a relative-time like "3 weeks ago"
    // (from updated_at) doesn't also match a loose `includes("3")`.
    expect(screen.getByText("3")).toBeInTheDocument();
  });
});

/**
 * Bulk select must only ever reach rows that are ON SCREEN. `useBulkSelect` was
 * fed the pre-grouping `rows`, so "Select all" swept in the rows of a collapsed
 * folder and "Move n to Trash" trashed things the user had never seen.
 *
 * The fixture puts one dashboard in each of TWO folders and collapses only one.
 * A fixture with everything inside the collapsed folder cannot tell a correct
 * fix from one that selects nothing at all — both end at an empty selection.
 */
describe("Dashboards — bulk select only reaches rows on screen", () => {
  const IN_TWO_FOLDERS: DashFixture[] = [
    { id: "d-hidden", name: "Hidden dash", description: "", created_by: null, card_count: 1, updated_at: "2026-06-21T00:00:00Z", folder_id: "f-rep" },
    { id: "d-shown", name: "Shown dash", description: "", created_by: null, card_count: 2, updated_at: "2026-06-20T00:00:00Z", folder_id: "f-fin" },
  ];
  const TWO_FOLDERS = [{ id: "f-rep", name: "Reporting" }, { id: "f-fin", name: "Finance" }];

  it("leaves a collapsed folder's rows out of Select all and out of the trash call", async () => {
    dashboards = IN_TWO_FOLDERS;
    folders = TWO_FOLDERS;
    const user = userEvent.setup();
    renderAt();
    // Both folders start expanded, so both rows are genuinely on screen first.
    expect(screen.getByText("Hidden dash")).toBeInTheDocument();
    expect(screen.getByText("Shown dash")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Toggle Reporting" }));
    expect(screen.queryByText("Hidden dash")).toBeNull();
    // Finance is still open — the fix has something left to select.
    expect(screen.getByText("Shown dash")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    await user.click(screen.getByLabelText("Select all"));

    expect(screen.getByRole("button", { name: "Move 1 to Trash" })).toBeInTheDocument();
    expect(screen.queryByLabelText("Select Hidden dash")).toBeNull();

    await user.click(screen.getByRole("button", { name: "Move 1 to Trash" }));

    // The ids, not merely that it was called: `toHaveBeenCalled()` is satisfied
    // by trashing exactly the wrong dashboard, which is the whole defect.
    await waitFor(() => expect(trashMock).toHaveBeenCalledWith("d-shown"));
    expect(trashMock).not.toHaveBeenCalledWith("d-hidden");
    expect(trashMock).toHaveBeenCalledTimes(1);
  });

  it("drops a row from the selection when its folder collapses after it was picked", async () => {
    dashboards = IN_TWO_FOLDERS;
    folders = TWO_FOLDERS;
    const user = userEvent.setup();
    renderAt();

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    // SELECT first, while it is legitimately on screen…
    await user.click(screen.getByLabelText("Select Hidden dash"));
    await user.click(screen.getByLabelText("Select Shown dash"));
    expect(screen.getByRole("button", { name: "Move 2 to Trash" })).toBeInTheDocument();

    // …then HIDE it. `toggleAll` is not involved: the id is already in
    // `selected`, and only `selectedIds`' re-filter can take it back out.
    await user.click(screen.getByRole("button", { name: "Toggle Reporting" }));
    expect(screen.getByRole("button", { name: "Move 1 to Trash" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Move 1 to Trash" }));

    await waitFor(() => expect(trashMock).toHaveBeenCalledWith("d-shown"));
    expect(trashMock).not.toHaveBeenCalledWith("d-hidden");
    expect(trashMock).toHaveBeenCalledTimes(1);
  });
});
