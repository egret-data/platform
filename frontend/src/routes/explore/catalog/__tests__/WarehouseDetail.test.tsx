import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

const warehouseMock = vi.fn();
const browseProps = vi.fn();
const accessProps = vi.fn();

vi.mock("@/api/catalog", () => ({
  useWarehouse: () => warehouseMock(),
  useRenameWarehouse: () => ({ mutateAsync: vi.fn(), isPending: false }),
}));

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

  it("renders the three tabs", () => {
    renderAt();
    expect(screen.getByRole("tab", { name: "Browse" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Access" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Details" })).toBeInTheDocument();
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
});
