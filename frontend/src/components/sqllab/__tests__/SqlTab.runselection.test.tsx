import { render, screen, fireEvent } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { expect, test, vi } from "vitest";

const submitMock = vi.fn().mockResolvedValue({ id: "q1" });

// Stub SqlEditor with two buttons that drive the selection/run callbacks.
vi.mock("@/components/sqllab/SqlEditor", () => ({
  SqlEditor: ({ onRun, onSelectionChange }: { onRun?: (s: string) => void; onSelectionChange?: (s: string) => void }) => (
    <div>
      <button onClick={() => onSelectionChange?.("SELECT 2")}>select-some</button>
      <button onClick={() => onSelectionChange?.("")}>select-none</button>
      <button onClick={() => onRun?.("SELECT 2")}>kbd-run</button>
    </div>
  ),
}));

vi.mock("@/api/sqllab", () => ({
  useDatabases: () => ({ data: [{ id: "db1", name: "DB", engine: "trino" }] }),
  useSubmitQuery: () => ({ mutateAsync: submitMock }),
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
  csvUrl: () => "",
}));

vi.mock("@/api/datasets", () => ({ useCreateDataset: () => ({ mutateAsync: vi.fn(), isPending: false }) }));

import { SqlTab } from "@/components/sqllab/SqlTab";

function renderTab() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>
        <SqlTab ws="eng" tab={{ id: "t1", title: "Q", sql: "SELECT 1", dbId: "db1", limit: 100 }} patch={vi.fn()} onOpenSaved={vi.fn()} />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

test("Run button runs the full SQL by default", () => {
  submitMock.mockClear();
  renderTab();
  fireEvent.click(screen.getByRole("button", { name: "Run" }));
  expect(submitMock).toHaveBeenCalledWith(expect.objectContaining({ sql: "SELECT 1" }));
});

test("with a selection, the button becomes 'Run selection' and runs the selection", () => {
  submitMock.mockClear();
  renderTab();
  fireEvent.click(screen.getByRole("button", { name: "select-some" }));
  const runBtn = screen.getByRole("button", { name: "Run selection" });
  fireEvent.click(runBtn);
  expect(submitMock).toHaveBeenCalledWith(expect.objectContaining({ sql: "SELECT 2" }));
});

test("the keyboard run path (onRun) submits the resolved SQL", () => {
  submitMock.mockClear();
  renderTab();
  fireEvent.click(screen.getByRole("button", { name: "kbd-run" }));
  expect(submitMock).toHaveBeenCalledWith(expect.objectContaining({ sql: "SELECT 2" }));
});
