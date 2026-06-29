import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route } from "react-router-dom";

const warehouseMock = vi.fn();
const statsMock = vi.fn();
const tasksMock = vi.fn();

vi.mock("@/api/catalog", () => ({
  useWarehouse: (ws: string, wid: string) => warehouseMock(ws, wid),
  useRenameWarehouse: () => ({ mutateAsync: vi.fn(), isPending: false }),
  useWarehouseStatistics: (ws: string, wid: string) => statsMock(ws, wid),
  useWarehouseTasks: (ws: string, wid: string) => tasksMock(ws, wid),
}));

// CatalogBrowse / CatalogAccess are heavy children unrelated to these tabs.
vi.mock("../CatalogBrowse", () => ({ default: () => <div>browse-stub</div> }));
vi.mock("../CatalogAccess", () => ({ default: () => <div>access-stub</div> }));

import WarehouseDetail from "../WarehouseDetail";

function renderAt() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/catalog/wh-1"]}>
        <Routes>
          <Route path="/explore/:workspace/catalog/:wid" element={<WarehouseDetail />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  warehouseMock.mockReset();
  statsMock.mockReset();
  tasksMock.mockReset();
  warehouseMock.mockReturnValue({
    data: {
      id: "wh-1",
      name: "Analytics",
      storage_conn_id: "c1",
      storage_conn_name: "minio",
      created_at: "2026-06-01T00:00:00Z",
    },
    isLoading: false,
    isError: false,
  });
  statsMock.mockReturnValue({
    data: {
      entries: [
        {
          timestamp: "2026-06-29T10:00:00Z",
          number_of_tables: 7,
          number_of_views: 2,
          updated_at: "2026-06-29T10:05:00Z",
        },
      ],
    },
    isLoading: false,
    isError: false,
  });
  tasksMock.mockReturnValue({
    data: [
      {
        task_id: "t-1",
        task_type: "tabular_expiration",
        status: "SUCCESS",
        entity_name: "sale.orders",
        created_at: "2026-06-29T09:00:00Z",
        scheduled_for: "2026-06-29T09:00:00Z",
        attempt: 1,
        progress: 1.0,
      },
    ],
    isLoading: false,
    isError: false,
  });
});

describe("WarehouseDetail tabs", () => {
  it("renders the Statistics tab with the latest counts", async () => {
    const user = userEvent.setup();
    renderAt();
    await user.click(screen.getByRole("tab", { name: "Statistics" }));
    expect(statsMock).toHaveBeenCalledWith("eng", "wh-1");
    expect(screen.getByText("7")).toBeInTheDocument(); // tables
    expect(screen.getByText("2")).toBeInTheDocument(); // views
  });

  it("renders the Tasks tab with a row + status badge", async () => {
    const user = userEvent.setup();
    renderAt();
    await user.click(screen.getByRole("tab", { name: "Tasks" }));
    expect(tasksMock).toHaveBeenCalledWith("eng", "wh-1");
    expect(screen.getByText("tabular_expiration")).toBeInTheDocument();
    expect(screen.getByText("sale.orders")).toBeInTheDocument();
    expect(screen.getByText("SUCCESS")).toBeInTheDocument();
  });

  it("shows empty states when there is no data", async () => {
    statsMock.mockReturnValue({ data: { entries: [] }, isLoading: false, isError: false });
    tasksMock.mockReturnValue({ data: [], isLoading: false, isError: false });
    const user = userEvent.setup();
    renderAt();

    await user.click(screen.getByRole("tab", { name: "Statistics" }));
    expect(screen.getByText(/No statistics yet/)).toBeInTheDocument();

    await user.click(screen.getByRole("tab", { name: "Tasks" }));
    expect(screen.getByText("No tasks.")).toBeInTheDocument();
  });
});
