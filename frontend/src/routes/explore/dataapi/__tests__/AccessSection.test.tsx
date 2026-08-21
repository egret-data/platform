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
import { expectSectionCapsItsOwnWidth } from "./sectionWidth";

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
/**
 * Second accounts for the two "acted on the row that was clicked" tests below.
 *
 * A fixture with one actionable row cannot tell "used this row's account id"
 * apart from "used `accounts[0].id`" — the trap that let a pre-existing test
 * survive writing every value to index 0, and that let a hard-coded
 * `remove(0)` pass a removal test in #529. Both tests act on the SECOND row.
 */
const SA_GRANTED_2 = makeAccount("sa-granted-2", "billing-bot");
const SA_NONE_2 = makeAccount("sa-none-2", "loading-bot");

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

    // Every assertion is scoped to its own row. A global `getByText` here would
    // die on "found multiple elements" the moment the two states collapse —
    // which reports a duplicate-badge problem rather than the state-collapse
    // problem, and stops the run before the revoke/membership assertions that
    // actually carry the requirement.
    const granted = await screen.findByTestId(`access-row-${SA_GRANTED.id}`);
    const inherited = screen.getByTestId(`access-row-${SA_INHERITED.id}`);

    expect(within(granted).getByText("Granted on this endpoint")).toBeInTheDocument();
    expect(within(inherited).getByText("Inherited from the workspace")).toBeInTheDocument();
    expect(within(inherited).queryByText("Granted on this endpoint")).not.toBeInTheDocument();

    // The inherited row must NOT offer a revoke that would do nothing.
    expect(within(inherited).queryByRole("button", { name: /revoke/i })).not.toBeInTheDocument();
    expect(within(inherited).getByRole("link", { name: /membership/i })).toBeInTheDocument();

    // …and the granted row must, because there it changes something.
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

  it("revokes the SECOND of two granted rows when it is the second one that is clicked", async () => {
    setup({
      accounts: [SA_GRANTED, SA_GRANTED_2],
      grants: [makeGrant(SA_GRANTED.id, SA_GRANTED.name), makeGrant(SA_GRANTED_2.id, SA_GRANTED_2.name)],
      canInvoke: {},
    });
    renderAccess();

    // Both rows offer Revoke, so "clicked the right one" is a real claim here
    // in a way it is not in the test above (where only one row had a button and
    // that row was also `accounts[0]`).
    const revokes = await screen.findAllByRole("button", { name: /revoke/i });
    expect(revokes).toHaveLength(2);

    const second = screen.getByTestId(`access-row-${SA_GRANTED_2.id}`);
    await userEvent.click(within(second).getByRole("button", { name: /revoke/i }));

    // The WHOLE call, deep-equalled: a mutant that also sent the endpoint's own
    // id as the principal, or dropped `workspace`, fails here rather than
    // slipping past a partial match.
    await waitFor(() => expect(revokeMutateAsync).toHaveBeenCalledTimes(1));
    expect(revokeMutateAsync).toHaveBeenCalledWith({
      workspace: "eng",
      id: "ep-1",
      serviceAccountId: SA_GRANTED_2.id,
    });
  });

  it("grants the SECOND of two ungranted rows when it is the second one that is clicked", async () => {
    setup({ accounts: [SA_NONE, SA_NONE_2], grants: [], canInvoke: {} });
    renderAccess();

    const grants = await screen.findAllByRole("button", { name: /grant invoke/i });
    expect(grants).toHaveLength(2);

    const second = screen.getByTestId(`access-row-${SA_NONE_2.id}`);
    await userEvent.click(within(second).getByRole("button", { name: /grant invoke/i }));

    await waitFor(() => expect(grantMutateAsync).toHaveBeenCalledTimes(1));
    expect(grantMutateAsync).toHaveBeenCalledWith({
      workspace: "eng",
      id: "ep-1",
      data: { service_account_id: SA_NONE_2.id },
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

  // Reviewer finding (Important #1). The unchecked state used to be discarded
  // on the GRANTED arm — three branches hard-coded `alsoInherited: false` — so
  // a granted row whose probe failed rendered the negative claim by OMISSION:
  // no caveat, a live Revoke button, nothing said. `can_invoke` propagates PDP
  // errors as errors by design (`invoke.rs`), so an account holding BOTH an
  // object-scoped grant and `ws_admin` would show a clean "Granted on this
  // endpoint", the admin would click Revoke, the grant would really be deleted,
  // and the account would keep invoking through the cascade — the same "clicked
  // revoke, nothing happened" surprise this section exists to prevent, reached
  // through the error path instead of the inherited one.
  it("warns that revoking may not remove access when a granted row's inherited check failed", async () => {
    setup({ accounts: [SA_GRANTED], grants: [makeGrant(SA_GRANTED.id, SA_GRANTED.name)], openapiError: true });
    renderAccess();

    const row = await screen.findByTestId(`access-row-${SA_GRANTED.id}`);
    expect(within(row).getByText("Granted on this endpoint")).toBeInTheDocument();
    // The point of the assertion: the row must not present Revoke as if it were
    // known to be the only thing holding the access up.
    expect(within(row).getByText(/could not check.*also reaches invoke.*may not remove access/i)).toBeInTheDocument();
    expect(within(row).getByRole("button", { name: /revoke/i })).toBeInTheDocument();
  });

  // Same discard, reached by the other unchecked path: granted on an endpoint
  // that is not live, where the probe never runs at all.
  it("warns that revoking may not remove access on a granted row of an endpoint that is not live", async () => {
    setup({ accounts: [SA_GRANTED], grants: [makeGrant(SA_GRANTED.id, SA_GRANTED.name)] });
    renderAccess(makeEndpoint({ state: "draft", live_version_id: null }));

    const row = await screen.findByTestId(`access-row-${SA_GRANTED.id}`);
    expect(within(row).getByText("Granted on this endpoint")).toBeInTheDocument();
    expect(within(row).getByText(/cannot check.*also reaches invoke.*may not remove access/i)).toBeInTheDocument();
  });

  // A disabled endpoint WAS published and then switched off, so "take effect
  // when it is published" states something false about it. No access claim
  // changes — only the explanation.
  it("does not tell a disabled endpoint to publish itself", async () => {
    setup({ accounts: [SA_NONE], grants: [] });
    renderAccess(makeEndpoint({ state: "disabled", live_version_id: "v-1" }));

    expect(await screen.findByText(/this endpoint is disabled/i)).toBeInTheDocument();
    expect(screen.getByText(/apply again when it is re-enabled/i)).toBeInTheDocument();
    expect(screen.queryByText(/take effect when it is published/i)).not.toBeInTheDocument();
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
  //
  // Asserting the ABSENCE of some invented verdict string would pass against an
  // empty component and against any wording change — it is unfalsifiable. What
  // is falsifiable: the Ranger line must be BYTE-IDENTICAL for two accounts
  // whose invoke status is opposite. Any code that let this line vary with
  // something egret knows — invoke status, the grant list, the SQL — breaks
  // this equality, and nothing else about the section does.
  it("says exactly the same thing about data access for accounts with opposite invoke status", async () => {
    setup({
      accounts: [SA_GRANTED, SA_NONE],
      grants: [makeGrant(SA_GRANTED.id, SA_GRANTED.name)],
      canInvoke: { [SA_GRANTED.id]: true },
    });
    renderAccess();

    const grantedRow = await screen.findByTestId(`access-row-${SA_GRANTED.id}`);
    const noneRow = screen.getByTestId(`access-row-${SA_NONE.id}`);

    // Guard the premise: if both rows had the same invoke status the equality
    // below would hold trivially.
    expect(within(grantedRow).getByText("Granted on this endpoint")).toBeInTheDocument();
    expect(within(noneRow).getByText("Not granted")).toBeInTheDocument();

    const dataText = (row: HTMLElement) =>
      within(row).getByText(/egret cannot check/i).parentElement?.textContent;

    expect(dataText(grantedRow)).toBe(dataText(noneRow));
    expect(dataText(grantedRow)).toMatch(/egret cannot check whether this account may read/i);
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

/**
 * The re-presentation itself (`docs/en/ui-conventions.md` §4). The account list
 * is list-shaped — one row per principal, unbounded — so it is a plain `Table`,
 * not the `Card` that framed it and not a `ConfigTable` (there is no Value to
 * edit, only a per-row action).
 *
 * What the columns buy: "Invoke" and "Data" used to be a `<dt>` repeated on
 * every row, which put each row's answer at a different vertical offset from
 * the one above it. Written once in the header, the answers line up down the
 * page — and comparing accounts is the entire job of this screen.
 */
describe("AccessSection — the shape", () => {
  it("names the two conditions once, as columns, with one row per account", async () => {
    setup({
      accounts: [SA_GRANTED, SA_NONE],
      grants: [makeGrant(SA_GRANTED.id, SA_GRANTED.name)],
      canInvoke: {},
    });
    renderAccess();

    expect((await screen.findAllByRole("columnheader")).map((h) => h.textContent)).toEqual([
      "Service account",
      "Invoke",
      "Data",
      // Named for a screen reader and hidden on screen — a visible heading over
      // one small button is louder than what it labels.
      "Actions",
    ]);

    // Positions, not merely presence: a header row that says "Data" over the
    // Invoke answer is a table that reads as wrong while every text assertion
    // in this file still passes.
    const cells = within(screen.getByTestId(`access-row-${SA_GRANTED.id}`)).getAllByRole("cell");
    expect(cells).toHaveLength(4);
    expect(cells[0]).toHaveTextContent(SA_GRANTED.name);
    expect(cells[1]).toHaveTextContent("Granted on this endpoint");
    expect(cells[2]).toHaveTextContent(/egret cannot check whether this account may read/i);
    expect(within(cells[3]).getByRole("button", { name: /revoke/i })).toBeInTheDocument();
  });

  // Every state is a row spanning the table now, so the columns are named
  // before there is an account to put under them. This is also the branch that
  // must never be reached by a FAILED fetch (§2.2) — an admin told their
  // workspace has no service accounts goes off to create one that exists.
  it("keeps the empty state inside the table, under the named columns", async () => {
    setup({ accounts: [], grants: [] });
    renderAccess();

    expect(await screen.findByText(/no service accounts yet/i)).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: "Invoke" })).toBeInTheDocument();
  });

  /**
   * Access had no width guard at all, and neither did `Try it` — which is how
   * `Try it` shipped a `max-w-5xl` that nothing was holding in place.
   * `docs/en/ui-conventions.md` §5.1 lists this section among the four that cap
   * themselves; the assertion lives in `sectionWidth.ts` so all five sections
   * make it the same way.
   */
  it("caps its own width, as §5.1 says this section does", async () => {
    setup({ accounts: [SA_GRANTED], grants: [] });
    const { container } = renderAccess();
    const heading = await screen.findByRole("heading", { name: "Access", level: 2 });
    expectSectionCapsItsOwnWidth(container, heading);
  });
});
