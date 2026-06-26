import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

const summaryMock = vi.fn();
const namespacesMock = vi.fn();
const contentsMock = vi.fn();
const tableDetailMock = vi.fn();

vi.mock("@/api/catalog", () => ({
  nsDotted: (levels: string[]) => levels.join("."),
  useCatalogSummary: () => summaryMock(),
  useCatalogNamespaces: () => namespacesMock(),
  useNamespaceContents: (_ws: string, ns: string | null) => contentsMock(ns),
  useTableDetail: (_ws: string, ns: string | null, table: string | null) => tableDetailMock(ns, table),
}));

import Catalog from "../Catalog";

function renderAt() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/catalog"]}>
        <Routes>
          <Route path="/explore/:workspace/catalog" element={<Catalog />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  summaryMock.mockReset();
  namespacesMock.mockReset();
  contentsMock.mockReset();
  tableDetailMock.mockReset();
  // Sensible defaults; individual tests override summary/namespaces as needed.
  namespacesMock.mockReturnValue({
    data: [{ levels: ["sales"] }, { levels: ["sales", "2024"] }],
    isLoading: false,
    isError: false,
  });
  contentsMock.mockReturnValue({
    data: { tables: ["orders"], views: ["v_orders"] },
    isLoading: false,
    isError: false,
  });
  tableDetailMock.mockReturnValue({
    data: {
      name: "orders",
      location: "s3://wh/orders",
      current_schema_id: 1,
      columns: [{ name: "id", type: "long", required: true }],
      current_snapshot_id: 42,
      snapshot_count: 3,
      properties: { "write.format.default": "parquet" },
    },
    isLoading: false,
    isError: false,
  });
});

describe("Catalog browser", () => {
  it("renders the no-catalog empty state when summary is null (404)", () => {
    summaryMock.mockReturnValue({ data: null, isLoading: false, isError: false });
    renderAt();
    expect(screen.getByText("No catalog provisioned for this workspace yet.")).toBeInTheDocument();
  });

  it("renders namespaces and drills into a table detail", async () => {
    summaryMock.mockReturnValue({
      data: { id: "c1", name: "main", warehouse_location: "s3://wh" },
      isLoading: false,
      isError: false,
    });
    const user = userEvent.setup();
    renderAt();

    // Namespace list rendered.
    expect(screen.getByText("sales")).toBeInTheDocument();
    expect(screen.getByText("sales.2024")).toBeInTheDocument();

    // Selecting a namespace shows its tables + views.
    await user.click(screen.getByText("sales"));
    expect(screen.getByText("orders")).toBeInTheDocument();
    expect(screen.getByText("v_orders")).toBeInTheDocument();

    // Selecting a table shows its detail (schema column + snapshot info).
    await user.click(screen.getByText("orders"));
    expect(screen.getByText("id")).toBeInTheDocument();
    expect(screen.getByText("write.format.default")).toBeInTheDocument();
    expect(screen.getByText("42")).toBeInTheDocument();
  });
});
