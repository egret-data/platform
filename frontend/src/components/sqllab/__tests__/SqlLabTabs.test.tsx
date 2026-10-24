import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { fireEvent } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

vi.mock("@/components/monaco/MonacoEditor", () => ({
  default: ({ value, onChange }: { value: string; onChange?: (v: string) => void }) =>
    <textarea data-testid="sql" value={value} onChange={(e) => onChange?.(e.target.value)} />,
}));
import SqlLab from "@/routes/explore/SqlLab";

beforeEach(() => localStorage.clear());
afterEach(() => vi.restoreAllMocks());

function mockApi(opts: { saved?: unknown[] } = {}) {
  const calls: Record<string, number> = {};
  vi.stubGlobal("fetch", vi.fn(async (url: string | URL, init?: RequestInit) => {
    const u = String(url); const m = (init?.method ?? "GET").toUpperCase();
    const json = (b: unknown) => new Response(JSON.stringify(b), { status: 200, headers: { "content-type": "application/json" } });
    if (u.match(/\/ws\/[^/]+\/databases$/)) return json([{ id: "db1", name: "trino", engine: "trino" }]);
    if (u.includes("/folders")) return json([]);                     // saved-query folders
    if (u.includes("/sqllab/databases/")) return json([]);            // schema selects
    if (u.endsWith("/sqllab/queries")) return json(opts.saved ?? []); // saved list
    if (u.endsWith("/queries") && m === "GET") return json([]);      // history
    if (u.endsWith("/queries") && m === "POST") { calls.submit = (calls.submit ?? 0) + 1; return json({ id: `q${calls.submit}`, status: "queued" }); }
    if (u.match(/\/queries\/q\d+\/results/)) { const id = u.match(/q\d+/)![0]; return json({ columns: [{ name: id, type: "bigint" }], rows: [[1]], page: 0, page_size: 100, row_count: 1, truncated: false }); }
    if (u.match(/\/queries\/q\d+/)) { const id = u.match(/q\d+/)![0]; return json({ id, database_id: "db1", status: "finished", columns: [{ name: id, type: "bigint" }], preview: [[1]], row_count: 1, truncated: false, error: null }); }
    return json({});
  }));
}

function renderApp() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}><MemoryRouter initialEntries={["/explore/eng/sql-lab"]}>
    <Routes><Route path="/explore/:workspace/sql-lab" element={<SqlLab />} /></Routes>
  </MemoryRouter></QueryClientProvider>);
}

test("opens one tab, can add a second", async () => {
  mockApi(); renderApp();
  expect(await screen.findByText("Query 1")).toBeInTheDocument();
  await userEvent.click(screen.getByTitle("New tab"));
  expect(await screen.findByText("Query 2")).toBeInTheDocument();
});

test("run query A then B shows B's results, never A's (no stale)", async () => {
  mockApi(); renderApp();
  // Wait for the single DB to auto-select (DataPanel effect fires after databases load)
  await screen.findByText(/trino \(trino\)/);
  await userEvent.click(await screen.findByText("Run")); // q1
  await waitFor(() => expect(screen.getByText("q1")).toBeInTheDocument());
  await userEvent.click(screen.getByText("Run")); // q2
  await waitFor(() => expect(screen.getByText("q2")).toBeInTheDocument());
  expect(screen.queryByText("q1")).not.toBeInTheDocument();
});

/** KI-082: switching tabs keeps each tab's result, status and parameter values. */
test("switching to another tab and back keeps the result and the parameter values", async () => {
  mockApi(); renderApp();
  await screen.findByText(/trino \(trino\)/);
  fireEvent.change(screen.getByTestId("sql"), { target: { value: "SELECT {{p}}" } });
  fireEvent.change(await screen.findByLabelText("Value of p"), { target: { value: "u4" } });
  expect(screen.getByLabelText("Value of p")).toHaveValue("u4");
  await userEvent.click(screen.getByText("Run"));
  await waitFor(() => expect(screen.getByText("q1")).toBeVisible());
  await userEvent.click(screen.getByTitle("New tab"));
  await screen.findByText("Query 2");
  await waitFor(() => expect(screen.getByText("q1")).not.toBeVisible());
  await userEvent.click(screen.getByText("Query 1"));
  expect(screen.getByText("q1")).toBeVisible();
  expect(screen.getByLabelText("Value of p")).toHaveValue("u4");
  expect(screen.getByText(/Done in/)).toBeVisible();
});

/** KI-083: a saved query opens in a tab named after it. */
test("opening a saved query names the tab after the query", async () => {
  mockApi({ saved: [{ id: "s1", database_id: "db1", name: "u4 revenue", sql: "SELECT 2", description: "", created_by: null, folder_id: null, updated_at: new Date().toISOString() }] });
  renderApp();
  await screen.findByText(/trino \(trino\)/);
  // Radix Tabs switch on mousedown.
  fireEvent.mouseDown(screen.getByRole("tab", { name: "Saved" }));
  fireEvent.click(await screen.findByText("u4 revenue"));
  expect(await screen.findByRole("button", { name: "u4 revenue" })).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Saved" })).not.toBeInTheDocument();
});

/** KI-094: the rename box selects the old title, so typing replaces it. */
test("renaming a tab replaces the old title", async () => {
  mockApi(); renderApp();
  await userEvent.dblClick(await screen.findByText("Query 1"));
  await userEvent.keyboard("u4 tab{Enter}");
  expect(await screen.findByRole("button", { name: "u4 tab" })).toBeInTheDocument();
});

/** KI-228 + KI-161: the run is announced once, and one row is "1 row". */
test("a finished run is announced once, in the singular for one row", async () => {
  mockApi(); renderApp();
  await screen.findByText(/trino \(trino\)/);
  await userEvent.click(screen.getByText("Run"));
  await waitFor(() => expect(screen.getByText("q1")).toBeInTheDocument());
  const done = screen.getAllByText(/Done in/);
  expect(done).toHaveLength(1);
  expect(done[0]).toHaveTextContent(/· 1 row$/);
});
