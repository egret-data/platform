import { act, fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { beforeEach, expect, test, vi } from "vitest";

const state = vi.hoisted(() => ({
  status: undefined as unknown,
  submit: vi.fn(),
}));

// Monaco handles ⌘K itself, as the first key of a chord, on its own DOM node,
// and stops the event there: the stub does what it does to the event.
vi.mock("@/components/sqllab/SqlEditor", () => ({
  SqlEditor: () => {
    const ref = useRef<HTMLTextAreaElement>(null);
    useEffect(() => {
      const el = ref.current!;
      const swallow = (e: KeyboardEvent) => {
        if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") { e.preventDefault(); e.stopPropagation(); }
      };
      el.addEventListener("keydown", swallow);
      return () => el.removeEventListener("keydown", swallow);
    }, []);
    return <textarea ref={ref} aria-label="SQL editor" />;
  },
}));
vi.mock("@/components/sqllab/HistoryTab", () => ({ HistoryTab: () => <div /> }));
vi.mock("@/components/sqllab/SavedTab", () => ({ SavedTab: () => <div /> }));
vi.mock("@/components/sqllab/DataPanel", () => ({ DataPanel: () => <div /> }));
vi.mock("@/api/sqllab", () => ({
  useDatabases: () => ({ data: [{ id: "db1", name: "DB", engine: "trino" }] }),
  useSubmitQuery: () => ({ mutateAsync: state.submit }),
  useCancelQuery: () => ({ mutate: vi.fn() }),
  useQueryStatus: () => ({ data: state.status }),
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

const view = (status: string) => ({
  id: "q1", database_id: "db1", status, columns: null, preview: null, row_count: null, truncated: false, error: null,
});

function renderTab(sql = "SELECT 1") {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>
        <SqlTab ws="eng" tab={{ id: "t1", title: "Q", sql, dbId: "db1", limit: 100 }} patch={vi.fn()} onOpenSaved={vi.fn()} />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  state.status = undefined;
  state.submit = vi.fn().mockResolvedValue({ id: "q1" });
});

test("KI-079: ⌘K opens the command palette while the cursor is in the editor", () => {
  renderTab();
  const editor = screen.getByRole("textbox", { name: "SQL editor" });
  editor.focus();
  fireEvent.keyDown(editor, { key: "k", metaKey: true });
  expect(screen.getByRole("dialog", { name: "Command palette" })).toBeInTheDocument();
});

test("KI-079: Ctrl+K does the same", () => {
  renderTab();
  const editor = screen.getByRole("textbox", { name: "SQL editor" });
  fireEvent.keyDown(editor, { key: "K", ctrlKey: true });
  expect(screen.getByRole("dialog", { name: "Command palette" })).toBeInTheDocument();
});

test("KI-078: a cancelled run says so in the Results panel, with the time", async () => {
  const { rerender } = renderTab();
  await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Run" })); });
  state.status = view("cancelled");
  rerender(
    <QueryClientProvider client={new QueryClient()}>
      <MemoryRouter>
        <SqlTab ws="eng" tab={{ id: "t1", title: "Q", sql: "SELECT 1", dbId: "db1", limit: 100 }} patch={vi.fn()} onOpenSaved={vi.fn()} />
      </MemoryRouter>
    </QueryClientProvider>,
  );
  expect(screen.getByText("Query cancelled")).toBeInTheDocument();
  expect(screen.queryByText("No results yet.")).not.toBeInTheDocument();
  expect(screen.getAllByText(/^Cancelled after \d/).length).toBeGreaterThan(0);
});

test("KI-086: a query refused before it starts shows the message in the Results panel, without an `Error:` prefix", async () => {
  state.submit = vi.fn().mockRejectedValue(new Error("invalid input: parameter 'n' is not a number: abc"));
  renderTab("SELECT {{n}}");
  await act(async () => { fireEvent.click(screen.getByRole("button", { name: "Run" })); });
  const panel = screen.getByText("invalid input: parameter 'n' is not a number: abc");
  expect(panel).toBeInTheDocument();
  expect(screen.getByText("Query not started")).toBeInTheDocument();
  expect(screen.queryByText(/^Error:/)).not.toBeInTheDocument();
});
