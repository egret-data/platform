import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import { folderCollapseKey, UNCATEGORIZED_KEY } from "@/components/explore/folders";

type DatasetFixture = { id: string; name: string; description: string; database_id: string; kind: string; catalog: null; schema: string | null; table: string | null; column_count: number; updated_at: string; folder_id: string | null; bookmarked: boolean };
const DEFAULT_DATASETS: DatasetFixture[] = [
  { id: "d1", name: "Orders", description: "", database_id: "db1", kind: "virtual", catalog: null, schema: null, table: null, column_count: 3, updated_at: "2026-06-21T00:00:00Z", folder_id: null, bookmarked: false },
];
// Read inside the mock factory's closure, never at module scope, so a test can
// swap the fixture between renders.
const TWO_DATASETS: DatasetFixture[] = [
  DEFAULT_DATASETS[0],
  { ...DEFAULT_DATASETS[0], id: "d2", name: "Customers" },
];
let datasets: DatasetFixture[] = DEFAULT_DATASETS;
// Read inside the folders mock factory's closure, so a test can swap the
// folder list between renders — item 2 needs fixtures WITH and WITHOUT.
let folders: { id: string; name: string }[] = [];
let queryError: unknown = undefined;
vi.mock("@/api/datasets", () => ({
  useDatasetUsage: () => ({ data: [], isPending: false, isError: false }),
  useDuplicateDataset: () => ({ mutate: vi.fn(), isPending: false }),
  useDatasets: () => ({
    data: datasets,
    isLoading: false,
    isError: queryError !== undefined,
    error: queryError,
  }),
  useDataset: () => ({ data: undefined }),
  useDeleteDataset: () => ({ mutateAsync: vi.fn().mockResolvedValue(undefined) }),
  useDatasetListMutations: () => ({
    trash: { mutateAsync: vi.fn().mockResolvedValue({}) },
    restore: { mutateAsync: vi.fn().mockResolvedValue({}) },
    addBookmark: { mutateAsync: vi.fn().mockResolvedValue({}) },
    removeBookmark: { mutateAsync: vi.fn().mockResolvedValue({}) },
  }),
  useRefreshDataset: () => ({ isPending: false, mutateAsync: vi.fn().mockResolvedValue({}) }),
}));
vi.mock("@/api/sqllab", () => ({ useDatabases: () => ({ data: [] }) }));
vi.mock("@/api/folders", () => ({
  useFolders: () => ({ data: folders }),
  useSetFolder: () => ({ mutateAsync: vi.fn().mockResolvedValue({}) }),
  useFolderMutations: () => ({
    create: { mutateAsync: vi.fn().mockResolvedValue({ id: "f1", name: "F1" }) },
    rename: { mutateAsync: vi.fn().mockResolvedValue({}) },
    remove: { mutateAsync: vi.fn().mockResolvedValue(undefined) },
  }),
}));
vi.mock("@/components/dataset/CreateDatasetDialog", () => ({ CreateDatasetDialog: () => null }));

import Datasets from "../Datasets";

beforeEach(() => {
  vi.clearAllMocks();
  datasets = DEFAULT_DATASETS;
  folders = [];
  queryError = undefined;
  localStorage.clear();
});

function renderAt() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MemoryRouter initialEntries={["/explore/eng/datasets"]}>
          <Routes><Route path="/explore/:workspace/datasets" element={<Datasets />} /></Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

describe("Datasets on the Explore primitives", () => {
  // The confirmed defect: engaged state was carried only by
  // `variant="secondary"`, a visual class, so a screen reader announced
  // "Bookmarks" identically whether the filter was engaged or idle.
  it("reports the Bookmarks filter's engaged state with aria-pressed", async () => {
    renderAt();
    const btn = screen.getByRole("button", { name: "Bookmarks" });
    expect(btn).toHaveAttribute("aria-pressed", "false");
    await userEvent.click(btn);
    expect(btn).toHaveAttribute("aria-pressed", "true");
  });

  it("reports the Trash view's engaged state with aria-pressed", async () => {
    renderAt();
    const btn = screen.getByRole("button", { name: "Trash" });
    expect(btn).toHaveAttribute("aria-pressed", "false");
    await userEvent.click(btn);
    expect(btn).toHaveAttribute("aria-pressed", "true");
  });

  // Asserts the COUNT, not that one exists: the defect this guards is a layout
  // wrapper silently taking a route's only h1 with it.
  it("renders exactly one h1, and it is the page title", () => {
    renderAt();
    const h1s = screen.getAllByRole("heading", { level: 1 });
    expect(h1s).toHaveLength(1);
    expect(h1s[0]).toHaveTextContent("Datasets");
  });

  // `?? "Failed to load datasets."` renders a blank alert for a message-less
  // error, because "" is not nullish. `ListStates` uses `|| errorText`.
  it("falls back to the error text when the error carries no message", () => {
    queryError = new Error("");
    renderAt();
    expect(screen.getByText("Failed to load datasets.")).toBeInTheDocument();
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
describe("Datasets — the summary strip says `1 dataset`, not `1 datasets`", () => {
  it("uses the singular for one row and one folder", () => {
    // The folder chip is asserted here too, matching the other four pages.
    // Datasets passes its `folder`/`folders` pair inside a `folders.length ?`
    // spread, so a fixture with no folders never renders the chip and leaves
    // half this page's wiring — the half that is conditional, and so the more
    // fragile half — unpinned.
    datasets = [DEFAULT_DATASETS[0]];
    folders = [{ id: "f1", name: "Revenue" }];
    renderAt();
    expect(screen.getByText("dataset")).toBeInTheDocument();
    expect(screen.getByText("folder")).toBeInTheDocument();
    expect(screen.queryByText("datasets")).not.toBeInTheDocument();
    expect(screen.queryByText("folders")).not.toBeInTheDocument();
  });

  it("goes back to the plural at two", () => {
    datasets = [DEFAULT_DATASETS[0], { ...DEFAULT_DATASETS[0], id: "d2", name: "Customers" }];
    folders = [{ id: "f1", name: "Revenue" }, { id: "f2", name: "Ops" }];
    renderAt();
    expect(screen.getByText("datasets")).toBeInTheDocument();
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
describe("Datasets — no folders, no folderless header", () => {
  it("draws a flat table when the workspace has no folders", () => {
    datasets = TWO_DATASETS;
    folders = [];
    renderAt();
    expect(screen.getByText("Orders")).toBeInTheDocument();
    expect(screen.getByText("Customers")).toBeInTheDocument();
    // "Uncategorized" beside nothing labels the whole list after the one thing
    // it is not, and its collapse control's only effect is to hide everything.
    expect(screen.queryByRole("button", { name: "Toggle Uncategorized" })).not.toBeInTheDocument();
  });

  it("still heads the folderless bucket beside a real folder", () => {
    datasets = [{ ...TWO_DATASETS[0], folder_id: "f1" }, TWO_DATASETS[1]];
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
    localStorage.setItem(folderCollapseKey("eng", "datasets"), JSON.stringify([UNCATEGORIZED_KEY]));
    datasets = TWO_DATASETS;
    folders = [];
    const user = userEvent.setup();
    renderAt();
    expect(screen.getByText("Orders")).toBeInTheDocument();
    expect(screen.getByText("Customers")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    await user.click(screen.getByLabelText("Select all"));
    expect(screen.getByRole("button", { name: "Move 2 to Trash" })).toBeInTheDocument();
  });
});
