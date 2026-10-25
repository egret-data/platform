import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import { folderCollapseKey, UNCATEGORIZED_KEY } from "@/components/explore/folders";

const removeMock = vi.fn().mockResolvedValue(undefined);
const duplicateMock = vi.fn().mockResolvedValue({});
const trashMock = vi.fn().mockResolvedValue({});
const folderRemoveMock = vi.fn().mockResolvedValue(undefined);
type ChartFixture = { id: string; dataset_id: string; name: string; description: string; viz_type: string; params: object; created_by: null; updated_at: string; folder_id: string | null; bookmarked: boolean };
// c1 is newest → first row under the default "updated_at desc" sort.
const DEFAULT_CHARTS: ChartFixture[] = [
  { id: "c1", dataset_id: "d1", name: "Revenue bar", description: "", viz_type: "bar", params: {}, created_by: null, updated_at: "2026-06-21T00:00:00Z", folder_id: null, bookmarked: false },
  { id: "c2", dataset_id: "d1", name: "Orders line", description: "", viz_type: "line", params: {}, created_by: null, updated_at: "2026-06-20T00:00:00Z", folder_id: null, bookmarked: false },
];
// Read inside the mock factories' closures, never at module scope, so a test
// can swap the fixture between renders.
let charts: ChartFixture[] = DEFAULT_CHARTS;
let folders: { id: string; name: string }[] = [];
let queryError: unknown = undefined;
vi.mock("@/api/charts", () => ({
  useCharts: () => ({
    data: charts,
    isLoading: false,
    isError: queryError !== undefined,
    error: queryError,
  }),
  useChartMutations: () => ({
    remove: { mutateAsync: removeMock },
    duplicate: { mutateAsync: duplicateMock },
    trash: { mutateAsync: trashMock },
    restore: { mutateAsync: vi.fn().mockResolvedValue({}) },
    addBookmark: { mutateAsync: vi.fn().mockResolvedValue({}) },
    removeBookmark: { mutateAsync: vi.fn().mockResolvedValue({}) },
  }),
}));
vi.mock("@/api/folders", () => ({
  useFolders: () => ({ data: folders }),
  useSetFolder: () => ({ mutateAsync: vi.fn().mockResolvedValue({}) }),
  useFolderMutations: () => ({
    create: { mutateAsync: vi.fn().mockResolvedValue({ id: "f1", name: "F1" }) },
    rename: { mutateAsync: vi.fn().mockResolvedValue({}) },
    remove: { mutateAsync: folderRemoveMock },
  }),
}));

import Charts from "../Charts";

beforeEach(() => {
  vi.clearAllMocks();
  charts = DEFAULT_CHARTS;
  folders = [];
  queryError = undefined;
  // `useCollapsed` persists collapse state in localStorage under
  // `folders:eng:charts`; without this a test that collapses a folder leaks
  // that collapse into every later test in the file.
  localStorage.clear();
});

function renderAt() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MemoryRouter initialEntries={["/explore/eng/charts"]}>
          <Routes><Route path="/explore/:workspace/charts" element={<Charts />} /></Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

describe("Charts list", () => {
  it("renders chart rows with their type labels", () => {
    renderAt();
    expect(screen.getByText("Revenue bar")).toBeInTheDocument();
    expect(screen.getByText("Orders line")).toBeInTheDocument();
    expect(screen.getByText("Bar")).toBeInTheDocument();
  });
  it("search filters by name", () => {
    renderAt();
    fireEvent.change(screen.getByPlaceholderText("Search charts…"), { target: { value: "orders" } });
    expect(screen.queryByText("Revenue bar")).toBeNull();
    expect(screen.getByText("Orders line")).toBeInTheDocument();
  });
  it("Move to Trash trashes the first row", async () => {
    trashMock.mockClear();
    const user = userEvent.setup();
    renderAt();
    await user.click(screen.getAllByRole("button", { name: "Actions" })[0]);
    await user.click(await screen.findByText("Move to Trash"));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Move to Trash" }));
    expect(trashMock).toHaveBeenCalledWith("c1");
  });
  it("Duplicate fires the duplicate mutation for the first row", async () => {
    duplicateMock.mockClear();
    const user = userEvent.setup();
    renderAt();
    await user.click(screen.getAllByRole("button", { name: "Actions" })[0]);
    await user.click(await screen.findByText("Duplicate"));
    expect(duplicateMock).toHaveBeenCalledWith("c1");
  });
  it("Bulk select toggle reveals a Select all checkbox", async () => {
    const user = userEvent.setup();
    renderAt();
    expect(screen.queryByLabelText("Select all")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    expect(screen.getByLabelText("Select all")).toBeInTheDocument();
  });
});

/**
 * Bulk select must only ever reach rows that are ON SCREEN. `useBulkSelect` was
 * fed the pre-grouping `rows`, so "Select all" swept in the rows of a collapsed
 * folder and "Move n to Trash" trashed things the user had never seen.
 *
 * The fixture puts one chart in each of TWO folders and collapses only one. A
 * fixture with everything inside the collapsed folder cannot tell a correct fix
 * from one that selects nothing at all — both end at an empty selection.
 */
describe("Charts — bulk select only reaches rows on screen", () => {
  const IN_TWO_FOLDERS: ChartFixture[] = [
    { id: "c-hidden", dataset_id: "d1", name: "Hidden chart", description: "", viz_type: "bar", params: {}, created_by: null, updated_at: "2026-06-21T00:00:00Z", folder_id: "f-rep", bookmarked: false },
    { id: "c-shown", dataset_id: "d1", name: "Shown chart", description: "", viz_type: "line", params: {}, created_by: null, updated_at: "2026-06-20T00:00:00Z", folder_id: "f-fin", bookmarked: false },
  ];
  const TWO_FOLDERS = [{ id: "f-rep", name: "Reporting" }, { id: "f-fin", name: "Finance" }];

  it("leaves a collapsed folder's rows out of Select all and out of the trash call", async () => {
    charts = IN_TWO_FOLDERS;
    folders = TWO_FOLDERS;
    const user = userEvent.setup();
    renderAt();
    // Both folders start expanded, so both rows are genuinely on screen first.
    expect(screen.getByText("Hidden chart")).toBeInTheDocument();
    expect(screen.getByText("Shown chart")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Toggle Reporting" }));
    expect(screen.queryByText("Hidden chart")).toBeNull();
    // Finance is still open — the fix has something left to select.
    expect(screen.getByText("Shown chart")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    await user.click(screen.getByLabelText("Select all"));

    // The count in the button label is the user-visible half of the claim.
    expect(screen.getByRole("button", { name: "Move 1 to Trash" })).toBeInTheDocument();
    expect(screen.queryByLabelText("Select Hidden chart")).toBeNull();

    await user.click(screen.getByRole("button", { name: "Move 1 to Trash" }));
    // Confirmed first (KI-281).
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Move to Trash" }));

    // The ids, not merely that it was called: `toHaveBeenCalled()` is satisfied
    // by trashing exactly the wrong chart, which is the whole defect.
    await vi.waitFor(() => expect(trashMock).toHaveBeenCalledWith("c-shown"));
    expect(trashMock).not.toHaveBeenCalledWith("c-hidden");
    expect(trashMock).toHaveBeenCalledTimes(1);
  });

  it("drops a row from the selection when its folder collapses after it was picked", async () => {
    charts = IN_TWO_FOLDERS;
    folders = TWO_FOLDERS;
    const user = userEvent.setup();
    renderAt();

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    // SELECT first, while it is legitimately on screen…
    await user.click(screen.getByLabelText("Select Hidden chart"));
    await user.click(screen.getByLabelText("Select Shown chart"));
    expect(screen.getByRole("button", { name: "Move 2 to Trash" })).toBeInTheDocument();

    // …then HIDE it. `toggleAll` is not involved: the id is already in
    // `selected`, and only `selectedIds`' re-filter can take it back out.
    await user.click(screen.getByRole("button", { name: "Toggle Reporting" }));
    expect(screen.getByRole("button", { name: "Move 1 to Trash" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Move 1 to Trash" }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Move to Trash" }));

    await vi.waitFor(() => expect(trashMock).toHaveBeenCalledWith("c-shown"));
    expect(trashMock).not.toHaveBeenCalledWith("c-hidden");
    expect(trashMock).toHaveBeenCalledTimes(1);
  });
});

describe("Charts on the Explore primitives", () => {
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
    expect(h1s[0]).toHaveTextContent("Charts");
  });

  it("falls back to the error text when the error carries no message", () => {
    queryError = new Error("");
    renderAt();
    expect(screen.getByText("Failed to load charts.")).toBeInTheDocument();
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
describe("Charts — the summary strip says `1 chart`, not `1 charts`", () => {
  it("uses the singular for one row and one folder", () => {
    charts = [DEFAULT_CHARTS[0]];
    folders = [{ id: "f1", name: "Revenue" }];
    renderAt();
    expect(screen.getByText("chart")).toBeInTheDocument();
    expect(screen.getByText("folder")).toBeInTheDocument();
    expect(screen.queryByText("charts")).not.toBeInTheDocument();
    expect(screen.queryByText("folders")).not.toBeInTheDocument();
  });

  it("goes back to the plural at two", () => {
    charts = DEFAULT_CHARTS;
    folders = [{ id: "f1", name: "Revenue" }, { id: "f2", name: "Ops" }];
    renderAt();
    expect(screen.getByText("charts")).toBeInTheDocument();
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
describe("Charts — no folders, no folderless header", () => {
  it("draws a flat table when the workspace has no folders", () => {
    charts = DEFAULT_CHARTS;
    folders = [];
    renderAt();
    expect(screen.getByText("Revenue bar")).toBeInTheDocument();
    expect(screen.getByText("Orders line")).toBeInTheDocument();
    // "Uncategorized" beside nothing labels the whole list after the one thing
    // it is not, and its collapse control's only effect is to hide everything.
    expect(screen.queryByRole("button", { name: "Toggle Uncategorized" })).not.toBeInTheDocument();
  });

  it("still heads the folderless bucket beside a real folder", () => {
    charts = [{ ...DEFAULT_CHARTS[0], folder_id: "f1" }, DEFAULT_CHARTS[1]];
    folders = [{ id: "f1", name: "Revenue" }];
    renderAt();
    expect(screen.getByRole("button", { name: "Toggle Revenue" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Toggle Uncategorized" })).toBeInTheDocument();
  });

  /*
   * Item 3's real guard, and it has to live on a PAGE rather than on
   * `FolderHeaderRow` itself.
   *
   * The component emits exactly one cell, so "the folder header is the only
   * rowheader in the table" is not a claim its own test can fail — the fixture
   * there contains no data rows for a stray role to appear on, and the
   * assertion would only break if `FolderHeaderRow` emitted two rowheaders,
   * which it structurally cannot. Here the table has both: two folder headers
   * and four data rows, each with its own cells.
   *
   * The scenario this exists to catch is a data cell in `renderRow` picking up
   * `role="rowheader"`. That makes the folder header no longer distinguishable
   * from the rows it heads, which is precisely the confusion the role was added
   * to remove — and every other test in this file stays green through it.
   */
  it("makes the folder headers the ONLY rowheaders — the data rows stay cells", () => {
    charts = [{ ...DEFAULT_CHARTS[0], folder_id: "f1" }, DEFAULT_CHARTS[1]];
    folders = [{ id: "f1", name: "Revenue" }];
    renderAt();

    // Two groups are rendered: the named folder and the folderless bucket.
    const rowheaders = screen.getAllByRole("rowheader");
    expect(rowheaders).toHaveLength(2);
    expect(rowheaders.map((h) => h.textContent)).toEqual([
      expect.stringContaining("Revenue"),
      expect.stringContaining("Uncategorized"),
    ]);

    // And the chart rows are reachable as ordinary cells. Named through the
    // real role computation, so a data cell that gained `role="rowheader"`
    // would leave `getByRole("cell")` unable to find it AND push the count
    // above two — the assertion fails from both directions.
    const cells = screen.getAllByRole("cell");
    expect(cells.some((c) => c.textContent?.includes("Revenue bar"))).toBe(true);
    expect(cells.some((c) => c.textContent?.includes("Orders line"))).toBe(true);
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
    localStorage.setItem(folderCollapseKey("eng", "charts"), JSON.stringify([UNCATEGORIZED_KEY]));
    charts = DEFAULT_CHARTS;
    folders = [];
    const user = userEvent.setup();
    renderAt();
    expect(screen.getByText("Revenue bar")).toBeInTheDocument();
    expect(screen.getByText("Orders line")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    await user.click(screen.getByLabelText("Select all"));
    expect(screen.getByRole("button", { name: "Move 2 to Trash" })).toBeInTheDocument();
  });
});

// KI-055: the Trash is a flat list with no folders on show, so its summary counts none — the same
// rule the Dashboards list follows (KI-074).
describe("Charts Trash summary", () => {
  it("counts folders on the list and none in the Trash", async () => {
    folders = [{ id: "f1", name: "Finance" }];
    renderAt();
    expect(screen.getByText("folder")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Trash" }));
    await screen.findByRole("heading", { name: "Trash" });
    expect(screen.queryByText("folder")).toBeNull();
    expect(screen.queryByText("folders")).toBeNull();
  });

  // KI-291: nothing is created in the Trash, as on Dashboards (KI-074) and Datasets.
  it("offers + Chart on the list and not in the Trash", async () => {
    renderAt();
    expect(screen.getByRole("link", { name: "+ Chart" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Trash" }));
    await screen.findByRole("heading", { name: "Trash" });
    expect(screen.queryByRole("link", { name: "+ Chart" })).toBeNull();
  });
});

// KI-281: the Charts list trashed rows and deleted folders at once, while the Dashboards list asks
// first (KI-075).
describe("Charts asks before trashing or deleting a folder (KI-281)", () => {
  it("a row's Move to Trash asks first, names the chart, and trashes only on confirm", async () => {
    const user = userEvent.setup();
    renderAt();
    await user.click(screen.getAllByRole("button", { name: "Actions" })[0]);
    await user.click(await screen.findByText("Move to Trash"));
    const dialog = await screen.findByRole("alertdialog");
    expect(trashMock).not.toHaveBeenCalled();
    expect(dialog.textContent).toContain('"Revenue bar" moves to the Trash');
    await user.click(within(dialog).getByRole("button", { name: "Move to Trash" }));
    await vi.waitFor(() => expect(trashMock).toHaveBeenCalledWith("c1"));
  });

  it("Cancel leaves the chart where it is", async () => {
    const user = userEvent.setup();
    renderAt();
    await user.click(screen.getAllByRole("button", { name: "Actions" })[0]);
    await user.click(await screen.findByText("Move to Trash"));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(trashMock).not.toHaveBeenCalled();
  });

  it("the bulk Move n to Trash asks first and counts the charts", async () => {
    const user = userEvent.setup();
    renderAt();
    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    await user.click(screen.getByLabelText("Select all"));
    await user.click(screen.getByRole("button", { name: "Move 2 to Trash" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(trashMock).not.toHaveBeenCalled();
    expect(dialog.textContent).toContain("2 charts move to the Trash");
    await user.click(within(dialog).getByRole("button", { name: "Move to Trash" }));
    await vi.waitFor(() => expect(trashMock).toHaveBeenCalledTimes(2));
  });

  it("a folder's Delete asks first, says where its charts go, and deletes only on confirm", async () => {
    charts = [{ ...DEFAULT_CHARTS[0], folder_id: "f1" }, DEFAULT_CHARTS[1]];
    folders = [{ id: "f1", name: "F1" }];
    const user = userEvent.setup();
    renderAt();
    await user.click(screen.getByRole("button", { name: "Folder F1 actions" }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(folderRemoveMock).not.toHaveBeenCalled();
    expect(dialog.textContent).toContain("The chart in it moves to Uncategorized");
    await user.click(within(dialog).getByRole("button", { name: "Delete folder" }));
    await vi.waitFor(() => expect(folderRemoveMock).toHaveBeenCalledWith("f1"));
  });
});
