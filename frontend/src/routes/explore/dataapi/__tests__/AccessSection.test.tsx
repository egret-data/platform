import { describe, it, expect, vi, beforeEach } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Outlet, Route, Routes } from "react-router-dom";
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
import type { EndpointDetailContext } from "@/routes/explore/dataapi/EndpointDetail";
import AccessSection from "@/routes/explore/dataapi/detail/AccessSection";
import { expectSectionIntroCapsItsProse, expectSectionLeavesWidthToThePage } from "./sectionWidth";

function makeEndpoint(overrides: Partial<DataApiEndpointDto> = {}): DataApiEndpointDto {
  return {
    id: "ep-1",
    folder_id: null,
    name: "Daily report",
    description: "",
    path: "reports/daily",
    method: "GET",
    database_id: "db-1",
    statements: [{ name: "main", sql: "SELECT 1" }],
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

/**
 * The address form the BACKEND synthesises, not a short one that happens to fit.
 *
 * `crates/egret-dataapi/src/routes.rs` builds every service account's `email` as
 * `svc.<name-slug>@<workspace-slug>.service.egret.local` — a 25-character fixed
 * tail plus `svc.`, so the shortest address a workspace can have is already 29
 * characters before the account is named. The fixtures used to be
 * `${id}@svc.local`, which is 18 characters and fits `w-48` with room to spare:
 * a degenerate fixture that cannot tell a column wide enough to read an address
 * from one that is not (the trap `docs/en/ui-conventions.md` §8 names).
 *
 * Measured in real Chromium against the shipped `font-mono text-xs`, seven
 * seeded accounts: these lay out at 246–419px, against 176px of column content.
 */
function makeAccount(id: string, name: string): DataApiServiceAccountDto {
  return {
    id,
    name,
    description: "",
    email: `svc.${name}@demo.service.egret.local`,
    created_at: "2026-01-01T00:00:00Z",
  };
}

/** `inheritsInvoke` is the server's answer to "would this account keep invoke
 *  through the workspace if this grant were revoked?" (KI-180). */
function makeGrant(serviceAccountId: string, name: string, inheritsInvoke = false): DataApiEndpointGrantDto {
  return {
    id: `g-${serviceAccountId}`,
    service_account_id: serviceAccountId,
    service_account_name: name,
    created_at: "2026-02-01T00:00:00Z",
    inherits_invoke: inheritsInvoke,
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

/**
 * The section WITHOUT the shell around it.
 *
 * **Why this harness has to exist.** Every other test here mounts the real
 * `EndpointDetail`, which wraps the `Outlet` in `<fieldset disabled={trashed}>`
 * — and `toBeDisabled` walks up to a disabled fieldset, so under that harness a
 * button reads as disabled whether or not the section guards it itself.
 * Measured: removing `|| trashed` from one button, and then removing the
 * `trashed` prop from `AccessRow` entirely, left all 25 tests green. The
 * assertion could not tell the section's own check from the shell's floor,
 * which is the "both green, both worthless" shape this branch keeps running
 * into.
 *
 * With no fieldset in the tree there is nothing else that could be doing it.
 * Same stubbed-context shape `TryIt.test.tsx` uses for its own reasons.
 */
function ContextProvider({ ctx }: { ctx: Partial<EndpointDetailContext> }) {
  return <Outlet context={ctx} />;
}

function renderAccessWithoutTheShell(ctx: Partial<EndpointDetailContext>) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/access"]}>
        <Routes>
          <Route path="/access" element={<ContextProvider ctx={ctx} />}>
            <Route index element={<AccessSection />} />
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  // The shell reads the version history on EVERY tab now: the header names the
  // version a consumer is actually receiving, and says when the saved draft has
  // moved past it. Automocked, that hook hands back `undefined`, so this default
  // is what keeps the shell mountable at all here — tests that care about
  // versions override it afterwards.
  vi.mocked(api.useDataApiListEndpointVersions).mockReturnValue(
    { data: { status: 200, data: [] }, isLoading: false, isError: false, error: null } as unknown as
      ReturnType<typeof api.useDataApiListEndpointVersions>,
  );

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
      grants: [makeGrant(SA_GRANTED.id, SA_GRANTED.name, true)],
      canInvoke: { [SA_GRANTED.id]: true },
    });
    renderAccess();

    const row = await screen.findByTestId(`access-row-${SA_GRANTED.id}`);
    expect(within(row).getByText("Granted on this endpoint")).toBeInTheDocument();
    expect(within(row).getByText("Revoking will not remove access")).toBeInTheDocument();
    expect(within(row).getByText(/also reaches invoke through the workspace/i)).toBeInTheDocument();
    expect(within(row).getByRole("button", { name: /revoke/i })).toBeInTheDocument();
  });

  // KI-180. The per-endpoint probe says YES for every granted account — the
  // grant itself is what it sees — so reading the note off it put "Revoking
  // will not remove access" on accounts with no workspace role, whose next call
  // after a revoke is a 403. The note now follows the grant's own
  // `inherits_invoke`, and the probe saying yes must not bring it back.
  it("does not warn on a granted account that has no workspace role", async () => {
    setup({
      accounts: [SA_GRANTED],
      grants: [makeGrant(SA_GRANTED.id, SA_GRANTED.name, false)],
      canInvoke: { [SA_GRANTED.id]: true },
    });
    renderAccess();

    const row = await screen.findByTestId(`access-row-${SA_GRANTED.id}`);
    expect(within(row).getByText("Granted on this endpoint")).toBeInTheDocument();
    expect(within(row).queryByText(/remove access/i)).not.toBeInTheDocument();
    expect(within(row).queryByText(/also reaches invoke through the workspace/i)).not.toBeInTheDocument();
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
        { id: "g-ghost", service_account_id: "sa-ghost", service_account_name: null, created_at: "2026-02-01T00:00:00Z", inherits_invoke: false },
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

  // A granted row's note does not depend on the per-endpoint probe at all: the
  // grant list carries the answer, so a probe that failed — or an endpoint that
  // is not live, where it never runs — leaves the row saying exactly what the
  // server said.
  it("keeps a granted row's note when the per-endpoint probe failed", async () => {
    setup({ accounts: [SA_GRANTED], grants: [makeGrant(SA_GRANTED.id, SA_GRANTED.name, true)], openapiError: true });
    renderAccess();

    const row = await screen.findByTestId(`access-row-${SA_GRANTED.id}`);
    expect(within(row).getByText("Granted on this endpoint")).toBeInTheDocument();
    expect(within(row).getByText("Revoking will not remove access")).toBeInTheDocument();
    expect(within(row).queryByText(/could not check/i)).not.toBeInTheDocument();
  });

  it("says whether revoking removes access on an endpoint that is not live", async () => {
    setup({
      accounts: [SA_GRANTED, SA_GRANTED_2],
      grants: [makeGrant(SA_GRANTED.id, SA_GRANTED.name, true), makeGrant(SA_GRANTED_2.id, SA_GRANTED_2.name, false)],
    });
    renderAccess(makeEndpoint({ state: "draft", live_version_id: null }));

    const inherits = await screen.findByTestId(`access-row-${SA_GRANTED.id}`);
    const only = screen.getByTestId(`access-row-${SA_GRANTED_2.id}`);
    expect(within(inherits).getByText("Revoking will not remove access")).toBeInTheDocument();
    expect(within(only).queryByText(/remove access/i)).not.toBeInTheDocument();
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

    // Reachable, not merely present: the sentence now lives behind the Invoke
    // column's disclosure (Task 3), and jsdom does not hide a closed
    // `<details>` — so a `getByText` alone would pass on copy no reader can get
    // to. Open it the way a reader does, then assert.
    const summary = await screen.findByText("Invoke is not data access");
    const details = summary.closest("details")!;
    // Closed FIRST. Without this the test passes on `<details open>` — a
    // disclosure that collapses nothing — and "reachable" would be asserting
    // only that a click did not break anything.
    expect(details).not.toHaveAttribute("open");
    fireEvent.click(summary);
    expect(details).toHaveAttribute("open");
    expect(within(details).getByText(/egret cannot check/i)).toBeInTheDocument();
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

    // …and it is in the Invoke COLUMN HEADER, not inside any row — which is
    // what makes the count independent of how many accounts there are. It
    // moved into the header in Task 3 ("beside the column where the
    // distinction bites"), so `closest("table")` is no longer the discriminator
    // it was; the row is.
    const sentence = screen.getByText(/egret cannot check whether these accounts may read/i);
    expect(sentence.closest("[data-testid^=\"access-row-\"]")).toBeNull();
    expect(sentence.closest("th")).not.toBeNull();
    expect(sentence.closest("th")).toHaveTextContent("Invoke");
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

/**
 * Task 3. The page-level `SettingsNote` and the `Service accounts` paragraph
 * said overlapping things about invoke-right versus data-access — ~550
 * characters standing above a table whose first row is one line. They are now
 * ONE disclosure, headed by the distinction and attached to the Invoke column,
 * which is where that distinction bites.
 *
 * Nothing was cut. These tests are what says so: every sentence from both
 * blocks is asserted inside the disclosure, and the disclosure is asserted
 * openable — including on a trashed endpoint, which is the case §11.2 exists
 * for.
 */
describe("AccessSection — invoke is not data access, said once", () => {
  it("heads the distinction on the Invoke column and holds both blocks' sentences behind it", async () => {
    setup({ accounts: [SA_GRANTED], grants: [] });
    renderAccess();

    const summary = await screen.findByText("Invoke is not data access");
    // Beside the column, not above the table: the answers this qualifies are
    // the ones in that column.
    const header = summary.closest("th");
    expect(header).not.toBeNull();
    expect(within(header!).getByText("Invoke")).toBeInTheDocument();

    const details = summary.closest("details")!;
    expect(details).not.toHaveAttribute("open");

    // Every sentence from BOTH blocks, none of them in the summary.
    for (const re of [
      /Publishing an endpoint does not grant data access/i,
      /under whatever the engine enforces/i,
      /two separate permissions and only the first one is on this page/i,
      /egret cannot check whether these accounts may read the underlying tables/i,
      /which is why it is not a column/i,
      /to find out for real/i,
    ]) {
      const node = within(details).getByText(re);
      expect(node.closest("summary")).toBeNull();
    }

    fireEvent.click(summary);
    expect(details).toHaveAttribute("open");
  });

  it("says it once, not once above the table and again beside the column", async () => {
    setup({ accounts: [SA_GRANTED, SA_NONE], grants: [] });
    renderAccess();

    await screen.findByTestId(`access-row-${SA_GRANTED.id}`);
    // The merge is the point: two blocks became one, so each sentence appears
    // exactly once in the whole section — and it stays one no matter how many
    // accounts there are.
    expect(screen.getAllByText(/Publishing an endpoint does not grant data access/i)).toHaveLength(1);
    expect(screen.getAllByText("Invoke is not data access")).toHaveLength(1);
    // The standing `role="note"` box this replaced is gone; the sentences are
    // not.
    expect(screen.queryByRole("note")).not.toBeInTheDocument();
  });

  it("stays openable on a trashed endpoint, where a Radix trigger could not be", async () => {
    setup({ accounts: [SA_GRANTED], grants: [] });
    renderAccess(
      makeEndpoint({ state: "disabled", deleted_at: "2026-08-18T09:00:00Z" } as Partial<DataApiEndpointDto>),
    );

    const summary = await screen.findByText("Invoke is not data access");
    const fieldset = summary.closest("fieldset");
    expect(fieldset).not.toBeNull();
    expect(fieldset).toBeDisabled();

    const details = summary.closest("details")!;
    expect(within(details).queryByRole("button")).not.toBeInTheDocument();
    // `fireEvent`, not `userEvent` — see the caveat tests below for why the
    // difference is a false negative and not a product failure.
    fireEvent.click(summary);
    expect(details).toHaveAttribute("open");
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
    //
    // Their ACCESSIBLE NAMES, and computed by the library rather than
    // approximated here. A `<th>` names every cell in its column, so once the
    // Invoke header gained the invoke-vs-data caveat (Task 3) a `textContent`
    // assertion would have passed on a header that reads 550 characters at a
    // screen-reader user before each row's answer — the audio form of the very
    // defect that took the `Data` column out.
    //
    // The first version of this fix read `aria-label ?? textContent`, which is
    // a hand-rolled stand-in for the naming algorithm: it agrees with it today
    // and would disagree the moment the header used `aria-labelledby`, which is
    // exactly the sort of change this assertion exists to survive. `getByRole`
    // runs the real computation, so the identity checks below say "the header
    // in position N is the one the accessibility tree calls X" — order and name
    // in one, with nothing reimplemented.
    const headers = await screen.findAllByRole("columnheader");
    expect(headers).toHaveLength(3);
    expect(headers[0]).toBe(screen.getByRole("columnheader", { name: "Service account" }));
    expect(headers[1]).toBe(screen.getByRole("columnheader", { name: "Invoke" }));
    // Named for a screen reader and hidden on screen — a visible heading over
    // one small button is louder than what it labels.
    expect(headers[2]).toBe(screen.getByRole("columnheader", { name: "Actions" }));

    // …and the caveat is still IN that header, reachable by anyone reading the
    // header row itself. Naming the column is not hiding the sentence.
    expect(within(headers[1]).getByText("Invoke is not data access")).toBeInTheDocument();

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
   * The identity column is `w-48` inside a `table-fixed` table, so both of its
   * lines `truncate` at 192px at every viewport — measured live at seven
   * accounts, every email was cut. `title` is the cheap recovery: the full
   * value is still readable without rebalancing a column whose neighbour
   * genuinely carries a sentence per row.
   *
   * The assertion is that the tooltip equals the RENDERED text, not that a
   * tooltip merely exists. A `title` holding something else — the id, a
   * stale prop, an empty string — is a tooltip that lies, and `toHaveAttribute`
   * with no expected value would pass on all three. jsdom can say this: `title`
   * is an attribute, not a layout fact (§10), so the truncation itself stays a
   * browser question and only the recovery is pinned here.
   */
  it("makes the truncated account name and email recoverable on hover", async () => {
    setup({ accounts: [SA_GRANTED], grants: [] });
    renderAccess();

    const row = await screen.findByTestId(`access-row-${SA_GRANTED.id}`);
    expect(within(row).getByText(SA_GRANTED.name)).toHaveAttribute("title", SA_GRANTED.name);
    expect(within(row).getByText(SA_GRANTED.email!)).toHaveAttribute("title", SA_GRANTED.email);
  });

  /**
   * The second line is not always an address. An orphaned grant — a grant whose
   * service account this workspace no longer lists — renders an explanation
   * there instead, and the component hands the same binding to the text and to
   * the `title` so the two cannot drift. Pinned because the obvious shape
   * (`title={account.email}` beside a conditional child) is wrong in exactly
   * this row and right everywhere else, so nothing else in this file would
   * catch it.
   */
  it("points the tooltip at what an orphaned row actually says", async () => {
    setup({ accounts: [], grants: [makeGrant("sa-gone", "deleted-bot")] });
    renderAccess();

    const row = await screen.findByTestId("access-row-sa-gone");
    const line = within(row).getByText("no longer a service account in this workspace");
    expect(line).toHaveAttribute("title", "no longer a service account in this workspace");
  });

  /**
   * REVERSED, and the reversal is the point of the assertion rather than a
   * detail of it. This test used to say Access CAPS its own width, on
   * `docs/en/ui-conventions.md` §5.1's two-group rule: Config, Query and
   * Response used the page; Access, Try it and Versions capped themselves at
   * `max-w-5xl` because they hold prose and short lists that read worse
   * stretched across a wide monitor.
   *
   * That per-section argument lost to a whole-route one. Measured on the
   * deployed build at 1600×1000 with seeded data, the content box is 1296px on
   * every tab, but the widest block was 1296 on Query/Config/Response and 1024
   * on Access/Try it/Versions — so the content edge moved 272px on a tab
   * change, three times, with nothing on screen explaining it. §5.1 now says
   * one width for all six; the superseded argument is kept there under "The
   * part of this section that was reversed, and why".
   *
   * `expectSectionCapsItsOwnWidth` was deleted along with the rule — see
   * `sectionWidth.ts`.
   *
   * A class name, not a layout (§5.5): jsdom has no CSS, so this cannot see
   * whether six tabs agree on a width. `tests/e2e/data-api-detail-tab-widths.spec.ts`
   * measures that in a browser; this only stops a cap coming back silently.
   */

  /**
   * §5.1's intro rule, pinned where the class actually is.
   *
   * The class assertion is the RIGHT instrument here rather than a stand-in for
   * a measurement — see `expectSectionIntroCapsItsProse`. Before this test the
   * only guard on this cap was a Playwright spec that has never executed, and a
   * population count that `Response` and `Access` could satisfy without their
   * intro being capped at all.
   */
  it("caps its intro at a reading measure, not the section's (§5.1)", async () => {
    setup({ accounts: [SA_GRANTED], grants: [] });
    renderAccess();
    expectSectionIntroCapsItsProse(
      await screen.findByRole("heading", { name: "Access", level: 2 }),
    );
  });

  it("leaves width to the page, like all six tabs (§5.1, as reversed)", async () => {
    setup({ accounts: [SA_GRANTED], grants: [] });
    const { container } = renderAccess();
    const heading = await screen.findByRole("heading", { name: "Access", level: 2 });
    expectSectionLeavesWidthToThePage(container, heading);
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
      grants: [makeGrant(SA_GRANTED.id, SA_GRANTED.name, true)],
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
    // Trashing an endpoint disables it; the caveat still renders, because it
    // comes from the grant list rather than from the probe a disabled endpoint
    // cannot answer — and it matters most here, where the reader is deciding
    // whether to restore.
    renderGrantedAndAlsoInherited(
      makeEndpoint({ state: "disabled", deleted_at: "2026-08-18T09:00:00Z" } as Partial<DataApiEndpointDto>),
    );
    const row = await screen.findByTestId(`access-row-${SA_GRANTED.id}`);
    const headline = within(row).getByText("Revoking will not remove access");
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

  /**
   * §4.3, and TWO accounts because one cannot show the defect.
   *
   * Every row used to carry "Workspace-inherited invoke is unchecked while this
   * endpoint is not live." That renders on `reason === "not-live"`, which is
   * `!probeEnabled`, which is `!isLive` — the same expression that renders the
   * paragraph above the table, so the two can never disagree — and that
   * paragraph already says egret cannot check workspace-inherited invoke, plus
   * the draft-versus-disabled distinction the row cannot make. A strict subset,
   * once per row.
   *
   * Measured in a browser at seven service accounts: seven identical lines
   * under the sentence that had just said it. At the ONE account the audit ran
   * against it was one line and looked deliberate, which is why the fixture
   * here has two — a single-account fixture would pass over the restored bug.
   */
  it("says the not-live explanation once above the table, not once per row", async () => {
    setup({ accounts: [SA_NONE, SA_NONE_2], grants: [] });
    renderAccess(makeEndpoint({ state: "draft", live_version_id: null }));

    // The section-level sentence is there, exactly once.
    expect(
      await screen.findByText(/egret cannot check workspace-inherited invoke rights against it/i),
    ).toBeInTheDocument();
    expect(
      screen.getAllByText(/cannot check workspace-inherited invoke rights against it/i),
    ).toHaveLength(1);

    // And neither row repeats it.
    for (const account of [SA_NONE, SA_NONE_2]) {
      const row = screen.getByTestId(`access-row-${account.id}`);
      expect(
        within(row).queryByText(/workspace-inherited invoke is unchecked/i),
      ).not.toBeInTheDocument();
    }
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
      grants: [makeGrant(SA_GRANTED.id, SA_GRANTED.name, true)],
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

    const details = within(row).getByText("Revoking will not remove access").closest("details")!;
    expect(details).not.toHaveAttribute("open");
    fireEvent.click(within(row).getByText("Revoking will not remove access"));
    expect(details).toHaveAttribute("open");
  });

  /**
   * The section's OWN guard, which nothing above this can see.
   *
   * `Versions` records the argument for stating the rule in the section as well
   * as leaning on the shell — but under the real shell the two are
   * indistinguishable, and the mutation run proved it: dropping `|| trashed`
   * from a button, and dropping the `trashed` prop altogether, both left every
   * other test in this file green. Without the fieldset there is nothing else
   * that could disable these, so this is the one assertion that fails when the
   * section stops guarding itself.
   *
   * Both states, one test: `trashed: false` first, so "disabled because
   * trashed" is distinguishable from "disabled always".
   */
  it("guards its own mutations, with no fieldset in the tree to do it for it", async () => {
    setup({
      accounts: [SA_GRANTED, SA_NONE],
      grants: [makeGrant(SA_GRANTED.id, SA_GRANTED.name)],
      canInvoke: {},
    });

    const live = renderAccessWithoutTheShell({
      ws: "eng",
      id: "ep-1",
      endpoint: makeEndpoint(),
      trashed: false,
    });
    // The premise, asserted rather than assumed: this harness really has no
    // fieldset, so nothing here is inherited from the shell.
    expect(live.container.querySelector("fieldset")).toBeNull();
    expect(await within(live.container).findByRole("button", { name: /revoke/i })).toBeEnabled();
    expect(within(live.container).getByRole("button", { name: /grant invoke/i })).toBeEnabled();

    const trashed = renderAccessWithoutTheShell({
      ws: "eng",
      id: "ep-1",
      endpoint: makeEndpoint(TRASHED),
      trashed: true,
    });
    expect(trashed.container.querySelector("fieldset")).toBeNull();
    expect(await within(trashed.container).findByRole("button", { name: /revoke/i })).toBeDisabled();
    expect(within(trashed.container).getByRole("button", { name: /grant invoke/i })).toBeDisabled();
    // The link is not a form control and is not the section's to disable, so it
    // survives here too — this is where that is provable, since there is no
    // fieldset to have spared it by accident.
    expect(within(trashed.container).getAllByRole("link", { name: /try it/i })).toHaveLength(2);
  });
});

/**
 * Task 2 — the row that fits, and an identity you can read.
 *
 * # What was measured, and where
 *
 * Real Chromium, the shipped stylesheet, seven service accounts with the
 * addresses the backend synthesises, 1600×1000, on the tree this task started
 * from (`82ce049e`, i.e. with Task 1's cap already removed):
 *
 *   available 1296   columns 192 / 960 / 144   seven rows, 543px of table
 *   row heights 79, 79, 79, 79, 69, 79, 79
 *   the two row actions on TWO lines: a 32px 13px button, then a 16px 12px link
 *   every address cut: 246, 311, 419, 303, 289, 296, 311 against 176px of cell
 *
 * Three things follow, and only the third is one jsdom can hold at all:
 *
 * 1. The actions were STACKED, which is what made the row 79px, and the one the
 *    section's own prose points at ("Run **Try it** on a row to find out for
 *    real") was the smallest and faintest thing in it.
 * 2. The identity column was `w-48` — 176px of content — on the page whose
 *    subject *is* which identity may invoke. `title` recovers the value on
 *    hover; it is a fallback, not a fix.
 * 3. Removing the cap moved `Grant invoke` 272px further from the account it
 *    acts on. The row layout is what answers that, which is why 1 and 3 are one
 *    change.
 *
 * # Why almost none of that is asserted below
 *
 * jsdom implements no CSS (§10): every box is 0×0, so a row height, a column
 * width and a truncation all measure the same whether the fix is present or
 * not, and a test for them passes either way. The measurements live in
 * `tests/e2e/data-api-access-row.spec.ts`, with a negative control that puts
 * the old layout back and reproduces the numbers above.
 *
 * What is left here is the part that is real in a DOM: which elements are
 * siblings, in what order, and what each one IS. The two class assertions are
 * labelled as canaries where they appear — they stop a silent revert, they do
 * not make the claim.
 */
describe("AccessSection — a row that fits, and an identity you can read", () => {
  const TRASHED = { state: "disabled", deleted_at: "2026-08-18T09:00:00Z" } as Partial<DataApiEndpointDto>;

  /**
   * Both actions in one group, and `Try it` FIRST.
   *
   * Membership alone says nothing — they were siblings before this change too,
   * in a `flex-col`. The order is the falsifiable half, and it is §1.2's rule
   * (view controls first, primary action last) applied to a row: the mutation
   * is the primary action and sits at the outer right edge, where the same
   * control sits in the page header above it.
   *
   * Run over BOTH kinds of row. The mutation is a different button on a granted
   * row than on an ungranted one, and a fixture with one of them cannot say the
   * rule holds for the other.
   */
  it("puts the row's two actions in one group, with the mutation last", async () => {
    setup({
      accounts: [SA_NONE, SA_GRANTED],
      grants: [makeGrant(SA_GRANTED.id, SA_GRANTED.name)],
      canInvoke: {},
    });
    renderAccess();

    const cases = [
      { account: SA_NONE, mutation: /grant invoke/i },
      { account: SA_GRANTED, mutation: /revoke/i },
    ] as const;

    for (const { account, mutation } of cases) {
      const row = await screen.findByTestId(`access-row-${account.id}`);
      const tryIt = within(row).getByRole("link", { name: /try it/i });
      const button = within(row).getByRole("button", { name: mutation });

      const group = button.parentElement!;
      expect(tryIt.parentElement, `${account.name}: one group, not two blocks`).toBe(group);

      const order = [...group.children];
      expect(
        order.indexOf(tryIt),
        `${account.name}: Try it comes before the mutation (§1.2)`,
      ).toBeLessThan(order.indexOf(button));

      // Both mutations carry the SAME width floor, which is what keeps the link
      // to their left at one offset down the page: measured, `Revoke` is 78px
      // and `Grant invoke` 104px, so without it `Try it` moves 26px between
      // rows. It also stops the button resizing under the cursor when its own
      // label becomes "Revoking…". A class canary — the offsets themselves are
      // measured in `data-api-access-row.spec.ts`; what makes it worth pinning
      // here is that it has to be the same on BOTH arms, which is why this loop
      // runs over both.
      expect(button.className, `${account.name}: shares the mutation width floor`).toMatch(
        /\bmin-w-28\b/,
      );
    }

    // The canary for the geometry, and ONLY a canary: "beside each other" is a
    // browser question (§10), measured in `data-api-access-row.spec.ts`. What a
    // DOM can say is that the group is no longer declared as a column, which is
    // the declaration that stacked them.
    const group = within(await screen.findByTestId(`access-row-${SA_NONE.id}`))
      .getByRole("button", { name: /grant invoke/i })
      .parentElement!;
    expect(group.className).toMatch(/\bflex\b/);
    expect(group.className).not.toMatch(/flex-col/);
  });

  /**
   * `Try it` is a peer of the mutation, not the row's smallest text.
   *
   * Asserted through the component both controls come from rather than through
   * two class strings that agree today (§7.2): `data-slot="button"` is
   * `src/components/ui/button.tsx`'s own marker, so sharing it is sharing a
   * height, a hit area and a text size by construction.
   *
   * Everything that makes it QUIETER than the mutation — no border, no fill —
   * is paint, and paint is the browser spec's job. The one class named here is
   * the exact one the defect wore.
   */
  it("gives Try it the shared control treatment instead of the row's smallest text", async () => {
    setup({ accounts: [SA_NONE], grants: [], canInvoke: {} });
    renderAccess();

    const row = await screen.findByTestId(`access-row-${SA_NONE.id}`);
    const tryIt = within(row).getByRole("link", { name: /try it/i });
    const button = within(row).getByRole("button", { name: /grant invoke/i });

    // The premise: the mutation really does carry that marker, so "they agree"
    // is not two elements agreeing on nothing.
    expect(button).toHaveAttribute("data-slot", "button");
    expect(tryIt).toHaveAttribute("data-slot", "button");

    // …and it is still a LINK. It navigates, and it has to outlive the shell's
    // `<fieldset disabled={trashed}>`, which reaches form controls only — so
    // dressing it as a control must not turn it into one.
    expect(tryIt.tagName).toBe("A");
    expect(tryIt).toHaveAttribute("href");
    expect(tryIt).not.toHaveAttribute("aria-disabled");
    expect(tryIt).not.toHaveAttribute("disabled");

    // Canary, named after the defect: `text-xs` on a 12px link under a 13px
    // button is how it became the faintest thing in the row.
    expect(tryIt.className).not.toMatch(/\btext-xs\b/);
  });

  /**
   * Still true on a TRASHED endpoint, which is the case the treatment change
   * could plausibly break: `Button` renders `aria-disabled` + `tabIndex={-1}`
   * on the `asChild` path when it is given `disabled`, and inheriting that from
   * anywhere would silently kill the one control this section deliberately
   * keeps alive.
   */
  it("keeps Try it live and activatable when it wears the control treatment", async () => {
    setup({
      accounts: [SA_GRANTED],
      grants: [makeGrant(SA_GRANTED.id, SA_GRANTED.name)],
      canInvoke: {},
    });
    renderAccess(makeEndpoint(TRASHED));

    const row = await screen.findByTestId(`access-row-${SA_GRANTED.id}`);
    // The premise: the button beside it really is dead.
    expect(within(row).getByRole("button", { name: /revoke/i })).toBeDisabled();

    const tryIt = within(row).getByRole("link", { name: /try it/i });
    expect(tryIt).toHaveAttribute(
      "href",
      `/explore/eng/data-api/endpoints/ep-1/try?service_account=${SA_GRANTED.id}`,
    );
    expect(tryIt).not.toHaveAttribute("aria-disabled");
    // Not taken out of the tab order — the half `aria-disabled` alone would not
    // have shown.
    expect(tryIt).not.toHaveAttribute("tabindex");
  });

  /**
   * The identity column gets room, and the narrow regime does not.
   *
   * A CLASS assertion, and it is not the claim (§5.5): jsdom lays nothing out,
   * so this only stops the column narrowing again without anyone noticing. What
   * the width has to BE — a synthesised address fitting inside the column, in
   * the shipped mono face — is measured in `data-api-access-row.spec.ts`.
   *
   * The identity gate is `xl`, not `lg`, and that is measured too.
   * `table-fixed` does not shrink a sized column, it scrolls the table
   * (§5.3), so the widened column has to leave Invoke enough at the
   * breakpoint it turns on at. Measured available width: 1280px viewport →
   * 976px, so Invoke keeps 384px; 1024px viewport → 720px, which would leave
   * it 128px against content that lays out at 183px. `lg` would have traded a
   * truncated address for a wrapped badge.
   *
   * The actions column is UNCONDITIONAL, and the asymmetry is a measurement
   * rather than an oversight — asserting it is what stops somebody tidying it
   * into a matching `xl:` gate. Gated, the action group wraps back onto two
   * lines everywhere below `xl`, and two 32px controls stacked are TALLER than
   * the 32px-plus-16px stack this change removed: seven rows measured 651px
   * gated at a 1024px viewport, against 451px ungated and 543px before the
   * change.
   */
  it("widens the identity column above xl, and the actions column at every width", async () => {
    setup({ accounts: [SA_GRANTED], grants: [] });
    renderAccess();

    const identity = await screen.findByRole("columnheader", { name: "Service account" });
    expect(identity.className).toMatch(/\bw-48\b/);
    expect(identity.className).toMatch(/\bxl:w-96\b/);

    // Measured, the widest pair (`Try it` + `Grant invoke`, gap included) needs
    // 167px. `w-52` gives 192px of content; `w-36` gave 128, which is why the
    // two were stacked. The negative assertions are the ones with teeth: a
    // column that narrows again, or gains a breakpoint, wraps the group.
    const actions = screen.getByRole("columnheader", { name: "Actions" });
    expect(actions.className).toMatch(/\bw-52\b/);
    expect(actions.className).not.toMatch(/\bw-36\b/);
    expect(actions.className).not.toMatch(/:w-/);
  });

  /**
   * The other half of what Task 1's cap was buying, and the one that is prose
   * rather than layout.
   *
   * §5.2: cap the prose, not the column. With the section root uncapped these
   * three paragraphs run the full content box — measured at 1920 with the
   * sidebar collapsed they pass 1700px, roughly triple a comfortable measure.
   * `max-w-prose` is the spelling the sibling sections already use
   * (`ResponseSection.tsx`), and `tests/e2e/data-api-detail-tab-widths.spec.ts`
   * asserts capped prose stops SHORT of the section root, so this is the
   * deliberate exception to §5.1's one width rather than a second cap.
   *
   * All three in one render, and that is why the fixture is trashed AND not
   * live: the two conditional paragraphs exist only while their state holds, so
   * a fixture that renders one of them cannot say anything about the other.
   */
  it("caps every standing paragraph at a reading measure (§5.2)", async () => {
    setup({ accounts: [SA_NONE], grants: [] });
    renderAccess(makeEndpoint(TRASHED));

    const paragraphs = [
      await screen.findByText(/What each service account can actually do/i),
      screen.getByText(/This endpoint is disabled, so it is serving nothing/i),
      // NOT `/This endpoint is in the trash/` — the shell renders its own
      // trashed banner with that same opening, so the loose matcher finds two
      // elements and dies before asserting anything. This clause is the
      // section's own.
      screen.getByText(/so invoke cannot be granted or revoked here/i),
    ];

    for (const p of paragraphs) {
      // Found as a `<p>`, so a matcher that drifted onto a wrapping `div` would
      // fail loudly instead of asserting a class on the wrong element.
      expect(p.tagName, `"${p.textContent?.slice(0, 40)}…" is a paragraph`).toBe("P");
      expect(p.className, `"${p.textContent?.slice(0, 40)}…" is capped`).toMatch(
        /\bmax-w-prose\b/,
      );
    }
  });
});
