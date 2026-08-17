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
  it("Config holds the endpoint's identity and its three config tables", async () => {
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
