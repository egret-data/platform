import { render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { expect, test, vi } from "vitest";

vi.mock("@monaco-editor/react", () => ({
  default: ({ value, onChange }: { value: string; onChange?: (v: string) => void }) =>
    <textarea data-testid="sql" value={value} onChange={(e) => onChange?.(e.target.value)} />,
}));

vi.mock("@/api/sqllab", () => ({
  useDatabases: () => ({ data: [] }),
  useSubmitQuery: () => ({ mutateAsync: vi.fn() }),
  useCancelQuery: () => ({ mutate: vi.fn() }),
  useQueryStatus: () => ({ data: undefined }),
  useResults: () => ({ data: undefined }),
  useSaveQuery: () => ({ mutateAsync: vi.fn() }),
  useHistory: () => ({ data: [] }),
  useSavedQueries: () => ({ data: [] }),
  useDeleteSavedQuery: () => ({ mutate: vi.fn() }),
  useCatalogs: () => ({ data: [] }),
  useSchemas: () => ({ data: [] }),
  useTables: () => ({ data: [] }),
  useColumns: () => ({ data: [] }),
  useRefreshSchema: () => ({ mutate: vi.fn() }),
  useSearchObjects: () => ({ data: [], isLoading: false, isError: false }),
  fqnOf: (h: { catalog: string; schema: string; table: string }) => (h.catalog ? `${h.catalog}.${h.schema}.${h.table}` : `${h.schema}.${h.table}`),
  csvUrl: () => "",
}));

vi.mock("@/api/datasets", () => ({
  useCreateDataset: () => ({ mutateAsync: vi.fn(), isPending: false }),
}));

import { SqlTab } from "@/components/sqllab/SqlTab";

function renderSqlTab() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const tab = { id: "t1", title: "Q1", sql: "", dbId: null, limit: 1000 };
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>
        <SqlTab
          ws="eng"
          tab={tab}
          patch={vi.fn()}
          onOpenSaved={vi.fn()}
        />
      </MemoryRouter>
    </QueryClientProvider>
  );
}

test("renders 'Save as dataset' button in the SQL Lab toolbar", () => {
  renderSqlTab();
  expect(screen.getByRole("button", { name: "Save as dataset" })).toBeInTheDocument();
});
