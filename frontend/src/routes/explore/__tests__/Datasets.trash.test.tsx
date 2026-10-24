import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";

const trashMock = vi.fn().mockResolvedValue({});
const delMock = vi.fn().mockResolvedValue(undefined);
type DatasetFixture = { id: string; name: string; description: string; database_id: string; kind: string; catalog: null; schema: string | null; table: string | null; column_count: number; updated_at: string; folder_id: string | null; bookmarked: boolean };
// d1 is newest → first row under the default "updated_at desc" sort.
const DEFAULT_DATASETS: DatasetFixture[] = [
  { id: "d1", name: "Orders", description: "", database_id: "db1", kind: "virtual", catalog: null, schema: null, table: null, column_count: 3, updated_at: "2026-06-21T00:00:00Z", folder_id: null, bookmarked: false },
  { id: "d2", name: "Customers", description: "", database_id: "db1", kind: "physical", catalog: null, schema: "public", table: "customers", column_count: 5, updated_at: "2026-06-20T00:00:00Z", folder_id: null, bookmarked: false },
];
// Read inside the mock factories' closures, never at module scope, so a test
// can swap the fixture between renders.
let datasets: DatasetFixture[] = DEFAULT_DATASETS;
let folders: { id: string; name: string }[] = [];
vi.mock("@/api/datasets", () => ({
  useDatasetUsage: () => ({ data: [{ id: "c1", name: "Uses it", viz_type: "bar", trashed: false }], isPending: false, isError: false }),
  useDuplicateDataset: () => ({ mutate: vi.fn(), isPending: false }),
  useDatasets: () => ({ data: datasets, isLoading: false }),
  useDataset: () => ({ data: undefined }),
  useDeleteDataset: () => ({ mutateAsync: delMock }),
  useDatasetListMutations: () => ({
    trash: { mutateAsync: trashMock },
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

beforeEach(() => {
  vi.clearAllMocks();
  datasets = DEFAULT_DATASETS;
  folders = [];
  // `useCollapsed` persists collapse state in localStorage under
  // `folders:eng:datasets`; without this a test that collapses a folder leaks
  // that collapse into every later test in the file.
  localStorage.clear();
});

describe("Datasets trash", () => {
  it("renders dataset rows", () => {
    renderAt();
    expect(screen.getByText("Orders")).toBeInTheDocument();
    expect(screen.getByText("Customers")).toBeInTheDocument();
  });
  it("Move to Trash trashes the first row", async () => {
    trashMock.mockClear();
    const user = userEvent.setup();
    renderAt();
    await user.click(screen.getAllByRole("button", { name: "Actions" })[0]);
    await user.click(await screen.findByText("Move to Trash"));
    // KI-111: it asks first, saying the charts keep working until the dataset is deleted.
    expect(trashMock).not.toHaveBeenCalled();
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText(/1 chart uses this dataset and will keep working until the dataset is deleted/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Move to Trash" }));
    expect(trashMock).toHaveBeenCalledWith("d1");
  });
});

/**
 * Bulk select must only ever reach rows that are ON SCREEN. `useBulkSelect` was
 * fed the pre-grouping `rows`, so "Select all" swept in the rows of a collapsed
 * folder and "Move n to Trash" trashed things the user had never seen.
 *
 * The fixture puts one dataset in each of TWO folders and collapses only one. A
 * fixture with everything inside the collapsed folder cannot tell a correct fix
 * from one that selects nothing at all — both end at an empty selection.
 */
describe("Datasets — bulk select only reaches rows on screen", () => {
  const IN_TWO_FOLDERS: DatasetFixture[] = [
    { id: "ds-hidden", name: "Hidden set", description: "", database_id: "db1", kind: "virtual", catalog: null, schema: null, table: null, column_count: 3, updated_at: "2026-06-21T00:00:00Z", folder_id: "f-rep", bookmarked: false },
    { id: "ds-shown", name: "Shown set", description: "", database_id: "db1", kind: "virtual", catalog: null, schema: null, table: null, column_count: 4, updated_at: "2026-06-20T00:00:00Z", folder_id: "f-fin", bookmarked: false },
  ];
  const TWO_FOLDERS = [{ id: "f-rep", name: "Reporting" }, { id: "f-fin", name: "Finance" }];

  it("leaves a collapsed folder's rows out of Select all and out of the trash call", async () => {
    datasets = IN_TWO_FOLDERS;
    folders = TWO_FOLDERS;
    const user = userEvent.setup();
    renderAt();
    // Both folders start expanded, so both rows are genuinely on screen first.
    expect(screen.getByText("Hidden set")).toBeInTheDocument();
    expect(screen.getByText("Shown set")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Toggle Reporting" }));
    expect(screen.queryByText("Hidden set")).toBeNull();
    // Finance is still open — the fix has something left to select.
    expect(screen.getByText("Shown set")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    await user.click(screen.getByLabelText("Select all"));

    expect(screen.getByRole("button", { name: "Move 1 to Trash" })).toBeInTheDocument();
    expect(screen.queryByLabelText("Select Hidden set")).toBeNull();

    await user.click(screen.getByRole("button", { name: "Move 1 to Trash" }));

    // The ids, not merely that it was called: `toHaveBeenCalled()` is satisfied
    // by trashing exactly the wrong dataset, which is the whole defect.
    await waitFor(() => expect(trashMock).toHaveBeenCalledWith("ds-shown"));
    expect(trashMock).not.toHaveBeenCalledWith("ds-hidden");
    expect(trashMock).toHaveBeenCalledTimes(1);
  });

  it("drops a row from the selection when its folder collapses after it was picked", async () => {
    datasets = IN_TWO_FOLDERS;
    folders = TWO_FOLDERS;
    const user = userEvent.setup();
    renderAt();

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    // SELECT first, while it is legitimately on screen…
    await user.click(screen.getByLabelText("Select Hidden set"));
    await user.click(screen.getByLabelText("Select Shown set"));
    expect(screen.getByRole("button", { name: "Move 2 to Trash" })).toBeInTheDocument();

    // …then HIDE it. `toggleAll` is not involved: the id is already in
    // `selected`, and only `selectedIds`' re-filter can take it back out.
    await user.click(screen.getByRole("button", { name: "Toggle Reporting" }));
    expect(screen.getByRole("button", { name: "Move 1 to Trash" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Move 1 to Trash" }));

    await waitFor(() => expect(trashMock).toHaveBeenCalledWith("ds-shown"));
    expect(trashMock).not.toHaveBeenCalledWith("ds-hidden");
    expect(trashMock).toHaveBeenCalledTimes(1);
  });
});

// KI-055: the Datasets Trash read "1 dataset · 2 folders" although the Trash shows no folders.
describe("Datasets Trash summary", () => {
  it("counts folders on the list and none in the Trash", async () => {
    folders = [{ id: "f1", name: "Finance" }, { id: "f2", name: "Ops" }];
    renderAt();
    expect(screen.getByText("folders")).toBeInTheDocument();
    await userEvent.setup().click(screen.getByRole("button", { name: "Trash" }));
    await screen.findByRole("heading", { name: "Trash" });
    expect(screen.queryByText("folders")).toBeNull();
    expect(screen.queryByText("folder")).toBeNull();
  });
});
