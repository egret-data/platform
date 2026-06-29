import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

const warehouseMock = vi.fn();
const statsMock = vi.fn();
const tasksMock = vi.fn();
const browseProps = vi.fn();
const accessProps = vi.fn();

vi.mock("@/api/catalog", () => ({
  useWarehouse: (ws: string, wid: string) => warehouseMock(ws, wid),
  useRenameWarehouse: () => ({ mutateAsync: vi.fn(), isPending: false }),
  useWarehouseStatistics: (ws: string, wid: string) => statsMock(ws, wid),
  useWarehouseTasks: (ws: string, wid: string) => tasksMock(ws, wid),
}));

// CatalogBrowse / CatalogAccess are heavy children unrelated to these tabs;
// stub them but spy on their props so the prop-passing contract stays covered.
vi.mock("../CatalogBrowse", () => ({
  default: (props: { ws: string; wid: string }) => {
    browseProps(props);
    return <div>Browse content</div>;
  },
}));
vi.mock("../CatalogAccess", () => ({
  default: (props: { ws: string; wid: string }) => {
    accessProps(props);
    return <div>Access content</div>;
  },
}));

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
  browseProps.mockReset();
  accessProps.mockReset();
  warehouseMock.mockReturnValue({
    data: {
      id: "wh-1",
      name: "analytics",
      storage_conn_id: "conn-1",
      storage_conn_name: "prod-lakehouse",
      created_at: "2026-01-01T00:00:00Z",
    },
    isLoading: false,
    isError: false,
    error: null,
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

describe("WarehouseDetail", () => {
  it("renders the header with name + storage and a back link", () => {
    renderAt();
    expect(screen.getByRole("heading", { level: 1, name: "analytics" })).toBeInTheDocument();
    expect(screen.getByText(/on prod-lakehouse/i)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /all warehouses/i })).toHaveAttribute(
      "href",
      "/explore/eng/catalog",
    );
  });

  it("renders all five tabs", () => {
    renderAt();
    expect(screen.getByRole("tab", { name: "Browse" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Access" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Details" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Statistics" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Tasks" })).toBeInTheDocument();
  });

  it("passes ws + wid to the Browse tab", () => {
    renderAt();
    expect(screen.getByText("Browse content")).toBeInTheDocument();
    expect(browseProps).toHaveBeenCalledWith({ ws: "eng", wid: "wh-1" });
  });

  it("passes ws + wid to the Access tab", async () => {
    const user = userEvent.setup();
    renderAt();
    await user.click(screen.getByRole("tab", { name: "Access" }));
    expect(screen.getByText("Access content")).toBeInTheDocument();
    expect(accessProps).toHaveBeenCalledWith({ ws: "eng", wid: "wh-1" });
  });

  it("shows storage + created on the Details tab", async () => {
    const user = userEvent.setup();
    renderAt();
    await user.click(screen.getByRole("tab", { name: "Details" }));
    expect(screen.getByText("Storage connection")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /rename/i })).toBeInTheDocument();
  });

  it("shows a not-found alert on 404", () => {
    warehouseMock.mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: { status: 404 },
    });
    renderAt();
    expect(screen.getByText("Warehouse not found")).toBeInTheDocument();
  });

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
