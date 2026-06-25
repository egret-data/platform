import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { expect, test, vi } from "vitest";

const patchMock = vi.fn();

vi.mock("@/components/sqllab/SqlEditor", () => ({
  SqlEditor: () => <div data-testid="sql-editor" />,
}));
vi.mock("@/components/sqllab/HistoryTab", () => ({ HistoryTab: () => <div data-testid="history" /> }));
vi.mock("@/components/sqllab/SavedTab", () => ({ SavedTab: () => <div data-testid="saved" /> }));
vi.mock("@/components/sqllab/DataPanel", () => ({ DataPanel: () => <div data-testid="data-panel" /> }));

vi.mock("@/api/sqllab", () => ({
  useDatabases: () => ({ data: [{ id: "db1", name: "DB", engine: "trino" }] }),
  useSubmitQuery: () => ({ mutateAsync: vi.fn().mockResolvedValue({ id: "q1" }) }),
  useCancelQuery: () => ({ mutate: vi.fn() }),
  useQueryStatus: () => ({ data: { id: "q1", database_id: "db1", status: "failed", columns: null, preview: null, row_count: null, truncated: false, error: "line 1:8: boom" } }),
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

function renderTab(limit = 1000) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>
        <SqlTab ws="eng" tab={{ id: "t1", title: "Q", sql: "SELECT 1", dbId: "db1", limit }} patch={patchMock} onOpenSaved={vi.fn()} />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

test("a failed query shows the error in the Results panel, not on the toolbar", () => {
  renderTab();
  expect(screen.getByText("line 1:8: boom")).toBeInTheDocument();
});

test("the limit control shows the full preset text and a Custom… option", () => {
  renderTab();
  expect(screen.getByText("1,000 rows")).toBeInTheDocument();
});

test("a custom (non-preset) limit round-trips in the trigger", () => {
  renderTab(250);
  expect(screen.getByText("250 rows")).toBeInTheDocument();
});
