import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

const warehousesMock = vi.fn();
const createMutation = vi.fn();
const renameMutation = vi.fn();
const deleteMutation = vi.fn();
const createMutateAsync = vi.fn();
const renameMutateAsync = vi.fn();
const deleteMutate = vi.fn();
const storageConnsMock = vi.fn();

vi.mock("@/api/catalog", () => ({
  useWarehouses: () => warehousesMock(),
  useCreateWarehouse: () => createMutation(),
  useRenameWarehouse: () => renameMutation(),
  useDeleteWarehouse: () => deleteMutation(),
}));

vi.mock("@/api/storage", () => ({
  useStorageConns: () => storageConnsMock(),
}));

import WarehousesList from "../WarehousesList";

const navigateSpy = vi.fn();

function renderAt() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/catalog"]}>
        <Routes>
          <Route path="/explore/:workspace/catalog" element={<WarehousesList />} />
          <Route
            path="/explore/:workspace/catalog/:wid"
            element={<NavProbe />}
          />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

function NavProbe() {
  navigateSpy();
  return <div>warehouse detail page</div>;
}

const warehouses = [
  {
    id: "wh-1",
    name: "analytics",
    storage_conn_id: "conn-1",
    storage_conn_name: "prod-lakehouse",
    created_at: "2026-01-01T00:00:00Z",
  },
  {
    id: "wh-2",
    name: "staging",
    storage_conn_id: "conn-2",
    storage_conn_name: "backup-store",
    created_at: "2026-02-01T00:00:00Z",
  },
];

const conns = [
  { id: "conn-1", name: "prod-lakehouse" },
  { id: "conn-2", name: "backup-store" },
];

beforeEach(() => {
  warehousesMock.mockReset();
  createMutation.mockReset();
  renameMutation.mockReset();
  deleteMutation.mockReset();
  createMutateAsync.mockReset();
  renameMutateAsync.mockReset();
  deleteMutate.mockReset();
  storageConnsMock.mockReset();
  navigateSpy.mockReset();

  warehousesMock.mockReturnValue({
    data: warehouses,
    isLoading: false,
    isError: false,
    error: null,
  });
  createMutation.mockReturnValue({
    mutateAsync: createMutateAsync,
    isPending: false,
    reset: vi.fn(),
  });
  renameMutation.mockReturnValue({
    mutateAsync: renameMutateAsync,
    isPending: false,
    reset: vi.fn(),
  });
  deleteMutation.mockReturnValue({ mutate: deleteMutate, isPending: false });
  storageConnsMock.mockReturnValue({
    data: conns,
    isLoading: false,
    isError: false,
    error: null,
  });
  createMutateAsync.mockResolvedValue(warehouses[0]);
  renameMutateAsync.mockResolvedValue(warehouses[0]);
});

describe("WarehousesList", () => {
  it("renders the warehouse rows with name, storage, created", () => {
    renderAt();
    expect(screen.getByText("analytics")).toBeInTheDocument();
    expect(screen.getByText("staging")).toBeInTheDocument();
    expect(screen.getByText("prod-lakehouse")).toBeInTheDocument();
    expect(screen.getByText("backup-store")).toBeInTheDocument();
  });

  it("navigates to the warehouse detail when Open is clicked", async () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /open analytics/i }));
    await waitFor(() => expect(navigateSpy).toHaveBeenCalled());
    expect(screen.getByText("warehouse detail page")).toBeInTheDocument();
  });

  it("shows the empty state when there are no warehouses", () => {
    warehousesMock.mockReturnValue({
      data: [],
      isLoading: false,
      isError: false,
      error: null,
    });
    renderAt();
    expect(
      screen.getByText("No warehouses yet — create one on a storage connection."),
    ).toBeInTheDocument();
  });

  it("shows the skeleton while loading", () => {
    warehousesMock.mockReturnValue({
      data: undefined,
      isLoading: true,
      isError: false,
      error: null,
    });
    renderAt();
    expect(screen.queryByText("analytics")).not.toBeInTheDocument();
  });

  it("shows the workspace-admin notice on 403", () => {
    warehousesMock.mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: { status: 403 },
    });
    renderAt();
    expect(
      screen.getByText("Requires workspace-admin to manage catalog warehouses."),
    ).toBeInTheDocument();
    expect(screen.queryByText("analytics")).not.toBeInTheDocument();
  });

  it("creates a warehouse: picks a storage connection and sends {name, storage_conn_id}", async () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /add warehouse/i }));

    await waitFor(() => screen.getByText(/Provision an Iceberg warehouse/i));

    fireEvent.change(screen.getByLabelText(/^name$/i), {
      target: { value: "new-wh" },
    });

    // Radix Select renders a hidden native <select> in jsdom; drive it directly.
    const native = Array.from(
      document.querySelectorAll<HTMLSelectElement>("select"),
    ).find((s) => Array.from(s.options).some((o) => o.value === "conn-2"));
    if (!native) throw new Error("storage connection select not found");
    fireEvent.change(native, { target: { value: "conn-2" } });

    fireEvent.click(screen.getByRole("button", { name: /add warehouse/i, hidden: false }));

    await waitFor(() => {
      expect(createMutateAsync).toHaveBeenCalledWith({
        name: "new-wh",
        storage_conn_id: "conn-2",
      });
    });
  });

  it("renames a warehouse", async () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /rename analytics/i }));

    await waitFor(() => screen.getByText("Rename warehouse"));

    const input = screen.getByDisplayValue("analytics");
    fireEvent.change(input, { target: { value: "analytics-2" } });
    fireEvent.click(screen.getByRole("button", { name: /^save$/i }));

    await waitFor(() => {
      expect(renameMutateAsync).toHaveBeenCalledWith({
        wid: "wh-1",
        name: "analytics-2",
      });
    });
  });

  it("deletes a warehouse after confirmation", async () => {
    renderAt();
    fireEvent.click(screen.getByRole("button", { name: /delete analytics/i }));

    await waitFor(() => screen.getByText(/Delete «analytics»/));

    const confirmInput = screen.getByPlaceholderText("delete");
    fireEvent.change(confirmInput, { target: { value: "delete" } });
    fireEvent.click(screen.getByRole("button", { name: /^delete$/i }));

    await waitFor(() => {
      expect(deleteMutate).toHaveBeenCalledWith("wh-1");
    });
  });
});
