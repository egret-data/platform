import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

const removeMock = vi.fn().mockResolvedValue(undefined);
const duplicateMock = vi.fn().mockResolvedValue({});
const trashMock = vi.fn().mockResolvedValue({});
// c1 is newest → first row under the default "updated_at desc" sort.
const charts = [
  { id: "c1", dataset_id: "d1", name: "Revenue bar", description: "", viz_type: "bar", params: {}, created_by: null, updated_at: "2026-06-21T00:00:00Z", bookmarked: false },
  { id: "c2", dataset_id: "d1", name: "Orders line", description: "", viz_type: "line", params: {}, created_by: null, updated_at: "2026-06-20T00:00:00Z", bookmarked: false },
];
vi.mock("@/api/charts", () => ({
  useCharts: () => ({ data: charts, isLoading: false }),
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
  useFolders: () => ({ data: [] }),
  useSetFolder: () => ({ mutateAsync: vi.fn().mockResolvedValue({}) }),
  useFolderMutations: () => ({
    create: { mutateAsync: vi.fn().mockResolvedValue({ id: "f1", name: "F1" }) },
    rename: { mutateAsync: vi.fn().mockResolvedValue({}) },
    remove: { mutateAsync: vi.fn().mockResolvedValue(undefined) },
  }),
}));

import Charts from "../Charts";

function renderAt() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/charts"]}>
        <Routes><Route path="/explore/:workspace/charts" element={<Charts />} /></Routes>
      </MemoryRouter>
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
