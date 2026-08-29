import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";

type DatasetFixture = { id: string; name: string; description: string; database_id: string; kind: string; catalog: null; schema: string | null; table: string | null; column_count: number; updated_at: string; folder_id: string | null; bookmarked: boolean };
const DEFAULT_DATASETS: DatasetFixture[] = [
  { id: "d1", name: "Orders", description: "", database_id: "db1", kind: "virtual", catalog: null, schema: null, table: null, column_count: 3, updated_at: "2026-06-21T00:00:00Z", folder_id: null, bookmarked: false },
];
// Read inside the mock factory's closure, never at module scope, so a test can
// swap the fixture between renders.
let datasets: DatasetFixture[] = DEFAULT_DATASETS;
let queryError: unknown = undefined;
vi.mock("@/api/datasets", () => ({
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
  useFolders: () => ({ data: [] }),
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
  it("uses the singular for one row", () => {
    datasets = [DEFAULT_DATASETS[0]];
    renderAt();
    expect(screen.getByText("dataset")).toBeInTheDocument();
    expect(screen.queryByText("datasets")).not.toBeInTheDocument();
  });

  it("goes back to the plural at two", () => {
    datasets = [DEFAULT_DATASETS[0], { ...DEFAULT_DATASETS[0], id: "d2", name: "Customers" }];
    renderAt();
    expect(screen.getByText("datasets")).toBeInTheDocument();
  });
});
