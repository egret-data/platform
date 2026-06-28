import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

const namespacesMock = vi.fn();
const contentsMock = vi.fn();
const tableDetailMock = vi.fn();

vi.mock("@/api/catalog", () => ({
  nsDotted: (levels: string[]) => levels.join("."),
  useCatalogNamespaces: (ws: string, wid: string) => namespacesMock(ws, wid),
  useNamespaceContents: (_ws: string, wid: string, ns: string | null) => contentsMock(wid, ns),
  useTableDetail: (_ws: string, wid: string, ns: string | null, table: string | null) =>
    tableDetailMock(wid, ns, table),
}));

import CatalogBrowse from "../CatalogBrowse";

function renderAt() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <CatalogBrowse ws="eng" wid="wh-1" />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  namespacesMock.mockReset();
  contentsMock.mockReset();
  tableDetailMock.mockReset();
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
  it("calls the browse hooks scoped to the warehouse id", () => {
    renderAt();
    expect(namespacesMock).toHaveBeenCalledWith("eng", "wh-1");
  });

  it("renders namespaces and drills into a table detail", async () => {
    const user = userEvent.setup();
    renderAt();

    // Namespace list rendered.
    expect(screen.getByText("sales")).toBeInTheDocument();
    expect(screen.getByText("sales.2024")).toBeInTheDocument();

    // Selecting a namespace shows its tables + views.
    await user.click(screen.getByText("sales"));
    expect(screen.getByText("orders")).toBeInTheDocument();
    expect(screen.getByText("v_orders")).toBeInTheDocument();
    // Contents hook received the wid.
    expect(contentsMock).toHaveBeenCalledWith("wh-1", "sales");

    // Selecting a table shows its detail (schema column + snapshot info).
    await user.click(screen.getByText("orders"));
    expect(screen.getByText("id")).toBeInTheDocument();
    expect(screen.getByText("write.format.default")).toBeInTheDocument();
    expect(screen.getByText("42")).toBeInTheDocument();
    expect(tableDetailMock).toHaveBeenCalledWith("wh-1", "sales", "orders");
  });
});
