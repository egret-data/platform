import { describe, it, expect, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";

const trashMock = vi.fn().mockResolvedValue({});
const delMock = vi.fn().mockResolvedValue(undefined);
// d1 is newest → first row under the default "updated_at desc" sort.
const datasets = [
  { id: "d1", name: "Orders", description: "", database_id: "db1", kind: "virtual", catalog: null, schema: null, table: null, column_count: 3, updated_at: "2026-06-21T00:00:00Z", folder_id: null, bookmarked: false },
  { id: "d2", name: "Customers", description: "", database_id: "db1", kind: "physical", catalog: null, schema: "public", table: "customers", column_count: 5, updated_at: "2026-06-20T00:00:00Z", folder_id: null, bookmarked: false },
];
vi.mock("@/api/datasets", () => ({
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
    expect(trashMock).toHaveBeenCalledWith("d1");
  });
});
