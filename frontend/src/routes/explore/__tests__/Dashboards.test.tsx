import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import Dashboards from "../Dashboards";
import { folderCollapseKey, UNCATEGORIZED_KEY } from "@/components/explore/folders";

type DashFixture = { id: string; name: string; description: string; created_by: null; card_count: number; updated_at: string; folder_id: string | null };
const DEFAULT_DASHBOARDS: DashFixture[] = [
  { id: "d1", name: "Ops", description: "", created_by: null, card_count: 3, updated_at: "2026-06-21T00:00:00Z", folder_id: null },
];
// Read inside the mock factories' closures, never at module scope, so a test
// can swap the fixture between renders.
const TWO_DASHBOARDS: DashFixture[] = [
  DEFAULT_DASHBOARDS[0],
  { ...DEFAULT_DASHBOARDS[0], id: "d2", name: "Finance" },
];
let dashboards: DashFixture[] = DEFAULT_DASHBOARDS;
let folders: { id: string; name: string }[] = [];
let queryError: unknown = undefined;
const trashMock = vi.fn().mockResolvedValue(undefined);

vi.mock("@/api/dashboards", () => ({
  useDashboards: () => ({
    data: dashboards,
    isLoading: false,
    isError: queryError !== undefined,
    error: queryError,
  }),
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
  queryError = undefined;
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
    // Confirmed first (KI-075).
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Move to Trash" }));

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
    // Confirmed first (KI-075).
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Move to Trash" }));

    await waitFor(() => expect(trashMock).toHaveBeenCalledWith("d-shown"));
    expect(trashMock).not.toHaveBeenCalledWith("d-hidden");
    expect(trashMock).toHaveBeenCalledTimes(1);
  });
});

describe("Dashboards on the Explore primitives", () => {
  // The §2.1 defect, still present before this task: `hasData` was checked
  // before `isError`, so a failed refetch holding cached rows rendered the
  // table and swallowed the error. The fixture below is exactly that state —
  // rows present AND the query in error.
  it("shows the error, not the cached rows, when a refetch fails", () => {
    queryError = new Error("Backend unreachable");
    renderAt();
    expect(screen.getByText("Backend unreachable")).toBeInTheDocument();
    expect(screen.queryByText("Ops")).not.toBeInTheDocument();
  });

  it("reports the Bookmarks filter's engaged state with aria-pressed", async () => {
    renderAt();
    const btn = screen.getByRole("button", { name: "Bookmarks" });
    expect(btn).toHaveAttribute("aria-pressed", "false");
    await userEvent.click(btn);
    expect(btn).toHaveAttribute("aria-pressed", "true");
  });

  it("renders exactly one h1, and it is the page title", () => {
    renderAt();
    const h1s = screen.getAllByRole("heading", { level: 1 });
    expect(h1s).toHaveLength(1);
    expect(h1s[0]).toHaveTextContent("Dashboards");
  });
});

/*
 * Item 1: the strip owns the singular. These pages set `singular` beside
 * `label`; the RULE (only the number one takes it) is tested once, on the
 * component, in `src/components/explore/__tests__/SummaryStrip.test.tsx`.
 * What is checked here is the WIRING — that this page passes the pair at all,
 * which is the half a component test cannot see and the half five pages had
 * missing.
 */
describe("Dashboards — the summary strip says `1 dashboard`, not `1 dashboards`", () => {
  it("uses the singular for one row and one folder", () => {
    dashboards = [DEFAULT_DASHBOARDS[0]];
    folders = [{ id: "f1", name: "Revenue" }];
    renderAt();
    expect(screen.getByText("dashboard")).toBeInTheDocument();
    expect(screen.getByText("folder")).toBeInTheDocument();
    expect(screen.queryByText("dashboards")).not.toBeInTheDocument();
    expect(screen.queryByText("folders")).not.toBeInTheDocument();
  });

  it("goes back to the plural at two", () => {
    dashboards = [DEFAULT_DASHBOARDS[0], { ...DEFAULT_DASHBOARDS[0], id: "d2", name: "Finance" }];
    folders = [{ id: "f1", name: "Revenue" }, { id: "f2", name: "Ops" }];
    renderAt();
    expect(screen.getByText("dashboards")).toBeInTheDocument();
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
describe("Dashboards — no folders, no folderless header", () => {
  it("draws a flat table when the workspace has no folders", () => {
    dashboards = TWO_DASHBOARDS;
    folders = [];
    renderAt();
    expect(screen.getByText("Ops")).toBeInTheDocument();
    expect(screen.getByText("Finance")).toBeInTheDocument();
    // "Uncategorized" beside nothing labels the whole list after the one thing
    // it is not, and its collapse control's only effect is to hide everything.
    expect(screen.queryByRole("button", { name: "Toggle Uncategorized" })).not.toBeInTheDocument();
  });

  it("still heads the folderless bucket beside a real folder", () => {
    dashboards = [{ ...TWO_DASHBOARDS[0], folder_id: "f1" }, TWO_DASHBOARDS[1]];
    folders = [{ id: "f1", name: "Revenue" }];
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
    localStorage.setItem(folderCollapseKey("eng", "dashboards"), JSON.stringify([UNCATEGORIZED_KEY]));
    dashboards = TWO_DASHBOARDS;
    folders = [];
    const user = userEvent.setup();
    renderAt();
    expect(screen.getByText("Ops")).toBeInTheDocument();
    expect(screen.getByText("Finance")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    await user.click(screen.getByLabelText("Select all"));
    expect(screen.getByRole("button", { name: "Move 2 to Trash" })).toBeInTheDocument();
  });
});
