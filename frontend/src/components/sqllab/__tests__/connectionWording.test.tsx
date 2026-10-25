import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, expect, test, vi } from "vitest";

/**
 * KI-298: since KI-007 the workspace level says Connection, but SQL Lab and New dataset still
 * said "database" in a few messages.
 */
const toastError = vi.hoisted(() => vi.fn());
vi.mock("sonner", () => ({ toast: { error: toastError, success: vi.fn() } }));
vi.mock("@/components/sqllab/SqlEditor", () => ({ SqlEditor: () => <textarea aria-label="SQL editor" /> }));
vi.mock("@/components/sqllab/HistoryTab", () => ({ HistoryTab: () => <div /> }));
vi.mock("@/components/sqllab/SavedTab", () => ({ SavedTab: () => <div /> }));
vi.mock("@/components/sqllab/DataPanel", () => ({ DataPanel: () => <div /> }));
vi.mock("@/api/sqllab", () => ({
  useDatabases: () => ({ data: [{ id: "db1", name: "DB", engine: "trino" }] }),
  useSubmitQuery: () => ({ mutateAsync: vi.fn() }),
  useCancelQuery: () => ({ mutate: vi.fn() }),
  useQueryStatus: () => ({ data: undefined }),
  useResults: () => ({ data: undefined }),
  useSaveQuery: () => ({ mutateAsync: vi.fn() }),
  useHistory: () => ({ data: [] }),
  useCatalogs: () => ({ data: [] }),
  useSchemas: () => ({ data: [] }),
  useTables: () => ({ data: [] }),
  useColumns: () => ({ data: [] }),
  useRefreshSchema: () => ({ mutate: vi.fn() }),
  csvUrl: () => "",
}));
vi.mock("@/api/datasets", () => ({ useCreateDataset: () => ({ mutateAsync: vi.fn(), isPending: false }) }));

import { SqlTab } from "@/components/sqllab/SqlTab";
import { CreateDatasetDialog } from "@/components/dataset/CreateDatasetDialog";

const wrap = (ui: React.ReactNode) => render(
  <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
    <MemoryRouter>{ui}</MemoryRouter>
  </QueryClientProvider>,
);

beforeEach(() => toastError.mockReset());

test("a SQL Lab tab with no connection says so, and Run asks for a connection", async () => {
  wrap(<SqlTab ws="eng" tab={{ id: "t1", title: "Q", sql: "SELECT 1", dbId: null, limit: 100 }} patch={vi.fn()} onOpenSaved={vi.fn()} />);
  expect(screen.getByText("No connection")).toBeInTheDocument();
  expect(screen.getByTitle("No connection selected")).toBeInTheDocument();
  expect(screen.queryByText(/database/i)).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: /^Run/ }));
  expect(toastError).toHaveBeenCalledWith("Pick a connection");
});

test("saving a query as a dataset with no connection asks for a connection", async () => {
  wrap(<CreateDatasetDialog ws="eng" open onOpenChange={() => {}} initialSql="SELECT 1" initialName="x" />);
  await userEvent.click(screen.getByRole("button", { name: "Create dataset" }));
  expect(toastError).toHaveBeenCalledWith("Pick a connection");
});
