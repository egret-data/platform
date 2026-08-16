import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// No msw in this repo — tests mock the API module directly (see
// Endpoints.test.tsx / EndpointDetail.test.tsx).
vi.mock("@/api/generated/data-api/data-api");

import * as api from "@/api/generated/data-api/data-api";
import type {
  DataApiEndpointDto,
  DataApiEndpointGrantDto,
  DataApiServiceAccountDto,
} from "@/api/generated/model";
import EndpointDetail from "@/routes/explore/dataapi/EndpointDetail";
import AccessSection from "@/routes/explore/dataapi/detail/AccessSection";

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
    // Live by default: the invoke probe (the generated OpenAPI document) only
    // describes LIVE endpoints, so a draft fixture would answer "no paths" for
    // every account and quietly turn every test below into a test of the
    // not-live branch.
    live_version_id: "v-1",
    state: "live",
    created_by: "user-1",
    created_at: "2026-08-01T00:00:00Z",
    updated_at: "2026-08-10T00:00:00Z",
    ...overrides,
  } as unknown as DataApiEndpointDto;
}

function makeAccount(id: string, name: string): DataApiServiceAccountDto {
  return { id, name, description: "", email: `${id}@svc.local`, created_at: "2026-01-01T00:00:00Z" };
}

function makeGrant(serviceAccountId: string, name: string): DataApiEndpointGrantDto {
  return {
    id: `g-${serviceAccountId}`,
    service_account_id: serviceAccountId,
    service_account_name: name,
    created_at: "2026-02-01T00:00:00Z",
  };
}

const SA_GRANTED = makeAccount("sa-granted", "reporting-bot");
const SA_INHERITED = makeAccount("sa-inherited", "admin-bot");
const SA_NONE = makeAccount("sa-none", "stranger-bot");

/** A generated document with one path — i.e. this account MAY invoke this endpoint. */
const DOC_CAN_INVOKE = {
  openapi: "3.0.3",
  servers: [{ url: "https://egret.test/data-api/v1/analytics" }],
  paths: { "/reports/daily": { get: { operationId: "dataApi_x" } } },
};
/** The same document for an account that may not: `paths` is empty, by construction. */
const DOC_CANNOT_INVOKE = {
  openapi: "3.0.3",
  servers: [{ url: "https://egret.test/data-api/v1/analytics" }],
  paths: {},
};

const revokeMutateAsync = vi.fn();
const grantMutateAsync = vi.fn();

/**
 * Wire the four queries this section reads.
 *
 * `canInvoke` is keyed by service-account id and stands in for the backend's
 * Cedar answer: `useDataApiGetOpenapi(ws, { service_account_id, endpoint_id })`
 * returns a document whose `paths` is non-empty exactly when that account may
 * invoke that endpoint — cascade included.
 */
function setup(opts: {
  accounts?: DataApiServiceAccountDto[];
  grants?: DataApiEndpointGrantDto[];
  canInvoke?: Record<string, boolean>;
  openapiError?: boolean;
}) {
  const { accounts = [], grants = [], canInvoke = {}, openapiError = false } = opts;

  vi.mocked(api.useDataApiListServiceAccounts).mockReturnValue(
    { data: { status: 200, data: accounts }, isLoading: false, isError: false } as unknown as
      ReturnType<typeof api.useDataApiListServiceAccounts>,
  );
  vi.mocked(api.useDataApiListEndpointGrants).mockReturnValue(
    { data: { status: 200, data: grants }, isLoading: false, isError: false } as unknown as
      ReturnType<typeof api.useDataApiListEndpointGrants>,
  );
  vi.mocked(api.useDataApiGetOpenapi).mockImplementation(((
    _ws: string,
    params: { service_account_id: string },
    options?: { query?: { enabled?: boolean } },
  ) => {
    if (options?.query?.enabled === false) {
      return { data: undefined, isLoading: false, isError: false, error: null };
    }
    if (openapiError) {
      return { data: undefined, isLoading: false, isError: true, error: new Error("boom") };
    }
    return {
      data: {
        status: 200,
        data: canInvoke[params.service_account_id] ? DOC_CAN_INVOKE : DOC_CANNOT_INVOKE,
      },
      isLoading: false,
      isError: false,
      error: null,
    };
  }) as unknown as typeof api.useDataApiGetOpenapi);
}

function renderAccess(endpoint: DataApiEndpointDto = makeEndpoint()) {
  vi.mocked(api.useDataApiGetEndpoint).mockReturnValue(
    { data: { status: 200, data: endpoint }, isLoading: false, isError: false, error: null, refetch: vi.fn() } as unknown as
      ReturnType<typeof api.useDataApiGetEndpoint>,
  );

  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const utils = render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[`/explore/eng/data-api/endpoints/${endpoint.id}/access`]}>
        <Routes>
          <Route path="/explore/:workspace/data-api/endpoints/:id" element={<EndpointDetail />}>
            <Route path="access" element={<AccessSection />} />
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return { ...utils, qc };
}

beforeEach(() => {
  vi.clearAllMocks();

  vi.mocked(api.useDataApiUpdateEndpoint).mockReturnValue(
    { mutateAsync: vi.fn(), isPending: false } as unknown as ReturnType<typeof api.useDataApiUpdateEndpoint>,
  );
  vi.mocked(api.useDataApiRevokeEndpointInvoke).mockReturnValue(
    { mutateAsync: revokeMutateAsync, isPending: false } as unknown as ReturnType<typeof api.useDataApiRevokeEndpointInvoke>,
  );
  vi.mocked(api.useDataApiGrantEndpointInvoke).mockReturnValue(
    { mutateAsync: grantMutateAsync, isPending: false } as unknown as ReturnType<typeof api.useDataApiGrantEndpointInvoke>,
  );

  // Automocking turns the query-key builders into `vi.fn()`s returning
  // `undefined`; give them argument-sensitive fakes so "the right query was
  // invalidated" is distinguishable from "invalidateQueries was called".
  vi.mocked(api.getDataApiListEndpointGrantsQueryKey).mockImplementation(
    (ws, id) => ["grants", ws, id] as unknown as ReturnType<typeof api.getDataApiListEndpointGrantsQueryKey>,
  );
  vi.mocked(api.getDataApiGetOpenapiQueryKey).mockImplementation(
    (ws) => ["openapi", ws] as unknown as ReturnType<typeof api.getDataApiGetOpenapiQueryKey>,
  );

  revokeMutateAsync.mockResolvedValue({ status: 200, data: { ok: true }, headers: new Headers() });
  grantMutateAsync.mockResolvedValue({ status: 200, data: makeGrant("sa-none", "stranger-bot"), headers: new Headers() });
});

describe("AccessSection — condition 1, invoke", () => {
  // THE test this section exists for. Collapsing the two states into one ✓ with
  // a Revoke button produces: the user clicks revoke, the backend deletes a
  // grant that does not exist, the ✓ remains, and the product looks broken
  // while behaving correctly.
  it("distinguishes a grant on this endpoint from one inherited from the workspace", async () => {
    setup({
      accounts: [SA_GRANTED, SA_INHERITED],
      grants: [makeGrant(SA_GRANTED.id, SA_GRANTED.name)],
      // The inherited account owns no `access_grants` row for this endpoint —
      // it reaches invoke through a ws_admin membership and the Cedar
      // workspace→object cascade, which only the backend can see.
      canInvoke: { [SA_INHERITED.id]: true },
    });
    renderAccess();

    expect(await screen.findByText("Granted on this endpoint")).toBeInTheDocument();
    expect(screen.getByText("Inherited from the workspace")).toBeInTheDocument();

    // The inherited row must NOT offer a revoke that would do nothing.
    const inherited = screen.getByTestId(`access-row-${SA_INHERITED.id}`);
    expect(within(inherited).queryByRole("button", { name: /revoke/i })).not.toBeInTheDocument();
    expect(within(inherited).getByRole("link", { name: /membership/i })).toBeInTheDocument();

    // …and the granted row must, because there it changes something.
    const granted = screen.getByTestId(`access-row-${SA_GRANTED.id}`);
    expect(within(granted).getByRole("button", { name: /revoke/i })).toBeInTheDocument();
  });

  it("revokes the object-scoped grant for the account whose row was clicked", async () => {
    setup({
      accounts: [SA_GRANTED, SA_INHERITED],
      grants: [makeGrant(SA_GRANTED.id, SA_GRANTED.name)],
      canInvoke: { [SA_INHERITED.id]: true },
    });
    renderAccess();

    const granted = await screen.findByTestId(`access-row-${SA_GRANTED.id}`);
    await userEvent.click(within(granted).getByRole("button", { name: /revoke/i }));

    await waitFor(() => expect(revokeMutateAsync).toHaveBeenCalled());
    expect(revokeMutateAsync).toHaveBeenCalledWith({
      workspace: "eng",
      id: "ep-1",
      serviceAccountId: SA_GRANTED.id,
    });
  });

  it("offers Grant for an account with neither a grant here nor inherited access", async () => {
    setup({ accounts: [SA_NONE], grants: [], canInvoke: {} });
    renderAccess();

    const row = await screen.findByTestId(`access-row-${SA_NONE.id}`);
    expect(within(row).getByText("Not granted")).toBeInTheDocument();
    await userEvent.click(within(row).getByRole("button", { name: /grant/i }));

    await waitFor(() => expect(grantMutateAsync).toHaveBeenCalled());
    expect(grantMutateAsync).toHaveBeenCalledWith({
      workspace: "eng",
      id: "ep-1",
      data: { service_account_id: SA_NONE.id },
    });
  });

  // An account can hold BOTH — an explicit grant here AND a ws_admin
  // membership. Revoking here is a real change (the row goes away) but does
  // NOT remove access, which is the same "clicked revoke, nothing happened"
  // surprise in a different disguise, so the row has to say so up front.
  it("warns when an account is granted here AND also reaches invoke through the workspace", async () => {
    setup({
      accounts: [SA_GRANTED],
      grants: [makeGrant(SA_GRANTED.id, SA_GRANTED.name)],
      canInvoke: { [SA_GRANTED.id]: true },
    });
    renderAccess();

    const row = await screen.findByTestId(`access-row-${SA_GRANTED.id}`);
    expect(within(row).getByText("Granted on this endpoint")).toBeInTheDocument();
    expect(within(row).getByText(/also reaches invoke through the workspace/i)).toBeInTheDocument();
    expect(within(row).getByRole("button", { name: /revoke/i })).toBeInTheDocument();
  });

  // The probe only describes LIVE endpoints, so on a draft it answers "no
  // paths" for everyone. Reporting that as "Not granted" would state a fact
  // egret has not established — the same class of lie as inferring Ranger.
  it("says inherited access cannot be checked on an endpoint that is not live, instead of guessing", async () => {
    setup({ accounts: [SA_NONE], grants: [], canInvoke: {} });
    renderAccess(makeEndpoint({ state: "draft", live_version_id: null }));

    expect(await screen.findByText(/cannot check workspace-inherited/i)).toBeInTheDocument();
    const row = screen.getByTestId(`access-row-${SA_NONE.id}`);
    expect(within(row).queryByText("Not granted")).not.toBeInTheDocument();
    expect(within(row).getByText("No grant on this endpoint")).toBeInTheDocument();
  });

  // Driving the rows purely from the service-account list makes a grant whose
  // principal is no longer listed INVISIBLE — while it is still in
  // `access_grants` and still consulted by Cedar. `/grants` returns
  // `service_account_name: null` for exactly this case, so it is an expected
  // shape, not a defensive one.
  it("still shows — and can revoke — a grant whose service account is no longer listed", async () => {
    setup({
      accounts: [SA_GRANTED],
      grants: [
        makeGrant(SA_GRANTED.id, SA_GRANTED.name),
        { id: "g-ghost", service_account_id: "sa-ghost", service_account_name: null, created_at: "2026-02-01T00:00:00Z" },
      ],
      canInvoke: { [SA_GRANTED.id]: true },
    });
    renderAccess();

    const ghost = await screen.findByTestId("access-row-sa-ghost");
    expect(within(ghost).getByText("Granted on this endpoint")).toBeInTheDocument();
    await userEvent.click(within(ghost).getByRole("button", { name: /revoke/i }));
    await waitFor(() => expect(revokeMutateAsync).toHaveBeenCalled());
    expect(revokeMutateAsync).toHaveBeenCalledWith({
      workspace: "eng",
      id: "ep-1",
      serviceAccountId: "sa-ghost",
    });
  });

  it("does not claim an account lacks access when the invoke check itself failed", async () => {
    setup({ accounts: [SA_NONE], grants: [], openapiError: true });
    renderAccess();

    const row = await screen.findByTestId(`access-row-${SA_NONE.id}`);
    expect(within(row).queryByText("Not granted")).not.toBeInTheDocument();
    expect(within(row).getByText(/could not check/i)).toBeInTheDocument();
  });
});

describe("AccessSection — condition 2, Ranger", () => {
  it("says egret cannot check Ranger, and offers Try it", async () => {
    setup({ accounts: [SA_GRANTED], grants: [makeGrant(SA_GRANTED.id, SA_GRANTED.name)] });
    renderAccess();

    expect(await screen.findByText(/egret cannot check/i)).toBeInTheDocument();
    expect(screen.getAllByRole("link", { name: /try it/i }).length).toBeGreaterThan(0);
  });

  it("preselects the row's own service account in the Try-it link", async () => {
    setup({
      accounts: [SA_GRANTED, SA_INHERITED],
      grants: [makeGrant(SA_GRANTED.id, SA_GRANTED.name)],
      canInvoke: { [SA_INHERITED.id]: true },
    });
    renderAccess();

    const row = await screen.findByTestId(`access-row-${SA_INHERITED.id}`);
    expect(within(row).getByRole("link", { name: /try it/i })).toHaveAttribute(
      "href",
      `/explore/eng/data-api/endpoints/ep-1/try?service_account=${SA_INHERITED.id}`,
    );
  });

  // The one thing this section must never do: read the SQL and project Ranger
  // policy onto it. A half-right answer here is worse than none.
  it("never presents a Ranger verdict", async () => {
    setup({
      accounts: [SA_GRANTED],
      grants: [makeGrant(SA_GRANTED.id, SA_GRANTED.name)],
      canInvoke: { [SA_GRANTED.id]: true },
    });
    renderAccess();

    await screen.findByTestId(`access-row-${SA_GRANTED.id}`);
    expect(screen.queryByText(/ranger: (allowed|denied|ok)/i)).not.toBeInTheDocument();
  });
});

describe("AccessSection — the §5.3 warning", () => {
  it("states that publishing does not grant data access, as a neutral note rather than amber emphasis", async () => {
    setup({ accounts: [SA_GRANTED], grants: [] });
    renderAccess();

    const note = await screen.findByRole("note");
    expect(note).toHaveTextContent(/publishing.*does not grant data access/i);
    // SettingsNote's neutral surface. `--highlight` (amber) is reserved for a
    // page's single primary emphasis, and a standing passive note is not that.
    expect(note.className).toContain("bg-muted/40");
    expect(note.className).not.toContain("highlight");
  });
});
