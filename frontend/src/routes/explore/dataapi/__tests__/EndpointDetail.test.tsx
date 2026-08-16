import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Link, MemoryRouter, Route, Routes } from "react-router-dom";
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
import { ParamLocation, ParamType, type DataApiEndpointDto } from "@/api/generated/model";
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
    { data: { status: 200, data: endpoint }, isLoading: false, isError: false, error: null, refetch: vi.fn() } as unknown as
      ReturnType<typeof api.useDataApiGetEndpoint>,
  );

  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const utils = render(
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
  return { ...utils, qc };
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

  // Automocking `vi.mock("@/api/generated/data-api/data-api")` turns these
  // plain query-key builders into `vi.fn()`s that return `undefined` — give
  // them a deterministic, argument-sensitive fake so a test can tell "the
  // right query was invalidated" from "invalidateQueries was called with
  // nothing distinguishable".
  vi.mocked(api.getDataApiGetEndpointQueryKey).mockImplementation(
    (ws, id) => ["dataApiGetEndpoint", ws, id] as unknown as ReturnType<typeof api.getDataApiGetEndpointQueryKey>,
  );
  vi.mocked(api.getDataApiListEndpointsQueryKey).mockImplementation(
    (ws) => ["dataApiListEndpoints", ws] as unknown as ReturnType<typeof api.getDataApiListEndpointsQueryKey>,
  );

  updateMutateAsync.mockImplementation(({ data }: { workspace: string; id: string; data: Partial<DataApiEndpointDto> }) =>
    Promise.resolve({ status: 200, data: { ...makeEndpoint(), ...data }, headers: new Headers() }));
  validateMutateAsync.mockResolvedValue({ status: 200, data: { columns: [] }, headers: new Headers() });
});

describe("EndpointDetail dirtiness", () => {
  it("arms the unsaved-changes warning only after a real change, and disarms it when reverted", async () => {
    renderDetail();
    const sql = await screen.findByDisplayValue("SELECT 1");
    // Radix hydration and controlled-input echo must not count as a change —
    // not even a transient one: a flag that latches `true` during hydration
    // and only "catches up" to `false` a render later would still show the
    // phantom dialog to anyone who closes the tab in that window, so the
    // stronger assertion is that `true` is never seen at all, not just that
    // the LAST call happens to be `false`.
    expect(warnSpy).not.toHaveBeenCalledWith(true);
    expect(warnSpy).toHaveBeenLastCalledWith(false);

    await userEvent.type(sql, " -- x");
    expect(warnSpy).toHaveBeenLastCalledWith(true);

    // The property that actually separates "compare to baseline" from "a
    // flag that only latches": reverting the edit back to the loaded value
    // must disarm the warning. A `touched` flag can never do this without
    // extra reset logic bolted on; the comparison gets it for free.
    await userEvent.type(sql, "{Backspace}".repeat(5));
    expect(sql).toHaveValue("SELECT 1");
    expect(warnSpy).toHaveBeenLastCalledWith(false);
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
      { data: { status: 200, data: { ...endpoint } }, isLoading: false, isError: false, error: null, refetch: vi.fn() } as unknown as
        ReturnType<typeof api.useDataApiGetEndpoint>,
    );
    // Force a re-render by re-rendering the same tree is not enough on its own
    // (React Testing Library doesn't re-run hooks without a state change in the
    // tree), so instead assert the value survived the mock swap plus a fresh
    // paint triggered by a further keystroke.
    await userEvent.type(sql, "y");
    expect(sql).toHaveValue("SELECT 1 -- xy");
  });

  // Reviewer finding (Critical #1): a bare "ran once" ref never re-arms when
  // only the `:id` route param changes, because React Router reconciles the
  // SAME `EndpointDetail` instance across `/endpoints/A` → `/endpoints/B`.
  // Without keying the hydration guard (and the nested Outlet) by id, this
  // would show B's name in the header over A's SQL/path/params/database —
  // and a subsequent Save would silently overwrite B's record with A's data.
  it("shows the newly-navigated endpoint's own fields, not the previous one's, after an in-app navigation", async () => {
    const epA = makeEndpoint({ id: "ep-a", name: "Endpoint A", sql: "SELECT A" });
    const epB = makeEndpoint({ id: "ep-b", name: "Endpoint B", sql: "SELECT B" });
    vi.mocked(api.useDataApiGetEndpoint).mockImplementation(((_ws: string, id: string) => ({
      data: { status: 200, data: id === "ep-a" ? epA : epB },
      isLoading: false,
      isError: false,
      error: null,
      refetch: vi.fn(),
    })) as unknown as typeof api.useDataApiGetEndpoint);

    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const user = userEvent.setup();
    render(
      <QueryClientProvider client={qc}>
        <MemoryRouter initialEntries={["/explore/eng/data-api/endpoints/ep-a"]}>
          <Routes>
            {/* The `Link` inside the SAME route element is what reproduces the
                bug: the route pattern (and so the rendered `EndpointDetail`
                element) never changes across this navigation, only `:id` does —
                matching what a future in-app link between endpoints (Tasks
                8/9) will look like. */}
            <Route
              path="/explore/:workspace/data-api/endpoints/:id"
              element={
                <>
                  <Link to="/explore/eng/data-api/endpoints/ep-b">Go to B</Link>
                  <EndpointDetail />
                </>
              }
            >
              <Route index element={<Definition />} />
            </Route>
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>,
    );

    await screen.findByDisplayValue("SELECT A");
    await user.click(screen.getByText("Go to B"));

    await waitFor(() => expect(screen.getByLabelText("SQL")).toHaveValue("SELECT B"));
    expect(screen.getByText("Endpoint B")).toBeInTheDocument();
    expect(screen.queryByDisplayValue("SELECT A")).not.toBeInTheDocument();
    expect(screen.queryByText("Endpoint A")).not.toBeInTheDocument();
  });

  it("does not offer a location control for parameters", async () => {
    const endpoint = makeEndpoint({
      params: [{ name: "region", type: ParamType.string, location: ParamLocation.query, required: false }],
    });
    renderDetail(endpoint);
    // Confirms ParamEditor actually rendered a row — otherwise this assertion
    // would pass trivially against an empty editor with nothing to check.
    await screen.findByRole("combobox", { name: "Parameter 1 type" });
    expect(screen.queryByText(/location/i)).not.toBeInTheDocument();
  });

  // Reviewer finding (Critical #2): the field used to be fully controlled —
  // `value={allowed.join(", ")}` with an `onChange` that immediately
  // re-split/trimmed/filtered — which silently ate a trailing comma on every
  // keystroke (existing "a", type "," → "a," → normalized straight back to
  // "a"), so a second value could never be started by typing.
  it("accepts a comma typed as a separate keystroke when building an enum's allowed values", async () => {
    const endpoint = makeEndpoint({
      params: [{ name: "region", type: ParamType.enum, location: ParamLocation.query, required: true, allowed: [] }],
    });
    renderDetail(endpoint);
    const allowed = await screen.findByLabelText("Allowed values");

    await userEvent.type(allowed, "a,b");
    expect(allowed).toHaveValue("a,b");

    // Clicking elsewhere blurs the field, committing the projected array —
    // and re-formats the display for a stray typo in spacing.
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    expect(allowed).toHaveValue("a, b");

    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    const call = updateMutateAsync.mock.calls[0][0];
    expect(call.data.params[0].allowed).toEqual(["a", "b"]);
  });

  it("saves only the changed fields, not the whole form (a partial PATCH)", async () => {
    const endpoint = makeEndpoint();
    renderDetail(endpoint);
    await userEvent.type(await screen.findByLabelText("SQL"), " -- x");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    const call = updateMutateAsync.mock.calls[0][0];
    expect(call).toMatchObject({ workspace: "eng", id: "ep-1" });
    // Reviewer finding (Important #6): `UpdateEndpointInput`'s own doc says an
    // absent field means "leave it alone" — a whole-document PATCH built from
    // a form hydrated once at mount would silently revert any change made
    // elsewhere since. Only `sql` changed, so only `sql` should be sent.
    expect(call.data).toEqual({ sql: "SELECT 1 -- x" });
  });

  // Reviewer finding (Important #5): the generated `useDataApiUpdateEndpoint`
  // attaches no invalidation of its own, so a rename would leave the OLD name
  // in the header and the endpoints list stale, forever, until a hard reload.
  it("invalidates the endpoint and list query caches after a successful save", async () => {
    const { qc } = renderDetail();
    await screen.findByDisplayValue("SELECT 1");
    const invalidateSpy = vi.spyOn(qc, "invalidateQueries");

    const opts = vi.mocked(api.useDataApiUpdateEndpoint).mock.calls.at(-1)?.[0];
    await opts?.mutation?.onSuccess?.(
      { status: 200, data: makeEndpoint(), headers: new Headers() } as never,
      { workspace: "eng", id: "ep-1", data: {} } as never,
      undefined as never,
      undefined as never,
    );

    expect(invalidateSpy).toHaveBeenCalledWith(
      expect.objectContaining({ queryKey: ["dataApiGetEndpoint", "eng", "ep-1"] }),
    );
    expect(invalidateSpy).toHaveBeenCalledWith(
      expect.objectContaining({ queryKey: ["dataApiListEndpoints", "eng"] }),
    );
  });
});
