import { describe, it, expect, vi, beforeEach } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
  // The shell renders a Restore button in its trashed banner, so a TRASHED
  // fixture — which the caveat tests need, to prove the `<fieldset disabled>`
  // is really in play — crashes the whole tree without this. Automocking
  // returns `undefined`, not a stub.
  vi.mocked(api.useDataApiRestoreEndpoint).mockReturnValue(
    { mutateAsync: vi.fn(), isPending: false } as unknown as ReturnType<typeof api.useDataApiRestoreEndpoint>,
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

  /**
   * The one thing this section must never do: read the SQL and project Ranger
   * policy onto it. A half-right answer here is worse than none.
   *
   * The previous guard asserted the Ranger line was BYTE-IDENTICAL across two
   * rows with opposite invoke status. That was the right property and the wrong
   * shape — it accepted the sentence being repeated per row, which the browser
   * pass then measured as 288px of column spent saying one thing N times (two
   * accounts filled ~382px of a 786px box).
   *
   * Said ONCE, outside the row loop, the property is not merely asserted but
   * unrepresentable: there is no per-row render left that could vary with
   * invoke status, the grant list, or the SQL. This test is what pins the
   * "once" — and it is falsifiable in the direction that matters, because
   * putting the sentence back in the row makes the count N.
   */
  it("says the Ranger unknown exactly once, not once per account", async () => {
    setup({
      accounts: [SA_GRANTED, SA_NONE],
      grants: [makeGrant(SA_GRANTED.id, SA_GRANTED.name)],
      canInvoke: { [SA_GRANTED.id]: true },
    });
    renderAccess();

    // Guard the premise: two accounts, and their invoke status is OPPOSITE. A
    // one-account fixture would make "once" true for the wrong reason, and two
    // rows in the same state would not exercise the thing the old test was
    // written to catch.
    expect(await screen.findByText("Granted on this endpoint")).toBeInTheDocument();
    expect(screen.getByText("Not granted")).toBeInTheDocument();
    expect(screen.getAllByTestId(/^access-row-/)).toHaveLength(2);

    expect(screen.getAllByText(/egret cannot check whether these accounts may read/i)).toHaveLength(1);

    // …and it is above the table, not inside any row — which is what makes the
    // count independent of how many accounts there are.
    const sentence = screen.getByText(/egret cannot check whether these accounts may read/i);
    expect(sentence.closest("table")).toBeNull();
    expect(sentence.closest("[data-testid^=\"access-row-\"]")).toBeNull();
  });

  // The per-row half of condition 2 is the only part that was ever per-row: the
  // trial has to run as THIS account or it answers a different question.
  it("still offers a per-row Try it, preselected to that row's account", async () => {
    setup({
      accounts: [SA_GRANTED, SA_NONE],
      grants: [makeGrant(SA_GRANTED.id, SA_GRANTED.name)],
      canInvoke: {},
    });
    renderAccess();

    const first = await screen.findByTestId(`access-row-${SA_GRANTED.id}`);
    const second = screen.getByTestId(`access-row-${SA_NONE.id}`);
    // Two rows, two DIFFERENT hrefs — a link hard-coded to `accounts[0]` reads
    // identically on the page and answers the wrong account's question.
    expect(within(first).getByRole("link", { name: /try it/i })).toHaveAttribute(
      "href",
      `/explore/eng/data-api/endpoints/ep-1/try?service_account=${SA_GRANTED.id}`,
    );
    expect(within(second).getByRole("link", { name: /try it/i })).toHaveAttribute(
      "href",
      `/explore/eng/data-api/endpoints/ep-1/try?service_account=${SA_NONE.id}`,
    );
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
  it("names the one per-row condition as a column, with one row per account", async () => {
    setup({
      accounts: [SA_GRANTED, SA_NONE],
      grants: [makeGrant(SA_GRANTED.id, SA_GRANTED.name)],
      canInvoke: {},
    });
    renderAccess();

    // THREE columns, not four. `Data` was one and every cell in it held the
    // same sentence; the browser pass measured what that cost (see the
    // component's doc) and it is section-level copy now.
    expect((await screen.findAllByRole("columnheader")).map((h) => h.textContent)).toEqual([
      "Service account",
      "Invoke",
      // Named for a screen reader and hidden on screen — a visible heading over
      // one small button is louder than what it labels.
      "Actions",
    ]);

    // Positions, not merely presence: a header row that says "Invoke" over the
    // account name is a table that reads as wrong while every text assertion in
    // this file still passes.
    const cells = within(screen.getByTestId(`access-row-${SA_GRANTED.id}`)).getAllByRole("cell");
    expect(cells).toHaveLength(3);
    expect(cells[0]).toHaveTextContent(SA_GRANTED.name);
    expect(cells[1]).toHaveTextContent("Granted on this endpoint");
    // Both per-row actions share the last cell — the mutation and the link that
    // answers the condition that is no longer a column.
    expect(within(cells[2]).getByRole("button", { name: /revoke/i })).toBeInTheDocument();
    expect(within(cells[2]).getByRole("link", { name: /try it/i })).toBeInTheDocument();
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

/**
 * The caveat that must not set the row height.
 *
 * It is the difference between "revoking works" and "revoking looks like it
 * worked", so it must stay reachable — but as a wall of text in a cell it made
 * one row 293px against an 89px neighbour (measured in the browser at a 786px
 * content box), and the rows then disagreed visually for a reason that was not
 * about importance.
 *
 * The split these tests pin: **the consequence is the summary and is on screen
 * with nothing clicked; the reason is behind the disclosure.** Collapsing the
 * headline itself would be the mistake `TryIt`'s doc records against drawers —
 * collapsing the paragraph under it is the point.
 */
describe("AccessSection — the row-scoped caveat", () => {
  function renderGrantedAndAlsoInherited(endpoint?: DataApiEndpointDto) {
    setup({
      accounts: [SA_GRANTED],
      grants: [makeGrant(SA_GRANTED.id, SA_GRANTED.name)],
      canInvoke: { [SA_GRANTED.id]: true },
    });
    return renderAccess(endpoint);
  }

  it("puts the consequence on screen unclicked and the reason behind the disclosure", async () => {
    renderGrantedAndAlsoInherited();
    const row = await screen.findByTestId(`access-row-${SA_GRANTED.id}`);

    // The load-bearing half, with nothing interacted with. This is the
    // assertion that would fail if the caveat were collapsed WHOLE — which is
    // the failure mode that matters, because an admin who never opens it would
    // click Revoke believing it removes the access.
    const summary = within(row).getByText("Revoking will not remove access");
    const details = summary.closest("details");
    expect(details).not.toBeNull();

    // jsdom does not hide a closed `<details>` — it has no layout to hide it
    // with, and Testing Library does not exclude its contents (§11.3). So
    // `toBeVisible` passes either way and says nothing; the `open` ATTRIBUTE is
    // the only observable, and it is what changes.
    expect(details).not.toHaveAttribute("open");

    // The reason is INSIDE the disclosure, not beside it — otherwise nothing
    // has been taken out of the row's height.
    const body = within(details!).getByText(/also reaches invoke through the workspace/i);
    expect(body.closest("summary")).toBeNull();
    expect(within(details!).getByRole("link", { name: /membership/i })).toBeInTheDocument();

    await userEvent.click(summary);
    expect(details).toHaveAttribute("open");
  });

  /**
   * §11.2, and the trap it was written for. The shell wraps every section in
   * `<fieldset disabled={trashed}>`, which disables every BUTTON under it — a
   * Radix `Collapsible` trigger included. Swapping `ui/collapsible` in here
   * would make the caveat unopenable on exactly the endpoint where an admin is
   * deciding whether to restore it, and every text assertion above would still
   * pass because jsdom does not hide a closed disclosure either way.
   *
   * A `<summary>` is not a form control. That is the whole reason it is one.
   */
  it("keeps the caveat openable on a trashed endpoint, where a Radix trigger could not be", async () => {
    // Trashing an endpoint disables it, so the probe cannot run and this row's
    // caveat is the "may not" one rather than the "will not" one. Same
    // `Caveat`, which is what this test is about — and it is the caveat that
    // matters most here, because the reader is deciding whether to restore.
    renderGrantedAndAlsoInherited(
      makeEndpoint({ state: "disabled", deleted_at: "2026-08-18T09:00:00Z" } as Partial<DataApiEndpointDto>),
    );
    const row = await screen.findByTestId(`access-row-${SA_GRANTED.id}`);
    const headline = within(row).getByText("Revoking may not remove access");
    const summary = headline.closest("summary");
    expect(summary).not.toBeNull();

    // Guard the premise: the fieldset really is disabled here, so this is a
    // test about surviving it rather than a test that happens to pass because
    // nothing was disabled in the first place.
    const fieldset = summary!.closest("fieldset");
    expect(fieldset).not.toBeNull();
    expect(fieldset).toBeDisabled();

    // …and the disclosure contains no form control that the fieldset could
    // reach. A `ui/collapsible` renders a `<button>` here.
    const details = summary!.closest("details")!;
    expect(within(details).queryByRole("button")).not.toBeInTheDocument();

    /**
     * `fireEvent`, not `userEvent`, and the difference is the finding.
     *
     * `userEvent.click` REFUSES this click — the identical gesture in the test
     * above works, and the only difference between the two renders is
     * `deleted_at`, so it is the disabled `fieldset` that stops it. That is
     * userEvent being stricter than the platform: `fieldset[disabled]` disables
     * *form-associated* descendants, and `<summary>` is not one, so every
     * browser opens this. `fireEvent` dispatches the event and jsdom's own
     * summary activation behaviour runs it — which is the evidence that the
     * element is genuinely activatable here rather than an assertion that it
     * ought to be.
     *
     * Recorded because the failure mode is a FALSE NEGATIVE: a later test of
     * any non-form affordance inside a trashed section, written with
     * `userEvent` out of habit, will report the product broken when it is not.
     */
    fireEvent.click(headline);
    expect(details).toHaveAttribute("open");
  });

  // A six-word transient status has no explanation to hold, so collapsing it
  // would cost a click to reveal nothing — §11.2's rule applied per caveat
  // rather than per section: collapse a thing because it is big.
  it("leaves the short transient states as plain text, not as an empty disclosure", async () => {
    setup({ accounts: [SA_NONE], grants: [] });
    renderAccess(makeEndpoint({ state: "draft", live_version_id: null }));

    const row = await screen.findByTestId(`access-row-${SA_NONE.id}`);
    const line = within(row).getByText(/Workspace-inherited invoke is unchecked/i);
    expect(line.closest("details")).toBeNull();
  });
});

/**
 * A trashed endpoint: what is dead, what is deliberately not, and the sentence
 * that tells the two apart.
 *
 * `grant_invoke` / `revoke_invoke` refusing a trashed endpoint is a recorded
 * backend decision, not a defect — they write no `state` and no
 * `live_version_id`, so there is no resurrection path. **The defect was that
 * the shell greyed both buttons silently and this section never explained it**,
 * and the `Try it` link landing beside them (deliberately still live) made a
 * greyed button next to a working one look like a bug rather than a rule.
 */
describe("AccessSection — a trashed endpoint", () => {
  const TRASHED = { state: "disabled", deleted_at: "2026-08-18T09:00:00Z" } as Partial<DataApiEndpointDto>;

  /**
   * Both states in ONE test, and each assertion scoped to its own render's
   * `container`.
   *
   * A trashed-only fixture cannot tell "explains the rule" from "always shows
   * that sentence", and cannot tell "disabled because trashed" from "disabled
   * unconditionally" — the same class of unfalsifiable fixture that let a
   * `remove(0)` pass in #529 and that this branch has now cut out three times.
   * The two renders differ ONLY in `deleted_at`.
   */
  it("says why grant and revoke are dead when trashed, and says nothing when it is not", async () => {
    setup({
      accounts: [SA_GRANTED, SA_NONE],
      grants: [makeGrant(SA_GRANTED.id, SA_GRANTED.name)],
      canInvoke: {},
    });

    const live = renderAccess(makeEndpoint());
    const liveScope = within(live.container);
    expect(await liveScope.findByRole("button", { name: /revoke/i })).toBeEnabled();
    expect(liveScope.getByRole("button", { name: /grant invoke/i })).toBeEnabled();
    expect(liveScope.queryByText(/restore it first/i)).not.toBeInTheDocument();

    const trashed = renderAccess(makeEndpoint(TRASHED));
    const trashedScope = within(trashed.container);
    expect(await trashedScope.findByRole("button", { name: /revoke/i })).toBeDisabled();
    expect(trashedScope.getByRole("button", { name: /grant invoke/i })).toBeDisabled();

    // Visible, not a `title`: the rule is only taught if it is on screen — the
    // same reason `Versions` puts "Restore it first" beside Publish rather than
    // in a tooltip.
    expect(trashedScope.getByText(/restore it first/i)).toBeInTheDocument();
    // …and the half the shell's banner gets wrong. Its "everything below is
    // read-only" is false of this section, so the section has to correct it.
    expect(trashedScope.getByText(/Try it still runs/i)).toBeInTheDocument();
    expect(trashedScope.getByText(/already recorded are untouched/i)).toBeInTheDocument();
  });

  /**
   * The two affordances that must NOT die with the buttons.
   *
   * `Try it` is a `<Link>` on purpose: a trial is the only way to answer "could
   * this account read the data" for an endpoint someone is deciding whether to
   * restore, and the shell's `fieldset` does not reach a link. The caveat is a
   * `<summary>` for the same class of reason (§11.2).
   *
   * Both are asserted the way the PLATFORM behaves, not the way the library
   * does: `userEvent.click` refuses to activate a `<summary>` under
   * `<fieldset disabled>` even though `fieldset[disabled]` disables only
   * form-associated descendants, so it is a false negative here.
   */
  it("keeps Try it and the caveat reachable on a trashed endpoint", async () => {
    setup({
      accounts: [SA_GRANTED],
      grants: [makeGrant(SA_GRANTED.id, SA_GRANTED.name)],
      canInvoke: {},
    });
    renderAccess(makeEndpoint(TRASHED));

    const row = await screen.findByTestId(`access-row-${SA_GRANTED.id}`);

    // Guard the premise: the button beside the link really is dead, so this is
    // a test about the link surviving rather than one where nothing was
    // disabled to begin with.
    expect(within(row).getByRole("button", { name: /revoke/i })).toBeDisabled();

    const tryIt = within(row).getByRole("link", { name: /try it/i });
    expect(tryIt).toHaveAttribute(
      "href",
      `/explore/eng/data-api/endpoints/ep-1/try?service_account=${SA_GRANTED.id}`,
    );
    // A `<Link>`, not a disabled control wearing a link's clothes.
    expect(tryIt.tagName).toBe("A");
    expect(tryIt).not.toHaveAttribute("aria-disabled");

    const details = within(row).getByText("Revoking may not remove access").closest("details")!;
    expect(details).not.toHaveAttribute("open");
    fireEvent.click(within(row).getByText("Revoking may not remove access"));
    expect(details).toHaveAttribute("open");
  });
});
