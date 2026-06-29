import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";

const namespacesMock = vi.fn();
const contentsMock = vi.fn();
const tableDetailMock = vi.fn();
const createNamespaceMock = vi.fn();
const deleteNamespaceMock = vi.fn();

vi.mock("@/api/catalog", () => ({
  nsDotted: (levels: string[]) => levels.join("."),
  useCatalogNamespaces: (ws: string, wid: string) => namespacesMock(ws, wid),
  useNamespaceContents: (_ws: string, wid: string, ns: string | null) => contentsMock(wid, ns),
  useTableDetail: (_ws: string, wid: string, ns: string | null, table: string | null) =>
    tableDetailMock(wid, ns, table),
  useCreateNamespace: () => ({ mutateAsync: createNamespaceMock, isPending: false }),
  useDeleteNamespace: () => ({ mutate: deleteNamespaceMock, isPending: false }),
}));

import CatalogBrowse from "../CatalogBrowse";

function renderAt(initialEntries: string[] = ["/"]) {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={initialEntries}>
        <CatalogBrowse ws="eng" wid="wh-1" />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  namespacesMock.mockReset();
  contentsMock.mockReset();
  tableDetailMock.mockReset();
  createNamespaceMock.mockReset();
  deleteNamespaceMock.mockReset();
  createNamespaceMock.mockResolvedValue(undefined);
  namespacesMock.mockReturnValue({
    data: [{ levels: ["sale"] }, { levels: ["sale", "2024"] }, { levels: ["marketing"] }],
    isLoading: false,
    isError: false,
  });
  // Contents only resolve for the expanded namespace; default empty.
  contentsMock.mockImplementation((_wid: string, ns: string | null) =>
    ns === "sale"
      ? { data: { tables: ["orders", "customers"], views: ["v_orders"] }, isLoading: false, isError: false }
      : { data: { tables: [], views: [] }, isLoading: false, isError: false },
  );
  tableDetailMock.mockReturnValue({
    data: {
      name: "orders",
      location: "s3://wh/orders",
      current_schema_id: 1,
      columns: [{ name: "id", type: "long", required: true }],
      current_snapshot_id: 42,
      snapshot_count: 1,
      properties: { "write.format.default": "parquet" },
      format_version: 2,
      last_updated_ms: 1750000000000,
      partition_fields: [],
      sort_fields: [],
      snapshots: [],
    },
    isLoading: false,
    isError: false,
  });
});

describe("Catalog browse tree", () => {
  it("renders top-level namespaces as a tree", () => {
    renderAt();
    expect(screen.getByText("marketing")).toBeInTheDocument();
    expect(screen.getByText("sale")).toBeInTheDocument();
    // Child not visible until 'sale' is expanded.
    expect(screen.queryByText("2024")).not.toBeInTheDocument();
  });

  it("expands a namespace, lazy-loads its tables/views, and drills into a table", async () => {
    const user = userEvent.setup();
    renderAt();
    await user.click(screen.getByRole("button", { name: /expand sale/i }));
    expect(screen.getByText("2024")).toBeInTheDocument(); // child namespace
    expect(screen.getByText("orders")).toBeInTheDocument(); // table leaf
    expect(screen.getByText("v_orders")).toBeInTheDocument(); // view leaf
    expect(contentsMock).toHaveBeenCalledWith("wh-1", "sale");

    await user.click(screen.getByText("orders"));
    expect(tableDetailMock).toHaveBeenCalledWith("wh-1", "sale", "orders");
    expect(screen.getByText("write.format.default")).toBeInTheDocument(); // detail shown
  });

  it("filters the tree by the search box", async () => {
    const user = userEvent.setup();
    renderAt();
    await user.type(screen.getByPlaceholderText(/filter/i), "marketing");
    expect(screen.getByText("marketing")).toBeInTheDocument();
    expect(screen.queryByText("sale")).not.toBeInTheDocument();
  });

  it("restores selection from the URL (deep-link)", () => {
    renderAt(["/?ns=sale&table=orders"]);
    // Detail rendered without any clicks.
    expect(tableDetailMock).toHaveBeenCalledWith("wh-1", "sale", "orders");
    expect(screen.getByText("write.format.default")).toBeInTheDocument();
  });

  it("creates a root namespace from the New namespace dialog", async () => {
    const user = userEvent.setup();
    renderAt();
    await user.click(screen.getByRole("button", { name: "New namespace" }));
    await user.type(screen.getByLabelText("Name"), "ops");
    await user.click(screen.getByRole("button", { name: "Create" }));
    expect(createNamespaceMock).toHaveBeenCalledWith({ parent: null, name: "ops" });
  });

  it("deletes a namespace after confirmation", async () => {
    const user = userEvent.setup();
    renderAt();
    await user.click(screen.getByRole("button", { name: "Delete sale" }));
    await user.type(screen.getByPlaceholderText("delete"), "delete");
    await user.click(screen.getByRole("button", { name: /^Delete$/ }));
    expect(deleteNamespaceMock).toHaveBeenCalledWith("sale", expect.anything());
  });

  it("shows a 'Showing X of Y' note for a large namespace", async () => {
    contentsMock.mockImplementation((_wid: string, ns: string | null) =>
      ns === "sale"
        ? {
            data: {
              tables: Array.from({ length: 250 }, (_, i) => `t${i}`),
              views: [],
            },
            isLoading: false,
            isError: false,
          }
        : { data: { tables: [], views: [] }, isLoading: false, isError: false },
    );
    const user = userEvent.setup();
    renderAt();
    await user.click(screen.getByRole("button", { name: /expand sale/i }));
    expect(screen.getByText(/Showing 200 of 250/i)).toBeInTheDocument();
  });
});
