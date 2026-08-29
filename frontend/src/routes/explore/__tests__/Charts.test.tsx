import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";

const removeMock = vi.fn().mockResolvedValue(undefined);
const duplicateMock = vi.fn().mockResolvedValue({});
const trashMock = vi.fn().mockResolvedValue({});
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
    remove: { mutateAsync: vi.fn().mockResolvedValue(undefined) },
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
