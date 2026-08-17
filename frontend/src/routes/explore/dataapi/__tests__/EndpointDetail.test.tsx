import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Link, MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// There is no msw in this repo — tests mock the API module directly (see
// Endpoints.test.tsx). Definition reads `useDatabases` for the Database
// picker unconditionally, so it needs a mock too or it fires a real,
// unhandled `fetch` in jsdom. Versions resolves a version's `created_by` to
// an email via `listUsers` (see Activity.test.tsx's equivalent mock for
// ActivityCallList's trial-actor column), so that module needs one too.
vi.mock("@/api/generated/data-api/data-api");
vi.mock("@/api/databases");
vi.mock("@/api/access");

// Spy on the real hook rather than replace it with a no-op: the test needs to
// see every call's argument, in order, to tell "armed once, on a real edit"
// apart from "armed on hydration" or "never armed at all".
const warnSpy = vi.fn();
vi.mock("@/lib/useUnsavedChangesWarning", () => ({
  useUnsavedChangesWarning: (when: boolean) => warnSpy(when),
}));

import * as api from "@/api/generated/data-api/data-api";
import * as accessApi from "@/api/access";
import * as databasesApi from "@/api/databases";
import { ParamLocation, ParamType, type DataApiEndpointDto, type DataApiEndpointVersionDto } from "@/api/generated/model";
import EndpointDetail from "@/routes/explore/dataapi/EndpointDetail";
import Definition from "@/routes/explore/dataapi/detail/Definition";
import ResponseSection from "@/routes/explore/dataapi/detail/ResponseSection";
import VersionsSection from "@/routes/explore/dataapi/detail/Versions";

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
  // Default: resolves with nobody, so the Who column falls back to a
  // truncated id everywhere that doesn't set up its own fixture.
  vi.mocked(accessApi.listUsers).mockResolvedValue([]);

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
    // The visible breadcrumb heading (`h2`), not `getByText` — the page also
    // carries an `sr-only` `h1` with the same name (see the "document
    // heading" describe block below), so a bare text match is now ambiguous.
    expect(screen.getByRole("heading", { level: 2, name: "Endpoint B" })).toBeInTheDocument();
    expect(screen.queryByDisplayValue("SELECT A")).not.toBeInTheDocument();
    expect(screen.queryByRole("heading", { level: 2, name: "Endpoint A" })).not.toBeInTheDocument();
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

function renderResponse(endpoint: DataApiEndpointDto = makeEndpoint()) {
  vi.mocked(api.useDataApiGetEndpoint).mockReturnValue(
    { data: { status: 200, data: endpoint }, isLoading: false, isError: false, error: null, refetch: vi.fn() } as unknown as
      ReturnType<typeof api.useDataApiGetEndpoint>,
  );

  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[`/explore/eng/data-api/endpoints/${endpoint.id}/response`]}>
        <Routes>
          <Route path="/explore/:workspace/data-api/endpoints/:id" element={<EndpointDetail />}>
            <Route path="response" element={<ResponseSection />} />
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

// `servers[0].url` already carries the workspace slug (P5a fixed the backend
// for exactly this — see `openapi_gen.rs`) — the fixture pins that shape so a
// regression that drops the slug or reconstructs the URL from parts shows up
// here rather than only in a live check.
const OPENAPI_FIXTURE = {
  openapi: "3.0.3",
  info: { title: "Data API", version: "1.0.0" },
  servers: [{ url: "https://egret.test/data-api/v1/analytics" }],
  paths: {},
};

function makeVersion(overrides: Partial<DataApiEndpointVersionDto> = {}): DataApiEndpointVersionDto {
  return {
    id: "v-1",
    version: 1,
    note: "",
    is_live: true,
    created_by: "user-1",
    created_at: "2026-08-01T00:00:00Z",
    snapshot: {
      sql: "SELECT 1",
      method: "GET",
      path: "reports/daily",
      database_id: "db-1",
      params: [],
      output: {},
      cache: {},
      limits: {},
      rate_limit: {},
      delegated: false,
      columns: [{ name: "total_revenue", ty: "bigint" }],
    },
    ...overrides,
  } as unknown as DataApiEndpointVersionDto;
}

describe("ResponseSection", () => {
  beforeEach(() => {
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      { data: { status: 200, data: [] }, isLoading: false, isError: false } as unknown as
        ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    vi.mocked(api.useDataApiListServiceAccounts).mockReturnValue(
      {
        data: {
          status: 200,
          data: [{ id: "sa-1", name: "svc", description: "", email: "svc@x", created_at: "2026-01-01T00:00:00Z" }],
        },
        isLoading: false,
        isError: false,
      } as unknown as ReturnType<typeof api.useDataApiListServiceAccounts>,
    );
  });

  it("shows the served base URL beside the download", async () => {
    vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
      { data: { status: 200, data: OPENAPI_FIXTURE }, isError: false } as unknown as
        ReturnType<typeof api.useDataApiGetOpenapi>,
    );
    renderResponse();
    // OPENAPI_FIXTURE.servers[0].url — the slug is part of it; see the backend note above.
    expect(await screen.findByText("https://egret.test/data-api/v1/analytics")).toBeInTheDocument();
  });

  it("names the endpoint and says re-publish when the document is refused", async () => {
    // apiFetch throws on non-2xx, so a 409 arrives as react-query's `error`,
    // an ApiError carrying `.status` — not as `data`.
    vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
      {
        data: undefined,
        isError: true,
        error: Object.assign(new Error("endpoint daily has an output naming collision; re-publish it"), { status: 409 }),
      } as unknown as ReturnType<typeof api.useDataApiGetOpenapi>,
    );
    renderResponse();
    expect(await screen.findByText(/re-publish/i)).toBeInTheDocument();
    expect(screen.getByText(/daily/)).toBeInTheDocument();
  });

  it("picks the is_live version's columns, not a superseded one's", async () => {
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      {
        data: {
          status: 200,
          data: [
            makeVersion({ id: "v-0", version: 1, is_live: false }),
            makeVersion({ id: "v-1", version: 2, is_live: true }),
          ],
        },
        isLoading: false,
        isError: false,
      } as unknown as ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
      { data: undefined, isError: false } as unknown as ReturnType<typeof api.useDataApiGetOpenapi>,
    );
    renderResponse(makeEndpoint({ live_version_id: "v-1" }));
    expect(await screen.findByText("total_revenue")).toBeInTheDocument();
    expect(screen.getByText("bigint")).toBeInTheDocument();
  });

  // A draft endpoint (never published) has no frozen columns yet — rendering
  // an empty table here would misrepresent "nothing published" as "publishes
  // zero columns", so this must be a distinct message, not an empty table.
  it("says there is no live version rather than showing the draft's columns", async () => {
    vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
      { data: undefined, isError: false } as unknown as ReturnType<typeof api.useDataApiGetOpenapi>,
    );
    renderResponse(makeEndpoint({ live_version_id: null, state: "draft" }));
    expect(await screen.findByText(/no live version/i)).toBeInTheDocument();
  });

  // Reviewer finding (Important #1): `columnsLoading` alone was provable only
  // by reading the code. A published endpoint whose versions fetch hasn't
  // landed yet must show a loading state, not "no live version yet" — the
  // exact conflation this test and the next one pin apart.
  it("shows a loading state, not the publish prompt, while a live endpoint's versions are still in flight", async () => {
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      { data: undefined, isLoading: true, isError: false } as unknown as
        ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
      { data: undefined, isError: false } as unknown as ReturnType<typeof api.useDataApiGetOpenapi>,
    );
    renderResponse(makeEndpoint({ live_version_id: "v-1" }));
    expect(await screen.findByText("Loading…")).toBeInTheDocument();
    expect(screen.queryByText(/no live version/i)).not.toBeInTheDocument();
  });

  // Reviewer finding (Important #1): react-query clears `isLoading` on ANY
  // settle, error included, so a versions-fetch failure for a LIVE endpoint
  // would otherwise fall into the same branch as "never published" — telling
  // an author who has already published to do the one thing they've done.
  it("reports the versions fetch failing as an error, not as never having been published", async () => {
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      {
        data: undefined,
        isLoading: false,
        isError: true,
        error: new Error("network error"),
      } as unknown as ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
      { data: undefined, isError: false } as unknown as ReturnType<typeof api.useDataApiGetOpenapi>,
    );
    renderResponse(makeEndpoint({ live_version_id: "v-1" }));
    expect(await screen.findByText(/could not load the published contract/i)).toBeInTheDocument();
    expect(screen.queryByText(/no live version/i)).not.toBeInTheDocument();
  });

  it("changes only the touched output field when the shape is switched, via a partial PATCH", async () => {
    vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
      { data: undefined, isError: false } as unknown as ReturnType<typeof api.useDataApiGetOpenapi>,
    );
    const endpoint = makeEndpoint({ output: { shape: "objects", naming: "snake" } });
    const user = userEvent.setup();
    renderResponse(endpoint);

    await user.click(screen.getByRole("combobox", { name: "Shape" }));
    await user.click(await screen.findByRole("option", { name: "Arrays" }));
    await user.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    const call = updateMutateAsync.mock.calls[0][0];
    // `naming` must survive untouched — rebuilding `output` from scratch
    // (rather than spreading the existing object) is exactly the mistake
    // Task 3's review flagged: it would silently drop or reorder fields no
    // control on this page even touched.
    expect(call.data).toEqual({ output: { shape: "arrays", naming: "snake" } });
  });
});

function renderVersions(
  endpoint: DataApiEndpointDto = makeEndpoint({ live_version_id: "v-2", state: "live" }),
  refetch: () => void = vi.fn(),
) {
  vi.mocked(api.useDataApiGetEndpoint).mockReturnValue(
    { data: { status: 200, data: endpoint }, isLoading: false, isError: false, error: null, refetch } as unknown as
      ReturnType<typeof api.useDataApiGetEndpoint>,
  );

  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[`/explore/eng/data-api/endpoints/${endpoint.id}/versions`]}>
        <Routes>
          <Route path="/explore/:workspace/data-api/endpoints/:id" element={<EndpointDetail />}>
            <Route path="versions" element={<VersionsSection />} />
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

const publishMutateAsync = vi.fn();
const rollbackMutateAsync = vi.fn();

describe("VersionsSection", () => {
  const versionsRefetchSpy = vi.fn();

  beforeEach(() => {
    versionsRefetchSpy.mockClear();
    publishMutateAsync.mockReset();
    rollbackMutateAsync.mockReset();

    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      { data: { status: 200, data: [] }, isLoading: false, isError: false } as unknown as
        ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    vi.mocked(api.useDataApiPublishEndpoint).mockReturnValue(
      { mutateAsync: publishMutateAsync, isPending: false } as unknown as
        ReturnType<typeof api.useDataApiPublishEndpoint>,
    );
    vi.mocked(api.useDataApiRollbackEndpoint).mockReturnValue(
      { mutateAsync: rollbackMutateAsync, isPending: false } as unknown as
        ReturnType<typeof api.useDataApiRollbackEndpoint>,
    );
    vi.mocked(api.getDataApiListEndpointVersionsQueryKey).mockImplementation(
      (ws, id) => ["dataApiListEndpointVersions", ws, id] as unknown as
        ReturnType<typeof api.getDataApiListEndpointVersionsQueryKey>,
    );

    publishMutateAsync.mockResolvedValue({
      status: 200,
      data: { version_id: "v-3", version: 3, state: "live" },
      headers: new Headers(),
    });
    rollbackMutateAsync.mockResolvedValue({
      status: 200,
      data: { version_id: "v-1", version: 1, state: "live" },
      headers: new Headers(),
    });
  });

  // The one thing this section must not get wrong (spec §6.3, second of its
  // three placements): this is the moment BEFORE the endpoint goes live, and
  // the one place the warning reaches an author who never opens Access. It
  // must not be a toast fired after the fact — Publish still has to be
  // confirmed once the warning is showing.
  it("warns that publishing does not grant data access before it publishes", async () => {
    renderVersions();
    await userEvent.click(await screen.findByRole("button", { name: /publish/i }));
    expect(await screen.findByText(/does not grant data access/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /publish/i })).toBeInTheDocument();
    expect(publishMutateAsync).not.toHaveBeenCalled();
  });

  it("lists each version with its note and who, and puts the live badge on the one live_version_id points at", async () => {
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      {
        data: {
          status: 200,
          data: [
            makeVersion({ id: "v-1", version: 1, is_live: false, note: "initial", created_by: "user-1" }),
            makeVersion({ id: "v-2", version: 2, is_live: true, note: "fixed a typo", created_by: "user-2" }),
          ],
        },
        isLoading: false,
        isError: false,
      } as unknown as ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    renderVersions();

    expect(await screen.findByText("fixed a typo")).toBeInTheDocument();
    expect(screen.getByText("initial")).toBeInTheDocument();
    expect(screen.getByText("user-2")).toBeInTheDocument();
    // Exactly one Live badge — on v-2, not on both rows.
    expect(screen.getAllByText("Live")).toHaveLength(1);
  });

  // The defect this whole section exists to fix: on the live cluster the Who
  // column showed a raw user UUID (`5513767f-8cdd-4e48-9cf3-c493a74b683e`),
  // exactly the class of bug the Roles page was fixed for once already. The
  // DTO only ever carries the bare id (see `DataApiEndpointVersionDto`), so
  // resolving it to a name is a client-side lookup, the same shape
  // `ActivityCallList.tsx` uses for its trial-actor column.
  it("resolves created_by to the publisher's email, not the raw user id", async () => {
    const PUBLISHER_ID = "5513767f-8cdd-4e48-9cf3-c493a74b683e";
    vi.mocked(accessApi.listUsers).mockResolvedValue([
      { id: PUBLISHER_ID, email: "priya@egret.local", name: "Priya" },
    ] as unknown as accessApi.AccessUser[]);
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      {
        data: {
          status: 200,
          data: [makeVersion({ id: "v-1", version: 1, is_live: true, created_by: PUBLISHER_ID })],
        },
        isLoading: false,
        isError: false,
      } as unknown as ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    renderVersions();

    expect(await screen.findByText("priya@egret.local")).toBeInTheDocument();
    expect(screen.queryByText(PUBLISHER_ID)).not.toBeInTheDocument();
  });

  // A viewer-level operator may not be permitted `listUsers` (it's a
  // platform-level call — see the component's comment). That failure must
  // degrade this ONE column to a truncated id, not take down a page that
  // otherwise has everything it needs to render.
  it("falls back to a truncated id, not blank or a page error, when the users lookup fails", async () => {
    const PUBLISHER_ID = "5513767f-8cdd-4e48-9cf3-c493a74b683e";
    vi.mocked(accessApi.listUsers).mockRejectedValue(new Error("forbidden"));
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      {
        data: {
          status: 200,
          data: [makeVersion({ id: "v-1", version: 1, is_live: true, note: "initial", created_by: PUBLISHER_ID })],
        },
        isLoading: false,
        isError: false,
      } as unknown as ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    renderVersions();

    // The rest of the table rendered fine — this isn't the page-level error.
    await screen.findByText("initial");
    expect(screen.getByText(PUBLISHER_ID.slice(0, 8))).toBeInTheDocument();
    expect(screen.queryByText(PUBLISHER_ID)).not.toBeInTheDocument();
  });

  it("publishes with an optional note and refetches the endpoint afterward", async () => {
    renderVersions(makeEndpoint({ live_version_id: "v-2", state: "live" }), versionsRefetchSpy);
    await userEvent.click(await screen.findByRole("button", { name: /publish/i }));
    await screen.findByText(/does not grant data access/i);

    await userEvent.type(screen.getByLabelText(/note/i), "shipping v3");
    await userEvent.click(screen.getByRole("button", { name: "Publish" }));

    await waitFor(() =>
      expect(publishMutateAsync).toHaveBeenCalledWith({
        workspace: "eng",
        id: "ep-1",
        data: { note: "shipping v3" },
      }),
    );
    // `refetch` exists for exactly this case — publish changes `state` and
    // `live_version_id` server-side, and the header/other sections read
    // those off `endpoint`, not off this section's own versions query.
    await waitFor(() => expect(versionsRefetchSpy).toHaveBeenCalled());
  });

  // `PublishEndpointInput` documents `{}` as a valid publish body — an empty
  // note must be OMITTED, not sent as `{ note: "" }`. Every other publish
  // test types a note first, so a mutant that always attaches `note: ""`
  // would survive them undetected.
  it("publishes with no note as an empty body, not as an empty-string note", async () => {
    renderVersions(makeEndpoint({ live_version_id: "v-2", state: "live" }), versionsRefetchSpy);
    await userEvent.click(await screen.findByRole("button", { name: /publish/i }));
    await screen.findByText(/does not grant data access/i);

    await userEvent.click(screen.getByRole("button", { name: "Publish" }));

    await waitFor(() =>
      expect(publishMutateAsync).toHaveBeenCalledWith({
        workspace: "eng",
        id: "ep-1",
        data: {},
      }),
    );
  });

  it("rolls back only after typing the confirming word, naming the version by id, and refetches the endpoint afterward", async () => {
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      {
        data: {
          status: 200,
          data: [
            makeVersion({ id: "v-1", version: 1, is_live: false }),
            makeVersion({ id: "v-2", version: 2, is_live: true }),
          ],
        },
        isLoading: false,
        isError: false,
      } as unknown as ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    renderVersions(makeEndpoint({ live_version_id: "v-2", state: "live" }), versionsRefetchSpy);

    await userEvent.click(await screen.findByRole("button", { name: /roll back/i }));
    // The shared dialog's confirm button must say what this action actually
    // does — rollback is reversible (publish again), unlike the component's
    // default "Delete" label, which every OTHER call site of
    // `ConfirmDeleteDialog` genuinely wants.
    const confirmButton = screen.getByRole("button", { name: "Roll back" });
    expect(confirmButton).toBeDisabled();
    expect(rollbackMutateAsync).not.toHaveBeenCalled();

    await userEvent.type(screen.getByPlaceholderText("rollback"), "rollback");
    await userEvent.click(confirmButton);

    await waitFor(() =>
      expect(rollbackMutateAsync).toHaveBeenCalledWith({
        workspace: "eng",
        id: "ep-1",
        data: { version_id: "v-1" },
      }),
    );
    await waitFor(() => expect(versionsRefetchSpy).toHaveBeenCalled());
  });

  // Same class of bug ResponseSection was fixed for: react-query clears
  // `isLoading` on any settle, error included, so a fetch failure must not
  // fall into the same branch as "nothing published yet".
  it("reports a versions-fetch failure as an error, not as never having been published", async () => {
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      {
        data: undefined,
        isLoading: false,
        isError: true,
        error: new Error("network error"),
      } as unknown as ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    renderVersions();
    expect(await screen.findByText(/could not load/i)).toBeInTheDocument();
    expect(screen.queryByText(/never been published/i)).not.toBeInTheDocument();
  });

  it("says the endpoint has never been published when the version list is genuinely empty", async () => {
    renderVersions();
    expect(await screen.findByText(/never been published/i)).toBeInTheDocument();
  });
});

/**
 * Final-review finding (Important #3): Publish ignored the shared `dirty` flag.
 *
 * The five sections share ONE form that survives section navigation
 * (`EndpointDetail` keys the `Outlet` by id, not by pathname), so an author
 * could edit SQL in Definition, walk to Versions with "Unsaved changes" still
 * in the header, and publish — freezing the SAVED draft while the dialog said
 * "Freezes the current draft", which they would read as the one on screen.
 * Definition already guards the far cheaper Validate this way; Publish changes
 * what every deployed consumer receives.
 *
 * This mounts Definition AND Versions under the real shell and NAVIGATES
 * between them, because the bug only exists across that navigation — a test
 * that mounted Versions alone could never see it.
 */
describe("VersionsSection — Publish and the shared dirty flag", () => {
  const publishAsync = vi.fn();

  beforeEach(() => {
    publishAsync.mockReset();
    publishAsync.mockResolvedValue({
      status: 200,
      data: { version_id: "v-3", version: 3, state: "live" },
      headers: new Headers(),
    });
    vi.mocked(api.useDataApiPublishEndpoint).mockReturnValue(
      { mutateAsync: publishAsync, isPending: false } as unknown as
        ReturnType<typeof api.useDataApiPublishEndpoint>,
    );
    vi.mocked(api.useDataApiRollbackEndpoint).mockReturnValue(
      { mutateAsync: vi.fn(), isPending: false } as unknown as
        ReturnType<typeof api.useDataApiRollbackEndpoint>,
    );
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      { data: { status: 200, data: [] }, isLoading: false, isError: false } as unknown as
        ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
  });

  function renderBoth(endpoint: DataApiEndpointDto = makeEndpoint({ live_version_id: "v-2", state: "live" })) {
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue(
      { data: { status: 200, data: endpoint }, isLoading: false, isError: false, error: null, refetch: vi.fn() } as unknown as
        ReturnType<typeof api.useDataApiGetEndpoint>,
    );
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    return render(
      <QueryClientProvider client={qc}>
        <MemoryRouter initialEntries={[`/explore/eng/data-api/endpoints/${endpoint.id}`]}>
          <Routes>
            <Route path="/explore/:workspace/data-api/endpoints/:id" element={<EndpointDetail />}>
              <Route index element={<Definition />} />
              <Route path="versions" element={<VersionsSection />} />
            </Route>
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>,
    );
  }

  it("refuses to publish an edit made in Definition and carried unsaved into Versions", async () => {
    renderBoth();
    const sql = await screen.findByDisplayValue("SELECT 1");
    await userEvent.type(sql, " -- unsaved");
    expect(screen.getByText("Unsaved changes")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("link", { name: "Versions" }));
    const publish = await screen.findByRole("button", { name: "Publish" });

    expect(publish).toBeDisabled();
    expect(publish).toHaveAttribute("title", expect.stringContaining("Save your changes"));

    // Not merely visually disabled: the confirmation never opens, so the
    // publish cannot be reached at all.
    await userEvent.click(publish);
    expect(screen.queryByText(/does not grant data access/i)).not.toBeInTheDocument();
    expect(publishAsync).not.toHaveBeenCalled();
  });

  it("publishes normally once there is nothing unsaved", async () => {
    renderBoth();
    await screen.findByDisplayValue("SELECT 1");
    await userEvent.click(screen.getByRole("link", { name: "Versions" }));

    const publish = await screen.findByRole("button", { name: "Publish" });
    expect(publish).toBeEnabled();
    await userEvent.click(publish);
    expect(await screen.findByText(/does not grant data access/i)).toBeInTheDocument();
  });

  // The dialog's own copy is part of the fix: "the current draft" is what the
  // author reads as "the one I am looking at".
  it("says the SAVED draft is what gets frozen", async () => {
    renderBoth();
    await screen.findByDisplayValue("SELECT 1");
    await userEvent.click(screen.getByRole("link", { name: "Versions" }));
    await userEvent.click(await screen.findByRole("button", { name: "Publish" }));
    expect(await screen.findByText(/freezes the saved draft/i)).toBeInTheDocument();
  });
});

/**
 * Final-review finding (#8, second half): `AllowedValuesField` holds its text
 * locally so a trailing comma is not eaten mid-typing — but the local copy
 * must not survive a Save. `EndpointDetail` re-seeds the whole form from the
 * SERVER RESPONSE, so a server-normalised `allowed` list has to reach the
 * field, and the field's stale text must never be pushed back over it.
 */
describe("ParamEditor — the allowed-values field resyncs from the server", () => {
  const ENUM_PARAM = {
    name: "region",
    type: ParamType.enum,
    location: ParamLocation.query,
    required: false,
    allowed: ["eu", "us"],
  };

  it("shows the server's normalised list after a Save, not the pre-save text", async () => {
    const endpoint = makeEndpoint({ params: [ENUM_PARAM] } as Partial<DataApiEndpointDto>);
    // The server normalises: it lower-cases and de-duplicates what was sent.
    updateMutateAsync.mockResolvedValue({
      status: 200,
      data: makeEndpoint({ params: [{ ...ENUM_PARAM, allowed: ["eu", "us", "apac"] }] } as Partial<DataApiEndpointDto>),
      headers: new Headers(),
    });
    renderDetail(endpoint);

    const allowed = await screen.findByLabelText("Allowed values");
    await userEvent.clear(allowed);
    await userEvent.type(allowed, "EU, US, APAC");
    await userEvent.tab();

    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());

    // What the server returned, not what was typed.
    await waitFor(() => expect(screen.getByLabelText("Allowed values")).toHaveValue("eu, us, apac"));
  });

  // The other half of the same defect: a focus/blur that typed nothing must
  // not `commit()` a stale local string back over the prop.
  it("commits nothing on a focus/blur that typed nothing", async () => {
    const endpoint = makeEndpoint({ params: [ENUM_PARAM] } as Partial<DataApiEndpointDto>);
    renderDetail(endpoint);

    const allowed = await screen.findByLabelText("Allowed values");
    await userEvent.click(allowed);
    await userEvent.tab();

    expect(screen.getByLabelText("Allowed values")).toHaveValue("eu, us");
    // Touching a field without changing it is not an edit.
    expect(screen.queryByText("Unsaved changes")).not.toBeInTheDocument();
  });
});

/**
 * This route used to sit inside `DataApiLayout`'s `<Outlet/>`, which supplied
 * the page's one document-outline `h1` ("Data API", text-2xl). Once the shell
 * was removed (Data API became a sidebar group instead of a shell page), the
 * three list pages (Endpoints/Clients/Activity) got their own `h1` back —
 * but this route was missed: its visible title is a breadcrumb-style `h2`
 * (the endpoint name), and nothing above it carries a level-1 heading, so a
 * screen-reader user landing here got no page-level heading at all and the
 * document's heading outline started at level 2. Fixed with an `sr-only` h1,
 * the same idiom `SqlLab.tsx`/`ChartExplore.tsx` use for a page whose visible
 * design has no room for one — this page already has its title on screen (the
 * `h2` breadcrumb), so a second *visible* h1 would read as two titles.
 */
describe("EndpointDetail — document heading", () => {
  it("exposes exactly one level-1 heading, naming the endpoint", async () => {
    renderDetail(makeEndpoint({ name: "Weekly rollup" }));
    const headings = await screen.findAllByRole("heading", { level: 1 });
    expect(headings).toHaveLength(1);
    expect(headings[0]).toHaveTextContent("Weekly rollup");
  });
});
