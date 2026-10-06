import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import Dashboards from "../Dashboards";

const rows = [
  { id: "d1", name: "Ops", description: "", created_by: null, card_count: 3, updated_at: "2026-06-21T00:00:00Z", can_manage: true },
  { id: "d2", name: "Sales", description: "", created_by: null, card_count: 1, updated_at: "2026-06-22T00:00:00Z", can_manage: true },
];
const setFolder = vi.fn();
const createFolder = vi.fn();

vi.mock("@/api/dashboards", () => ({
  useDashboards: () => ({ data: rows, isLoading: false }),
  useDashboardMutations: () => ({
    create: { mutateAsync: vi.fn(), isPending: false },
    remove: { mutateAsync: vi.fn() },
    duplicate: { mutateAsync: vi.fn() },
    trash: { mutateAsync: vi.fn() },
    restore: { mutateAsync: vi.fn() },
    addBookmark: { mutateAsync: vi.fn() },
    removeBookmark: { mutateAsync: vi.fn() },
  }),
}));
vi.mock("@/api/folders", () => ({
  useFolders: () => ({ data: [{ id: "f1", name: "Finance" }] }),
  useSetFolder: () => ({ mutateAsync: setFolder }),
  useFolderMutations: () => ({
    create: { mutateAsync: createFolder },
    rename: { mutateAsync: vi.fn() },
    remove: { mutateAsync: vi.fn() },
  }),
}));
vi.mock("sonner", () => ({ toast: { error: vi.fn(), success: vi.fn() } }));
import { toast } from "sonner";

const renderList = () =>
  render(
    <QueryClientProvider client={new QueryClient()}>
      <TooltipProvider>
        <MemoryRouter initialEntries={["/explore/eng/dashboards"]}>
          <Routes><Route path="/explore/:workspace/dashboards" element={<Dashboards />} /></Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );

beforeEach(() => {
  vi.clearAllMocks();
  setFolder.mockResolvedValue({});
  createFolder.mockResolvedValue({ id: "f-new", name: "New" });
  localStorage.clear();
});

describe("Dashboards — bulk move to a folder", () => {
  const selectAll = async () => {
    await userEvent.click(screen.getByRole("button", { name: "Bulk select" }));
    await userEvent.click(screen.getByLabelText("Select all"));
    await userEvent.click(screen.getByRole("button", { name: "Move 2 to folder…" }));
  };

  it("picks from the folders that exist instead of asking for a name", async () => {
    // Arrange
    renderList();

    // Act
    await selectAll();
    expect(screen.getAllByRole("menuitem").map((el) => el.textContent)).toEqual([
      "Finance", "Uncategorized", "New folder…",
    ]);
    await userEvent.click(screen.getByRole("menuitem", { name: "Finance" }));

    // Assert: moved into the existing folder, and no folder was created on the way
    await waitFor(() => expect(setFolder).toHaveBeenCalledTimes(2));
    expect(setFolder.mock.calls.map((c) => c[0])).toEqual(
      expect.arrayContaining([{ id: "d1", folder_id: "f1" }, { id: "d2", folder_id: "f1" }]),
    );
    expect(createFolder).not.toHaveBeenCalled();
    expect(toast.success).toHaveBeenCalledWith("Moved 2 dashboards");
  });

  it("keeps moving past a refused row and counts only what moved", async () => {
    // Arrange: the old loop stopped at the first error, leaving later rows unmoved and unreported
    setFolder.mockReset().mockRejectedValueOnce(new Error("forbidden")).mockResolvedValueOnce({});
    renderList();

    // Act
    await selectAll();
    await userEvent.click(screen.getByRole("menuitem", { name: "Uncategorized" }));

    // Assert
    await waitFor(() => expect(setFolder).toHaveBeenCalledTimes(2));
    expect(toast.error).toHaveBeenCalledWith("Error: forbidden");
    expect(toast.success).toHaveBeenCalledWith("Moved 1 dashboard");
  });
});
