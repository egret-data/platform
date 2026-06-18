import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, test, vi } from "vitest";

// Monaco can't load under jsdom — stub it with a textarea.
vi.mock("@monaco-editor/react", () => ({
  default: ({ value, onChange }: { value: string; onChange?: (v: string) => void }) =>
    <textarea data-testid="sql" value={value} onChange={(e) => onChange?.(e.target.value)} />,
}));

import SqlLab from "@/routes/explore/SqlLab";

afterEach(() => vi.restoreAllMocks());

function mockApi() {
  let statusCalls = 0;
  vi.stubGlobal("fetch", vi.fn(async (url: string | URL, init?: RequestInit) => {
    const u = String(url); const m = (init?.method ?? "GET").toUpperCase();
    const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
    if (u.includes("/me/databases")) return json([{ id: "db1", name: "trino", engine: "trino" }]);
    if (u.endsWith("/sqllab/queries")) return json([]);
    if (u.includes("/sqllab/databases/")) return json([]);          // schema tree levels
    if (u.endsWith("/queries") && m === "GET") return json([]);     // history
    if (u.endsWith("/queries") && m === "POST") return json({ id: "q1", status: "queued" });
    if (u.includes("/queries/q1/results")) return json({ columns: [{ name: "n", type: "bigint" }], rows: [[1]], page: 0, page_size: 100, row_count: 1, truncated: false });
    if (u.includes("/queries/q1")) { statusCalls++; return json(statusCalls < 2
      ? { id: "q1", database_id: "db1", status: "running", columns: null, preview: null, row_count: null, truncated: false, error: null }
      : { id: "q1", database_id: "db1", status: "finished", columns: [{ name: "n", type: "bigint" }], preview: [[1]], row_count: 1, truncated: false, error: null }); }
    return json({});
  }));
}

test("run a query renders results", async () => {
  mockApi();
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={qc}><MemoryRouter initialEntries={["/explore/eng/sql-lab"]}>
    <Routes><Route path="/explore/:workspace/sql-lab" element={<SqlLab />} /></Routes>
  </MemoryRouter></QueryClientProvider>);
  await userEvent.click(await screen.findByText("Run"));
  await waitFor(() => expect(screen.getByText("n")).toBeInTheDocument(), { timeout: 4000 });
});
