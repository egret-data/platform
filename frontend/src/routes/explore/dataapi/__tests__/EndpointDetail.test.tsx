import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// There is no msw in this repo — tests mock the API module directly (see
// Endpoints.test.tsx). Definition reads `useDatabases` for the Database
// picker unconditionally, so it needs a mock too or it fires a real,
// unhandled `fetch` in jsdom.
vi.mock("@/api/generated/data-api/data-api");
vi.mock("@/api/databases");

// Spy on the real hook rather than replace it with a no-op: the test needs to
// see every call's argument, in order, to tell "armed once, on a real edit"
// apart from "armed on hydration" or "never armed at all".
const warnSpy = vi.fn();
vi.mock("@/lib/useUnsavedChangesWarning", () => ({
  useUnsavedChangesWarning: (when: boolean) => warnSpy(when),
}));

import * as api from "@/api/generated/data-api/data-api";
import * as databasesApi from "@/api/databases";
import type { DataApiEndpointDto } from "@/api/generated/model";
import EndpointDetail from "@/routes/explore/dataapi/EndpointDetail";
import Definition from "@/routes/explore/dataapi/detail/Definition";

function makeEndpoint(overrides: Partial<DataApiEndpointDto> = {}): DataApiEndpointDto {
  return {
    id: "ep-1",
    folder_id: null,
    name: "Daily report",
    description: "",
    path: "reports/daily",
    method: "GET",
    database_id: "db-1",
    sql: "SELECT 1",
    params: [],
    output: {},
    cache: {},
    limits: {},
    rate_limit: {},
    source_kind: "sql",
    source_ref: null,
    live_version_id: null,
    state: "draft",
    created_by: "user-1",
    created_at: "2026-08-01T00:00:00Z",
    updated_at: "2026-08-10T00:00:00Z",
    ...overrides,
  } as unknown as DataApiEndpointDto;
}

const DB_TRINO = { id: "db-1", name: "warehouse", engine: "trino", host: "h", port: 1, options: {}, has_secret: true, status: "ok", delegated: false };

const updateMutateAsync = vi.fn();
const validateMutateAsync = vi.fn();

function renderDetail(endpoint: DataApiEndpointDto = makeEndpoint()) {
  vi.mocked(api.useDataApiGetEndpoint).mockReturnValue(
    { data: { status: 200, data: endpoint }, isLoading: false, isError: false, error: null } as unknown as
      ReturnType<typeof api.useDataApiGetEndpoint>,
  );

  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[`/explore/eng/data-api/endpoints/${endpoint.id}`]}>
        <Routes>
          <Route path="/explore/:workspace/data-api/endpoints/:id" element={<EndpointDetail />}>
            <Route index element={<Definition />} />
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();

  vi.mocked(databasesApi.useDatabases).mockReturnValue(
    { data: [DB_TRINO], isLoading: false, isError: false } as unknown as ReturnType<typeof databasesApi.useDatabases>,
  );

  vi.mocked(api.useDataApiUpdateEndpoint).mockReturnValue(
    { mutateAsync: updateMutateAsync, isPending: false } as unknown as ReturnType<typeof api.useDataApiUpdateEndpoint>,
  );
  vi.mocked(api.useDataApiValidateEndpoint).mockReturnValue(
    { mutateAsync: validateMutateAsync, isPending: false } as unknown as ReturnType<typeof api.useDataApiValidateEndpoint>,
  );

  updateMutateAsync.mockImplementation(({ data }: { workspace: string; id: string; data: Partial<DataApiEndpointDto> }) =>
    Promise.resolve({ status: 200, data: { ...makeEndpoint(), ...data }, headers: new Headers() }));
  validateMutateAsync.mockResolvedValue({ status: 200, data: { columns: [] }, headers: new Headers() });
});

describe("EndpointDetail dirtiness", () => {
  it("arms the unsaved-changes warning only after a real change", async () => {
    renderDetail();
    await screen.findByDisplayValue("SELECT 1");
    // Radix hydration and controlled-input echo must not count as a change.
    expect(warnSpy).toHaveBeenLastCalledWith(false);
    await userEvent.type(screen.getByLabelText("SQL"), " -- x");
    expect(warnSpy).toHaveBeenLastCalledWith(true);
  });

  it("re-baselines after a successful save", async () => {
    renderDetail();
    await userEvent.type(await screen.findByLabelText("SQL"), " -- x");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(warnSpy).toHaveBeenLastCalledWith(false));
  });

  // Pins the hydrate-once guard: ChartExplore and NotebookSettings have both
  // shipped a regression where re-seeding the form on every data reference
  // (rather than once) clobbers a mid-edit form the instant react-query hands
  // back a new object identity for unchanged data (e.g. a background refetch).
  it("does not clobber a mid-edit field when the query result gets a new object identity", async () => {
    const endpoint = makeEndpoint();
    renderDetail(endpoint);
    const sql = await screen.findByLabelText("SQL");
    await userEvent.type(sql, " -- x");
    expect(sql).toHaveValue("SELECT 1 -- x");

    // Simulate a react-query refetch that resolves to an equal-but-new object.
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue(
      { data: { status: 200, data: { ...endpoint } }, isLoading: false, isError: false, error: null } as unknown as
        ReturnType<typeof api.useDataApiGetEndpoint>,
    );
    // Force a re-render by re-rendering the same tree is not enough on its own
    // (React Testing Library doesn't re-run hooks without a state change in the
    // tree), so instead assert the value survived the mock swap plus a fresh
    // paint triggered by a further keystroke.
    await userEvent.type(sql, "y");
    expect(sql).toHaveValue("SELECT 1 -- xy");
  });

  it("does not offer a location control for parameters", async () => {
    renderDetail();
    await screen.findByDisplayValue("SELECT 1");
    expect(screen.queryByText(/location/i)).not.toBeInTheDocument();
  });

  it("saves the mapped form fields, not the read-only DTO fields", async () => {
    const endpoint = makeEndpoint();
    renderDetail(endpoint);
    await userEvent.type(await screen.findByLabelText("SQL"), " -- x");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    const call = updateMutateAsync.mock.calls[0][0];
    expect(call).toMatchObject({ workspace: "eng", id: "ep-1" });
    expect(call.data).not.toHaveProperty("id");
    expect(call.data).not.toHaveProperty("state");
    expect(call.data).not.toHaveProperty("live_version_id");
    expect(call.data.sql).toBe("SELECT 1 -- x");
  });
});
