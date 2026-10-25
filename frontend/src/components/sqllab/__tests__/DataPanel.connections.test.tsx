import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";

/**
 * KI-159: the SQL Lab connection picker as a member who is not a workspace
 * admin. `fetch` is stubbed and nothing above it is, so the real hook, the
 * generated client and `apiFetch`'s status mapping all run.
 */
const fetchMock = vi.fn();
vi.stubGlobal("fetch", fetchMock);

import { DataPanel } from "@/components/sqllab/DataPanel";

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

function renderPanel(dbId: string | null = null) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <StrictMode>
      <QueryClientProvider client={qc}>
        <DataPanel ws="dsl" dbId={dbId} onDb={vi.fn()} onInsert={vi.fn()} onWords={vi.fn()} onCollapse={vi.fn()} />
      </QueryClientProvider>
    </StrictMode>,
  );
}

beforeEach(() => fetchMock.mockReset());
afterEach(() => vi.clearAllMocks());

test("lists the redacted connections a non-admin member receives", async () => {
  // The non-admin shape: id, name, engine, delegated — no host or options.
  fetchMock.mockImplementation(async () =>
    json(200, [
      { id: "db-1", name: "trino-demo", engine: "trino", delegated: true },
      { id: "db-2", name: "sr-demo", engine: "starrocks", delegated: false },
    ]),
  );
  const user = userEvent.setup();
  renderPanel();

  await user.click(screen.getAllByRole("combobox")[0]);
  expect(await screen.findByRole("option", { name: /trino-demo \(trino\)/ })).toBeInTheDocument();
  expect(screen.getByRole("option", { name: /sr-demo \(starrocks\)/ })).toBeInTheDocument();
  expect(fetchMock.mock.calls[0][0]).toBe("/api/ws/dsl/databases");
});

test("says no connection is granted instead of an empty picker", async () => {
  fetchMock.mockImplementation(async () => json(200, []));
  renderPanel();

  const note = await screen.findByRole("status");
  expect(note).toHaveTextContent("No connection is granted to this workspace. Ask a workspace admin.");
  expect(screen.queryByText("Select a connection to browse its schema.")).not.toBeInTheDocument();
});

test("a refused listing says so, not an empty picker", async () => {
  fetchMock.mockImplementation(async () => json(403, { error: "forbidden" }));
  renderPanel();

  const alert = await screen.findByRole("alert");
  expect(alert).toHaveTextContent(
    "You can't list this workspace's connections. Ask a workspace admin for a role that can run queries.",
  );
  expect(screen.queryByText(/No connection is granted/)).not.toBeInTheDocument();
});

// KI-298: the tab's connection was removed or revoked; the panel said "This database".
test("says the tab's connection is gone, by the word Connection", async () => {
  fetchMock.mockImplementation(async () => json(200, [{ id: "db-1", name: "trino-demo", engine: "trino", delegated: true }]));
  renderPanel("db-gone");

  expect(await screen.findByText("This connection is no longer available.")).toBeInTheDocument();
});
