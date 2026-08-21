import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
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

function mockApi() {
  const calls: Record<string, number> = {};
  vi.stubGlobal("fetch", vi.fn(async (url: string | URL, init?: RequestInit) => {
    const u = String(url); const m = (init?.method ?? "GET").toUpperCase();
    const json = (b: unknown) => new Response(JSON.stringify(b), { status: 200, headers: { "content-type": "application/json" } });
    if (u.match(/\/ws\/[^/]+\/databases$/)) return json([{ id: "db1", name: "trino", engine: "trino" }]);
    if (u.includes("/sqllab/databases/")) return json([]);            // schema selects
    if (u.endsWith("/sqllab/queries")) return json([]);              // saved list
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
