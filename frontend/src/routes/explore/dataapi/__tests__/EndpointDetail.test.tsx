import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// There is no msw in this repo — tests mock the API module directly (see
// Endpoints.test.tsx). Versions resolves a version's `created_by` to an email
// via `listUsers` (see Activity.test.tsx's equivalent mock for
// ActivityCallList's trial-actor column), so that module needs a mock.
vi.mock("@/api/generated/data-api/data-api");
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
import type { DataApiEndpointDto, DataApiEndpointVersionDto } from "@/api/generated/model";
import EndpointDetail from "@/routes/explore/dataapi/EndpointDetail";
import Config from "@/routes/explore/dataapi/detail/Config";
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

const updateMutateAsync = vi.fn();
const validateMutateAsync = vi.fn();

// Mounts Config at its real route (`/config`), not at the index — Query
// (Task 3) owns the index route and its own test file mounts it there. A
// bare `/explore/:workspace/data-api/endpoints/:id` entry with no trailing
// segment would hit neither route and render nothing.
function renderConfig(endpoint: DataApiEndpointDto = makeEndpoint()) {
  vi.mocked(api.useDataApiGetEndpoint).mockReturnValue(
    { data: { status: 200, data: endpoint }, isLoading: false, isError: false, error: null, refetch: vi.fn() } as unknown as
      ReturnType<typeof api.useDataApiGetEndpoint>,
  );

  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const utils = render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[`/explore/eng/data-api/endpoints/${endpoint.id}/config`]}>
        <Routes>
          <Route path="/explore/:workspace/data-api/endpoints/:id" element={<EndpointDetail />}>
            <Route path="config" element={<Config />} />
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return { ...utils, qc };
}

beforeEach(() => {
  vi.clearAllMocks();

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

describe("Config", () => {
  it("Config holds the endpoint's identity and its four config groups", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    renderConfig(endpoint);
    expect(await screen.findByLabelText("Name")).toBeInTheDocument();
    expect(screen.getByLabelText("Description")).toBeInTheDocument();
    expect(screen.getByLabelText("Method")).toBeInTheDocument();
    expect(screen.getByLabelText("Path")).toBeInTheDocument();
    expect(screen.getByText("Cache")).toBeInTheDocument();
    expect(screen.getByText("Limits")).toBeInTheDocument();
    expect(screen.getByText("Rate limit")).toBeInTheDocument();
  });

  it("Config does not hold the SQL editor — that is Query's", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    renderConfig(endpoint);
    await screen.findByLabelText("Name");
    expect(screen.queryByLabelText("SQL")).not.toBeInTheDocument();
  });

  // Reviewer finding: Config's subtitle once near-duplicated Query's ("The
  // query this endpoint serves, and how callers reach it."), contradicting
  // Config's own module doc that nothing here decides what the query
  // returns. Pin Config's own copy and pin the absence of Query's, so the
  // two sections' subtitles can't drift back into saying the same thing.
  it("has its own subtitle, not Query's", async () => {
    renderConfig();
    await screen.findByLabelText("Name");
    expect(
      screen.getByText("The endpoint as an HTTP object: the URL contract callers use, and the cache, limits, and rate-limit policy applied to their calls."),
    ).toBeInTheDocument();
    expect(
      screen.queryByText("The query this endpoint runs, and the parameters callers may pass it."),
    ).not.toBeInTheDocument();
  });

  // Spec §3.4: Validate runs the SAVED draft, not what's on screen — unlike
  // Preview. The label has to say so, or an author mid-edit reads "Validate"
  // as checking their current changes.
  it("labels Validate as checking the SAVED draft", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    renderConfig(endpoint);
    expect(await screen.findByRole("button", { name: /validate saved draft/i })).toBeInTheDocument();
  });

  // Phase 5 §5: the four groups used to be four `ConfigSection` cards, each
  // framing a table of one or two rows — a column of mostly-empty boxes. They
  // are now group rows of ONE table.
  //
  // Every assertion here has to die on a wrong implementation, which is why
  // this test does not stop at counting tables: the group rows are looked up
  // *inside* that table, and the four identity fields are located by row index
  // between the `Endpoint` group row and the `Cache` one. A `ConfigTableGroup`
  // that rendered nothing, a group row rendered outside the table, or the
  // identity form left sitting above the table all fail — and none of those
  // would disturb a bare table count.
  it("renders its groups as rows of one table, not as separate cards", async () => {
    const endpoint = makeEndpoint();
    vi.mocked(api.useDataApiGetEndpoint).mockReturnValue({
      data: { status: 200, data: endpoint },
      isLoading: false,
      isError: false,
      refetch: vi.fn(),
    } as never);
    renderConfig(endpoint);
    await screen.findByLabelText("Name");

    // Two tables would mean the cards were renamed rather than removed.
    expect(screen.getAllByRole("table")).toHaveLength(1);
    const table = screen.getByRole("table");
    const rows = within(table).getAllByRole("row");
    const rowIndexOf = (el: HTMLElement) => rows.indexOf(el.closest("tr") as HTMLElement);
    const groupAt = (name: string) => rowIndexOf(within(table).getByRole("rowheader", { name }));

    const endpointAt = groupAt("Endpoint");
    const cacheAt = groupAt("Cache");
    const limitsAt = groupAt("Limits");
    const rateLimitAt = groupAt("Rate limit");
    expect(endpointAt).toBeLessThan(cacheAt);
    expect(cacheAt).toBeLessThan(limitsAt);
    expect(limitsAt).toBeLessThan(rateLimitAt);

    // The identity fields are rows of that same table, under the `Endpoint`
    // group — not a form above it. This is the half of the change a table
    // count cannot see.
    for (const field of ["Name", "Description", "Method", "Path"]) {
      const at = rowIndexOf(within(table).getByLabelText(field));
      expect(at).toBeGreaterThan(endpointAt);
      expect(at).toBeLessThan(cacheAt);
    }
  });

  // Fix round 1, Minor 1: the reviewer replaced Name's `onChange` with
  // `() => {}` and the whole file stayed green — the four identity controls'
  // wiring was pinned by nothing, and this task rewrote the JSX around all
  // four of them. Four near-identical rows edited by hand is exactly how a
  // control ends up bound to its neighbour's field, so this drives all four
  // and asserts the WHOLE save payload with `toEqual`, not one key: a dead
  // `onChange` drops a key, and a cross-wired one puts the value under
  // someone else's. The values are deliberately distinct from each other and
  // from every default in `makeEndpoint`, so no swap can coincide.
  it("carries every identity field's edit to Save under its own key", async () => {
    const user = userEvent.setup();
    renderConfig();
    await screen.findByLabelText("Name");

    await user.clear(screen.getByLabelText("Name"));
    await user.type(screen.getByLabelText("Name"), "Weekly report");
    await user.clear(screen.getByLabelText("Description"));
    await user.type(screen.getByLabelText("Description"), "Rows for the weekly digest");
    await user.clear(screen.getByLabelText("Path"));
    await user.type(screen.getByLabelText("Path"), "reports/weekly");
    await user.click(screen.getByRole("combobox", { name: "Method" }));
    await user.click(await screen.findByRole("option", { name: "POST" }));

    await user.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    // The shell PATCHes only what changed (`diffToUpdateInput`), so these four
    // keys and nothing else is the exact contract of "four edits, four fields".
    expect(updateMutateAsync.mock.calls[0][0].data).toEqual({
      name: "Weekly report",
      description: "Rows for the weekly digest",
      method: "POST",
      path: "reports/weekly",
    });
  });
});

describe("EndpointDetail — save invalidation", () => {
  // Reviewer finding (Important #5): the generated `useDataApiUpdateEndpoint`
  // attaches no invalidation of its own, so a rename would leave the OLD name
  // in the header and the endpoints list stale, forever, until a hard reload.
  // This is section-agnostic (the mutation and its `onSuccess` live in the
  // shared `EndpointDetailInner` shell, not in any one section), so it is
  // exercised through Config rather than duplicated per section.
  it("invalidates the endpoint and list query caches after a successful save", async () => {
    const { qc } = renderConfig();
    await screen.findByLabelText("Name");
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

/**
 * Cache/Limits/Rate limit moved from bare Label+control pairs to the
 * `Setting | Details | Value` layout the control-plane engine config pages
 * use (`ConfigTable`/`ConfigTableRow`). This pins the one thing that layout
 * exists for — a Details explanation actually renders in the same row as its
 * control — so a future edit can't silently drop the `details` prop back to
 * nothing and leave the column blank again.
 */
describe("Config — config table Details column", () => {
  it("shows the Max rows Details copy in the same row as its control", async () => {
    renderConfig();
    const maxRows = await screen.findByLabelText("Max rows");
    const row = maxRows.closest("tr");
    expect(row).not.toBeNull();
    // The row renders the Details text twice (a `lg:table-cell` Details
    // column, and an `lg:hidden` inline copy for narrow screens) — jsdom
    // doesn't evaluate either media query, so both are present in the DOM;
    // asserting "at least one" is what "renders next to its control" means
    // here, matching how this codebase already asserts duplicated
    // responsive text elsewhere (see EngineDetail.test.tsx's `no delegation`
    // assertion).
    expect(
      within(row as HTMLElement).getAllByText(
        /hard cap on rows returned\. a result larger than this fails the call rather than being silently truncated/i,
      ).length,
    ).toBeGreaterThan(0);
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
    // Scoped to the error alert, not a bare `findByText`: the Naming row's
    // own Details copy ("...re-published") now also matches `/re-publish/i`
    // (rendered twice, per the Details-column note above), so an unscoped
    // query would fail with "multiple elements found" rather than pinning
    // the OpenAPI card's error message specifically.
    const alert = await screen.findByRole("alert");
    expect(within(alert).getByText(/re-publish/i)).toBeInTheDocument();
    expect(within(alert).getByText(/daily/)).toBeInTheDocument();
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

  // Shape's sibling. Two near-identical rows written by hand is exactly how a
  // control ends up bound to its neighbour's field (Task 2 hit this with four
  // of them), and this task rewrote the JSX around both. Asserting the WHOLE
  // PATCH body catches it: the shell sends only what changed, so a Naming
  // control wired to `shape` puts "camel" under `shape` and leaves `naming` at
  // its fixture value — a `naming: "camel"`-only assertion would miss that,
  // and a dead `onValueChange` drops the key entirely. The fixture's starting
  // values are distinct from the ones typed here so no swap can coincide.
  it("changes only the touched output field when the naming is switched, via a partial PATCH", async () => {
    vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
      { data: undefined, isError: false } as unknown as ReturnType<typeof api.useDataApiGetOpenapi>,
    );
    const endpoint = makeEndpoint({ output: { shape: "arrays", naming: "as_is" } });
    const user = userEvent.setup();
    renderResponse(endpoint);

    await user.click(screen.getByRole("combobox", { name: "Key naming" }));
    await user.click(await screen.findByRole("option", { name: "camelCase" }));
    await user.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() => expect(updateMutateAsync).toHaveBeenCalled());
    expect(updateMutateAsync.mock.calls[0][0].data).toEqual({
      output: { shape: "arrays", naming: "camel" },
    });
  });

  // Phase 5 §5, the Response half: "Output shape" used to be a `ConfigSection`
  // card framing a table of two rows. It is now a group ROW of that table.
  //
  // The two assertions on each block are deliberately opposite, so neither a
  // half-done conversion nor an over-eager one passes. `ConfigSection` renders
  // its title as an `<h3>`; `ConfigTableGroup` renders it as a `rowheader`
  // cell. So "Output shape" must be a rowheader and NOT a heading (a leftover
  // card fails), while "Frozen columns" and "OpenAPI document" must stay
  // headings and NOT be rowheaders (a later cleanup that swallows those two
  // real cards into the table fails). A group row that rendered nothing fails
  // the `getByRole("rowheader")` lookup, which is also the precondition the
  // row-order assertions below are computed from — they cannot pass without it.
  it("renders Output shape as a group row while the columns and document stay cards", async () => {
    vi.mocked(api.useDataApiGetOpenapi).mockReturnValue(
      { data: { status: 200, data: OPENAPI_FIXTURE }, isError: false } as unknown as
        ReturnType<typeof api.useDataApiGetOpenapi>,
    );
    // A live version so the Frozen columns card renders its own real `<table>`
    // — the harder case, and the one where scoping by "the table holding the
    // controls" rather than by a bare table count actually matters.
    vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
      { data: { status: 200, data: [makeVersion()] }, isLoading: false, isError: false } as unknown as
        ReturnType<typeof api.useDataApiListEndpointVersions>,
    );
    renderResponse(makeEndpoint({ live_version_id: "v-1" }));

    const shape = await screen.findByRole("combobox", { name: "Shape" });
    const configTable = shape.closest("table") as HTMLElement;
    expect(configTable).not.toBeNull();

    // The group heading is a row of the very table that holds the controls.
    const group = within(configTable).getByRole("rowheader", { name: "Output shape" });
    // ...and it spans the whole row. A group occupying only the Setting column
    // reads as a setting with no value — the confusion the cards were removed
    // to fix.
    expect(group).toHaveAttribute("colspan", "3");
    // No `ConfigSection` left behind: that would render "Output shape" as an h3.
    expect(screen.queryByRole("heading", { name: "Output shape" })).not.toBeInTheDocument();

    const rows = within(configTable).getAllByRole("row");
    const rowIndexOf = (el: HTMLElement) => rows.indexOf(el.closest("tr") as HTMLElement);
    const groupAt = rowIndexOf(group);
    for (const control of ["Shape", "Key naming"]) {
      const at = rowIndexOf(within(configTable).getByRole("combobox", { name: control }));
      expect(at).toBeGreaterThan(groupAt);
    }

    // These two are NOT `Setting | Details | Value` shaped — a frozen column
    // list and a download button — so they keep their cards.
    for (const card of ["Frozen columns", "OpenAPI document"]) {
      const title = screen.getByRole("heading", { name: card });
      expect(title.closest("table")).toBeNull();
      expect(screen.queryByRole("rowheader", { name: card })).not.toBeInTheDocument();
    }
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

  // Spec §3.4: Publish runs the SAVED draft, not what's on screen — unlike
  // Preview, the same distinction Config's "labels Validate as checking the
  // SAVED draft" test pins for Validate. Asserted here as its own test, not
  // only implicitly via the `{ name: "Publish saved draft" }` selector the
  // dirty-flag tests in `Query.test.tsx` already use to find this button —
  // that selector proves the label only as a side effect of finding
  // something to click.
  it("labels Publish as running the SAVED draft", async () => {
    renderVersions();
    expect(await screen.findByRole("button", { name: /publish saved draft/i })).toBeInTheDocument();
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
    renderConfig(makeEndpoint({ name: "Weekly rollup" }));
    const headings = await screen.findAllByRole("heading", { level: 1 });
    expect(headings).toHaveLength(1);
    expect(headings[0]).toHaveTextContent("Weekly rollup");
  });
});

/**
 * `max-w-3xl` on the `Outlet` wrapper is why the Details column wrapped to
 * three lines while the right third of the page sat empty.
 *
 * **The first assertion below is weak and is meant to be**: it pins a class
 * name, not a layout. It exists only so the cap cannot come back silently.
 * Whether the result is a good use of the width is a question only the browser
 * check can answer. The second assertion is the one with teeth — it says the
 * space the cap used to waste actually reaches the *controls*, not only the
 * muted Details prose that `table-fixed` would otherwise hand all of it to.
 */
describe("EndpointDetail — width", () => {
  it("does not cap the detail sections at max-w-3xl", async () => {
    const endpoint = makeEndpoint();
    const { container } = renderConfig(endpoint);
    await screen.findByLabelText("Max rows");
    expect(container.querySelector(".max-w-3xl")).toBeNull();
  });

  it("gives Config's Value column more than the shared default", async () => {
    renderConfig();
    await screen.findByLabelText("Max rows");
    const valueHead = screen.getByRole("columnheader", { name: "Value" });
    // Config owns the two widest controls on any detail tab (a 2-row
    // Description textarea and the Path input); at the shared `w-52` they show
    // roughly 50 and 25 characters. Removing the page cap does not help them —
    // under `table-fixed` the surplus goes only to the unsized Details column
    // — so this section opts into a wider one.
    expect(valueHead).toHaveClass("lg:w-96");
    // The `lg:` half is the whole point and jsdom cannot evaluate it, so this
    // second line is what stops a later "simplification" to a flat `w-96`:
    // below `lg` the sized columns do not shrink, they scroll, and a flat
    // `w-96` more than doubles that scroll on a narrow screen.
    expect(valueHead).toHaveClass("w-52");
  });
});
