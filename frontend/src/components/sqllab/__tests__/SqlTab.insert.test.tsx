import { fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { expect, test, vi } from "vitest";

vi.mock("@/components/sqllab/SqlEditor", () => ({ SqlEditor: () => <div /> }));
vi.mock("@/components/sqllab/HistoryTab", () => ({ HistoryTab: () => <div /> }));
vi.mock("@/components/sqllab/SavedTab", () => ({ SavedTab: () => <div /> }));
// The Explorer, reduced to the one thing under test: a click inserts a name.
vi.mock("@/components/sqllab/DataPanel", () => ({
  DataPanel: ({ onInsert }: { onInsert: (n: string) => void }) => (
    <button onClick={() => onInsert("tpch.tiny.nation")}>insert nation</button>
  ),
}));
vi.mock("@/components/sqllab/SnippetsTab", () => ({ SnippetsTab: () => <div /> }));
vi.mock("@/api/sqllab", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useDatabases: () => ({ data: [{ id: "db1", name: "DB", engine: "trino" }] }),
  useSubmitQuery: () => ({ mutateAsync: vi.fn() }),
  useCancelQuery: () => ({ mutate: vi.fn() }),
  useQueryStatus: () => ({ data: undefined }),
  useResults: () => ({ data: undefined }),
  useSaveQuery: () => ({ mutateAsync: vi.fn() }),
  csvUrl: () => "",
}));
vi.mock("@/api/datasets", () => ({ useCreateDataset: () => ({ mutateAsync: vi.fn(), isPending: false }) }));

import { SqlTab } from "@/components/sqllab/SqlTab";

function clickInsert(sql: string) {
  const patch = vi.fn();
  render(
    <QueryClientProvider client={new QueryClient()}>
      <MemoryRouter>
        <SqlTab ws="eng" tab={{ id: "t1", title: "Q", sql, dbId: "db1", limit: 1000 }} patch={patch} onOpenSaved={vi.fn()} />
      </MemoryRouter>
    </QueryClientProvider>,
  );
  fireEvent.click(screen.getByText("insert nation"));
  return patch;
}

/** KI-084: a name clicked in the Explorer is separated from the word before it. */
test.each([
  ["SELECT", "SELECT tpch.tiny.nation"],
  ["SELECT * FROM", "SELECT * FROM tpch.tiny.nation"],
  ["SELECT * FROM ", "SELECT * FROM tpch.tiny.nation"],
  ["SELECT count(", "SELECT count(tpch.tiny.nation"],
  ["", "tpch.tiny.nation"],
])("inserting after %j gives %j", (sql, expected) => {
  expect(clickInsert(sql)).toHaveBeenCalledWith({ sql: expected });
});
