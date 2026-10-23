import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// There is no msw in this repo — tests mock the API module directly (see
// Endpoints.test.tsx / AuditLog.test.tsx).
vi.mock("@/api/generated/data-api/data-api");

import * as api from "@/api/generated/data-api/data-api";
import type { DataApiClientDto, DataApiServiceAccountDto } from "@/api/generated/model";
import Clients from "@/routes/explore/dataapi/Clients";
import { expectChipTone, expectNoChipTone } from "./expectChipTone";
import { ApiError } from "@/api/client";

// The one rule this task exists to hold: `DataApiClientDto` (the list shape)
// has no secret field — not a skipped one, an absent one. This string stands
// in for one; it is planted directly on the raw mock object (bypassing the
// DTO type via a cast) so a mutation that makes the row renderer reach past
// the declared fields and print it would be caught here, even though nothing
// in the real backend response could ever carry it.
const SECRET_FIXTURE = "sk_live_do_not_leak_9f3c2b";

// A real 36-character client id. The short form is what the ID column renders;
// the full value stays reachable through `title` and the copy button.
const LONG_CLIENT_ID = "7f3b2c1a-9d4e-4f8b-9a2c-1e5d6f7a8b9c";
const LONG_CLIENT_ID_SHORT = "7f3b2c1a…8b9c";

const SERVICE_ACCOUNT: DataApiServiceAccountDto = {
  id: "sa-1",
  name: "reporting-bot",
  description: "",
  email: "sa-1@service.egret.local",
  created_at: "2026-08-01T00:00:00Z",
};

function makeAccount(overrides: Partial<DataApiServiceAccountDto> = {}): DataApiServiceAccountDto {
  return { ...SERVICE_ACCOUNT, ...overrides };
}

function makeClient(overrides: Partial<DataApiClientDto> = {}): DataApiClientDto {
  return {
    id: "cl-1",
    name: "etl",
    client_id: "client_abc123",
    expires_at: null,
    last_used_at: null,
    revoked_at: null,
    created_at: "2026-08-10T00:00:00Z",
    ...overrides,
  };
}

function mockAccounts(
  over: Partial<{ data: unknown; isError: boolean; isLoading: boolean; error: unknown }> = {},
) {
  vi.mocked(api.useDataApiListServiceAccounts).mockReturnValue(
    { data: undefined, isLoading: false, isError: false, error: null, ...over } as unknown as
      ReturnType<typeof api.useDataApiListServiceAccounts>,
  );
}

function mockClients(over: Partial<{ data: unknown; isError: boolean; isLoading: boolean }> = {}) {
  vi.mocked(api.useDataApiListClients).mockReturnValue(
    { data: { status: 200, data: [] }, isLoading: false, isError: false, ...over } as unknown as
      ReturnType<typeof api.useDataApiListClients>,
  );
}

/** Per-account client lists, so a grouping test can give two accounts different clients. */
function mockClientsByAccount(byAccount: Record<string, DataApiClientDto[]>) {
  vi.mocked(api.useDataApiListClients).mockImplementation(
    ((_ws: string, accountId: string) => ({
      data: { status: 200, data: byAccount[accountId] ?? [] },
      isLoading: false,
      isError: false,
    })) as unknown as typeof api.useDataApiListClients,
  );
}

const deleteAccountMutateAsync = vi.fn();
const revokeMutateAsync = vi.fn();

function renderClients(accounts?: DataApiServiceAccountDto[]) {
  // Optional so every pre-existing call site keeps `beforeEach`'s single
  // account; passing a list is how the multi-row cases (search) set up.
  if (accounts) mockAccounts({ data: { status: 200, data: accounts } });
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const utils = render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/data-api/clients"]}>
        <Routes>
          <Route path="/explore/:workspace/data-api/clients" element={<Clients />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return { ...utils, qc };
}

beforeEach(() => {
  vi.clearAllMocks();

  mockAccounts({ data: { status: 200, data: [SERVICE_ACCOUNT] } });
  mockClients({
    data: {
      status: 200,
      data: [
        // Poisoned with a field `DataApiClientDto` does not declare — see the
        // header comment. A correct row renderer never touches it.
        { ...makeClient(), client_secret: SECRET_FIXTURE } as unknown as DataApiClientDto,
      ],
    },
  });

  vi.mocked(api.useDataApiDeleteServiceAccount).mockReturnValue(
    { mutateAsync: deleteAccountMutateAsync, isPending: false } as unknown as
      ReturnType<typeof api.useDataApiDeleteServiceAccount>,
  );
  vi.mocked(api.useDataApiRevokeClient).mockReturnValue(
    { mutateAsync: revokeMutateAsync, isPending: false } as unknown as
      ReturnType<typeof api.useDataApiRevokeClient>,
  );

  // Automocking turns the query-key builders into `vi.fn()`s returning
  // `undefined`; argument-sensitive fakes are what let a test tell "the
  // service-account list was invalidated" from "something was", and "THIS
  // account's client list" from "some client list".
  vi.mocked(api.getDataApiListServiceAccountsQueryKey).mockImplementation(
    (ws) => ["dataApiListServiceAccounts", ws] as unknown as
      ReturnType<typeof api.getDataApiListServiceAccountsQueryKey>,
  );
  vi.mocked(api.getDataApiListClientsQueryKey).mockImplementation(
    (ws, accountId) => ["dataApiListClients", ws, accountId] as unknown as
      ReturnType<typeof api.getDataApiListClientsQueryKey>,
  );

  deleteAccountMutateAsync.mockResolvedValue({ status: 200, data: { ok: true }, headers: new Headers() });
  revokeMutateAsync.mockResolvedValue({ status: 200, data: { ok: true }, headers: new Headers() });
});

describe("Clients", () => {
  it("shows an error state, not the first-run CTA, when the list fails", async () => {
    mockAccounts({ isError: true });
    renderClients();
    expect(await screen.findByText(/could not load/i)).toBeInTheDocument();
    expect(screen.queryByText(/no service accounts yet/i)).not.toBeInTheDocument();
  });

  // KI-231: the shared refusal, not a red box with "forbidden" under the
  // page's own "+ Service account".
  it("renders the shared refusal, without the page's actions, when the server refuses the list", async () => {
    mockAccounts({ isError: true, error: new ApiError(403, "forbidden") });
    renderClients();
    expect(
      await screen.findByRole("heading", { level: 1, name: "You don't have access to this page." }),
    ).toBeInTheDocument();
    expect(
      screen.getByText("Viewing clients needs the service_account:read permission in this workspace."),
    ).toBeInTheDocument();
    expect(screen.queryByText("forbidden")).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: /service account/i })).not.toBeInTheDocument();
  });

  it("shows the first-run CTA when there are genuinely no service accounts", async () => {
    mockAccounts({ data: { status: 200, data: [] } });
    renderClients();
    expect(await screen.findByText(/no service accounts yet/i)).toBeInTheDocument();
  });

  it("shows the loading skeleton while isLoading", () => {
    mockAccounts({ isLoading: true });
    renderClients();
    expect(screen.getByTestId("list-skeleton")).toBeInTheDocument();
  });

  it("shows which account a client belongs to without clicking", async () => {
    renderClients();
    expect(await screen.findByText("reporting-bot")).toBeInTheDocument();
    // "etl" (the client) renders under its owning account's group header, with
    // no expand/select step required to reveal it.
    expect(screen.getByText("etl")).toBeInTheDocument();
  });

  it("does not show the secret anywhere in the list", async () => {
    renderClients();
    await screen.findByText("etl");
    expect(document.body.textContent).not.toContain(SECRET_FIXTURE);
  });

  /**
   * Issuing a client used to be TWO dialogs this page owned — a form, then the
   * one-time secret, the second mounting as the first was unmounted while still
   * open. Both are the `ClientCreate` route now, so what is left for the list to
   * get right is that the group header's menu points at it, for the GROUP's own
   * account.
   *
   * The `href` is asserted rather than clicked: this harness mounts only the
   * `clients` route, so a click would land on nothing and "pass" for a link
   * pointing anywhere. And no form or secret may appear here — opening the menu
   * must navigate, not reveal a dialog.
   */
  it("the group menu's Issue client item links to that account's new-client page, not a dialog trigger", async () => {
    renderClients([makeAccount({ id: "sa-7", name: "nightly-exporter" })]);
    await screen.findByText("nightly-exporter");
    await userEvent.click(screen.getByRole("button", { name: "Actions for nightly-exporter" }));

    const item = await screen.findByRole("menuitem", { name: /issue client/i });
    // The account id comes from the ROW, not from a constant: `sa-7` is not the
    // id any other test in this file uses.
    expect(item).toHaveAttribute("href", "/explore/eng/data-api/clients/sa-7/new-client");
    expect(screen.queryByLabelText("Name")).not.toBeInTheDocument();
    expect(document.body.textContent).not.toContain(SECRET_FIXTURE);
  });

  it("shows the client-fetch error, not the empty state, when useDataApiListClients fails", async () => {
    mockClients({ isError: true });
    renderClients();
    await screen.findByText("reporting-bot");
    expect(await screen.findByText(/could not load clients/i)).toBeInTheDocument();
    // The bug this guards: folding `isError` into `clients.length === 0`
    // would print the empty-state copy for an account whose clients simply
    // failed to load — the same isError-as-empty defect this codebase fixed
    // once across four list pages.
    expect(screen.queryByText(/no clients yet/i)).not.toBeInTheDocument();
  });

  it("shows a loading state for the client list while useDataApiListClients is in flight", async () => {
    mockClients({ isLoading: true });
    renderClients();
    await screen.findByText("reporting-bot");
    expect(screen.getByText(/loading clients/i)).toBeInTheDocument();
    expect(screen.queryByText(/no clients yet/i)).not.toBeInTheDocument();
  });

  it("shows expiry and last-used for a client that carries them", async () => {
    mockClients({
      data: {
        status: 200,
        data: [makeClient({ expires_at: "2027-01-01T00:00:00Z", last_used_at: "2026-08-15T00:00:00Z" })],
      },
    });
    renderClients();
    const row = await screen.findByRole("row", { name: /etl/ });
    expect(within(row).getByText(/expires/i)).toBeInTheDocument();
    // Last used is a COLUMN now, so the cell carries the time and nothing else;
    // "never" is what an unused client's cell says.
    expect(within(row).queryByText(/never/i)).not.toBeInTheDocument();
  });

  it("says Never in the Last used column for a client that has never been used", async () => {
    renderClients();
    const row = await screen.findByRole("row", { name: /etl/ });
    expect(within(row).getByText("Never")).toBeInTheDocument();
  });

  it("revoking a client goes through ConfirmDeleteDialog (type-to-confirm) and calls useDataApiRevokeClient", async () => {
    const user = userEvent.setup();
    renderClients();
    await screen.findByText("etl");

    await user.click(screen.getByRole("button", { name: "Actions for etl" }));
    await user.click(await screen.findByRole("menuitem", { name: /revoke/i }));
    const input = await screen.findByPlaceholderText("revoke");
    await user.type(input, "revoke");
    await user.click(screen.getByRole("button", { name: /^revoke$/i }));

    await waitFor(() => {
      expect(revokeMutateAsync).toHaveBeenCalledWith({ workspace: "eng", id: "cl-1" });
    });
  });

  it("deleting a service account goes through ConfirmDeleteDialog and names what goes with it", async () => {
    const user = userEvent.setup();
    renderClients();
    await screen.findByText("reporting-bot");

    await user.click(screen.getByRole("button", { name: "Actions for reporting-bot" }));
    await user.click(await screen.findByRole("menuitem", { name: /delete/i }));
    expect(await screen.findByText(/grants and every client/i)).toBeInTheDocument();

    const input = screen.getByPlaceholderText("delete");
    await user.type(input, "delete");
    await user.click(screen.getByRole("button", { name: /^delete$/i }));

    await waitFor(() => {
      expect(deleteAccountMutateAsync).toHaveBeenCalledWith({ workspace: "eng", id: "sa-1" });
    });
  });

  it("shows the error, not the empty state, when the list fails to load", async () => {
    vi.mocked(api.useDataApiListServiceAccounts).mockReturnValue({
      data: undefined, isLoading: false, isError: true, error: new Error("nope"),
    } as never);
    renderClients();
    expect(await screen.findByText("nope")).toBeInTheDocument();
    expect(screen.queryByText(/no service accounts yet/i)).not.toBeInTheDocument();
  });

  it("puts the account's actions in a menu, not loose in the row", async () => {
    renderClients([makeAccount({ id: "a1", name: "bot" })]);
    await screen.findByText("bot");
    await userEvent.click(screen.getByRole("button", { name: "Actions for bot" }));
    expect(await screen.findByRole("menuitem", { name: /issue client/i })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /delete/i })).toBeInTheDocument();
  });

  /**
   * The other end of a reconciliation whose first end is
   * `ServiceAccountCreate.test.tsx`. That page's subtitle used to tell a
   * reader to "grant it endpoints from the Clients list", and this is the
   * Clients list: there is no grant here, on the account or on any of its
   * clients. Invoke is granted per ENDPOINT, on that endpoint's Access tab —
   * `detail/AccessSection.tsx` is the only caller of
   * `useDataApiGrantEndpointInvoke` in the tree.
   *
   * By IDENTITY, not by count and not by absence. `toHaveLength(2)` passes on
   * any two items, and `queryByRole(name: /grant/i)` passes on a control
   * labelled anything else; the exact set is the only form that both forbids a
   * grant appearing here silently and forbids the two real items quietly
   * changing. If a grant path is ever genuinely added to this surface, this
   * test fails — and the sentence one screen upstream has to be revisited with
   * it, which is the entire point of pinning them together.
   */
  it("offers no grant on the Clients surface — both menus are exactly what they are", async () => {
    const user = userEvent.setup();
    renderClients([makeAccount({ id: "a1", name: "bot" })]);
    await screen.findByText("bot");

    // The ACCOUNT's menu, in the group header row.
    await user.click(screen.getByRole("button", { name: "Actions for bot" }));
    await screen.findByRole("menuitem", { name: /issue client/i });
    expect(screen.getAllByRole("menuitem").map((i) => i.textContent)).toEqual([
      "Issue client\u2026",
      "Delete",
    ]);

    // WAIT for the first menu to leave the DOM, do not merely ask for it to
    // close. The reads below are document-wide (that is the point — a grant
    // item added anywhere is caught), so an un-flushed Radix exit animation
    // leaves the account's two items standing and the next assertion sees
    // three. That fails loudly rather than passing falsely, but a guard that
    // is flaky gets deleted, and a deleted guard pins nothing.
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryAllByRole("menuitem")).toHaveLength(0));

    // And the CLIENT's own row menu, because the subtitle's claim was about
    // this whole surface and not only about the account header. Radix mounts
    // only the open menu, so each set is read on its own.
    await user.click(screen.getByRole("button", { name: "Actions for etl" }));
    await screen.findByRole("menuitem", { name: /revoke/i });
    expect(screen.getAllByRole("menuitem").map((i) => i.textContent)).toEqual(["Revoke\u2026"]);
  });

  it("searches from the page toolbar, where every other Explore page has it", async () => {
    renderClients([makeAccount({ id: "a1", name: "alpha" }), makeAccount({ id: "a2", name: "beta" })]);
    await screen.findByText("alpha");
    // WHERE it is, not just that it filters: the box is in the page toolbar,
    // and there is exactly one of it (a second, uncontrolled box would filter a
    // set this one had already filtered).
    const box = screen.getByPlaceholderText(/search service accounts/i);
    expect(screen.getByTestId("page-toolbar")).toContainElement(box);
    expect(screen.getAllByPlaceholderText(/^search/i)).toHaveLength(1);

    await userEvent.type(box, "bet");
    expect(screen.queryByText("alpha")).not.toBeInTheDocument();
    expect(screen.getByText("beta")).toBeInTheDocument();
  });

  /**
   * Creating a service account used to be a dialog this page owned; it is now
   * the `clients/new` PAGE (`ServiceAccountCreate`), which owns the fields and
   * the mutation. What is left for the list to get right is that both of its
   * entry points are real LINKS to that URL — a `<button>` that pushed the
   * route imperatively would look identical on screen but could not be
   * middle-clicked, opened in a new tab, or copied.
   */
  it("the + Service account action is a link to the create page, not a dialog trigger", async () => {
    renderClients();
    await screen.findByText("reporting-bot");
    expect(screen.getByRole("link", { name: /\+ service account/i })).toHaveAttribute(
      "href",
      "/explore/eng/data-api/clients/new",
    );
    expect(screen.queryByRole("button", { name: /\+ service account/i })).not.toBeInTheDocument();
  });

  it("the empty-state CTA links to the create page too", async () => {
    mockAccounts({ data: { status: 200, data: [] } });
    renderClients();
    expect(
      await screen.findByRole("link", { name: /create your first service account/i }),
    ).toHaveAttribute("href", "/explore/eng/data-api/clients/new");
  });
});

/**
 * The shape this task exists to fix.
 *
 * Before: ONE table cell nested a variable-length list of clients, each entry a
 * line of running text — name, a 36-character UUID, a badge, a last-used phrase
 * and an action — so nothing aligned down the page. A client is now a ROW with
 * real columns, and the service account is the group header above it.
 */
describe("Clients — a client is a row, an account is a group header", () => {
  const ACCOUNTS = [
    makeAccount({ id: "sa-1", name: "reporting-bot", email: "sa-1@service.egret.local" }),
    makeAccount({ id: "sa-2", name: "warehouse-bot", email: "sa-2@service.egret.local" }),
  ];

  function renderTwoAccounts() {
    mockClientsByAccount({
      "sa-1": [
        makeClient({ id: "cl-a", name: "alpha-loader", client_id: LONG_CLIENT_ID }),
        makeClient({ id: "cl-b", name: "beta-loader", client_id: "client_bbb222" }),
      ],
      "sa-2": [makeClient({ id: "cl-c", name: "gamma-loader", client_id: "client_ccc333" })],
    });
    return renderClients(ACCOUNTS);
  }

  it("names the columns a client is read across", async () => {
    renderTwoAccounts();
    await screen.findByText("alpha-loader");
    for (const label of ["Client", "ID", "Status", "Last used"]) {
      expect(screen.getByRole("columnheader", { name: label })).toBeInTheDocument();
    }
  });

  it("gives each client its own row, with one cell per column", async () => {
    renderTwoAccounts();
    const row = await screen.findByRole("row", { name: /alpha-loader/ });
    // Five cells: Client · ID · Status · Last used · actions. A row with one
    // cell is the shape this task replaced.
    expect(within(row).getAllByRole("cell")).toHaveLength(5);
    expect(within(row).getByText("Active")).toBeInTheDocument();
    expect(within(row).getByText("Never")).toBeInTheDocument();
    // The account is the header ABOVE the row, never inside it.
    expect(within(row).queryByText("reporting-bot")).not.toBeInTheDocument();
  });

  it("makes the service account a group header row, not a cell in a client's row", async () => {
    renderTwoAccounts();
    await screen.findByText("alpha-loader");
    const header = screen.getByRole("rowheader", { name: /reporting-bot/ });
    expect(header).toBeInTheDocument();
    // The header names how many clients hang off it, so a reader can tell an
    // account with none from one whose list has not arrived.
    expect(header.textContent).toContain("2 clients");
    expect(screen.getByRole("rowheader", { name: /warehouse-bot/ }).textContent).toContain("1 client");
  });

  it("keeps each client under its OWN account, not merged into one list", async () => {
    renderTwoAccounts();
    await screen.findByText("alpha-loader");
    // `<tbody>` per group: the group header and its clients share a parent, and
    // the other account's client does not.
    const group = screen.getByRole("rowheader", { name: /reporting-bot/ }).closest("tbody")!;
    expect(within(group).getByText("alpha-loader")).toBeInTheDocument();
    expect(within(group).getByText("beta-loader")).toBeInTheDocument();
    expect(within(group).queryByText("gamma-loader")).not.toBeInTheDocument();
  });

  it("keeps the column headers on screen when an account has no clients", async () => {
    mockClients({ data: { status: 200, data: [] } });
    renderClients([makeAccount()]);
    expect(await screen.findByText(/no clients yet/i)).toBeInTheDocument();
    // §4.3: a list's states are rows INSIDE the table, so the columns are named
    // in every state rather than disappearing with the branch that replaced them.
    expect(screen.getByRole("columnheader", { name: "Client" })).toBeInTheDocument();
    expect(screen.getByText(/no clients yet/i).closest("tr")).toBeInTheDocument();
  });

  it("keeps the column headers on screen while an account's clients load, and when they fail", async () => {
    mockClients({ isLoading: true });
    const { unmount } = renderClients([makeAccount()]);
    expect(await screen.findByText(/loading clients/i)).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: "Status" })).toBeInTheDocument();
    unmount();

    mockClients({ isError: true });
    renderClients([makeAccount()]);
    expect(await screen.findByText(/could not load clients/i)).toBeInTheDocument();
    expect(screen.getByRole("columnheader", { name: "Status" })).toBeInTheDocument();
  });
});

/**
 * The ID column. A 36-character UUID dominated the old running-text line and
 * nobody reads one; what a reader actually does with it is paste it into a
 * client config.
 */
describe("Clients — the client id is truncated and copyable", () => {
  beforeEach(() => {
    mockClients({ data: { status: 200, data: [makeClient({ client_id: LONG_CLIENT_ID })] } });
  });

  it("renders a short form, not the whole 36 characters", async () => {
    renderClients();
    await screen.findByText("etl");
    expect(screen.getByText(LONG_CLIENT_ID_SHORT)).toBeInTheDocument();
    // Asserted alongside the short form on purpose: "the long id is absent"
    // passes over a page that rendered no id at all.
    expect(screen.queryByText(LONG_CLIENT_ID)).not.toBeInTheDocument();
  });

  it("keeps the full id reachable without a copy, through the cell's title", async () => {
    renderClients();
    await screen.findByText("etl");
    expect(screen.getByTitle(LONG_CLIENT_ID)).toHaveTextContent(LONG_CLIENT_ID_SHORT);
  });

  it("copies the FULL id, not the short form it shows", async () => {
    // Read back through `userEvent`'s own clipboard stub rather than spying on
    // `writeText`: `setup()` replaces `navigator.clipboard` with that stub, so
    // a spy installed beforehand is the object the component never touched —
    // which is exactly how this assertion failed first time round. Reading the
    // clipboard also asserts the VALUE that landed, not the call that was made.
    const user = userEvent.setup();
    renderClients();
    await screen.findByText("etl");
    await user.click(screen.getByRole("button", { name: "Copy client id for etl" }));
    expect(await navigator.clipboard.readText()).toBe(LONG_CLIENT_ID);
  });

  it("leaves a short id alone rather than truncating it to something longer", async () => {
    mockClients({ data: { status: 200, data: [makeClient({ client_id: "client_abc123" })] } });
    renderClients();
    await screen.findByText("etl");
    expect(screen.getByText("client_abc123")).toBeInTheDocument();
    expect(screen.queryByText(/…/)).not.toBeInTheDocument();
  });
});

/**
 * Revoked clients.
 *
 * The old row struck through the client's NAME and left the UUID beside it
 * unstruck, so the line read half-cancelled. Status is a column now: the badge
 * carries the word, and the row's muted form carries the rest. Nothing is
 * encoded by strikethrough, and nothing is encoded by colour alone.
 */
describe("Clients — a revoked client is not encoded by strikethrough", () => {
  beforeEach(() => {
    mockClients({
      data: {
        status: 200,
        data: [makeClient({ name: "old-etl", client_id: LONG_CLIENT_ID, revoked_at: "2026-08-01T00:00:00Z" })],
      },
    });
  });

  it("says Revoked in words, in the Status column", async () => {
    renderClients();
    const row = await screen.findByRole("row", { name: /old-etl/ });
    expect(within(row).getByText("Revoked")).toBeInTheDocument();
  });

  it("strikes through nothing at all — not the name, not the id", async () => {
    renderClients();
    const row = await screen.findByRole("row", { name: /old-etl/ });
    expect(row.querySelectorAll(".line-through")).toHaveLength(0);
  });

  it("carries a form difference beyond the badge's colour: the whole row is muted", async () => {
    renderClients();
    const row = await screen.findByRole("row", { name: /old-etl/ });
    expect(row.className).toContain("text-muted-foreground");
  });

  it("offers no revoke action for a client that is already revoked", async () => {
    renderClients();
    await screen.findByText("old-etl");
    expect(screen.queryByRole("button", { name: "Actions for old-etl" })).not.toBeInTheDocument();
  });

  it("still offers revoke for an expired-but-not-revoked client", async () => {
    mockClients({
      data: {
        status: 200,
        data: [makeClient({ name: "stale-etl", expires_at: "2020-01-01T00:00:00Z" })],
      },
    });
    renderClients();
    const row = await screen.findByRole("row", { name: /stale-etl/ });
    // Expiry and revocation are different lifecycle events on the backend
    // (`expires_at` vs `revoked_at`, independently settable).
    expect(within(row).getByText("Expired")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Actions for stale-etl" })).toBeInTheDocument();
  });
});

/**
 * Revoke is irreversible, and it used to be body text sitting between two
 * timestamps.
 */
describe("Clients — Revoke is a destructive control behind a confirmation", () => {
  it("does not revoke on the menu click alone", async () => {
    const user = userEvent.setup();
    renderClients();
    await screen.findByText("etl");
    await user.click(screen.getByRole("button", { name: "Actions for etl" }));
    await user.click(await screen.findByRole("menuitem", { name: /revoke/i }));
    expect(revokeMutateAsync).not.toHaveBeenCalled();
  });

  it("does not revoke until the confirmation word is typed", async () => {
    const user = userEvent.setup();
    renderClients();
    await screen.findByText("etl");
    await user.click(screen.getByRole("button", { name: "Actions for etl" }));
    await user.click(await screen.findByRole("menuitem", { name: /revoke/i }));

    const confirm = screen.getByRole("button", { name: /^revoke$/i });
    // Rendered outside any `fieldset`, so `toBeDisabled` cannot be satisfied by
    // an ancestor: this is the button's own state.
    expect(confirm.closest("fieldset")).toBeNull();
    expect(confirm).toBeDisabled();

    await user.type(screen.getByPlaceholderText("revoke"), "revoke");
    expect(confirm).toBeEnabled();
  });

  it("marks the menu item destructive and names the client it acts on", async () => {
    const user = userEvent.setup();
    renderClients();
    await screen.findByText("etl");
    await user.click(screen.getByRole("button", { name: "Actions for etl" }));
    const item = await screen.findByRole("menuitem", { name: /revoke/i });
    expect(item.className).toContain("text-destructive");
    await user.click(item);
    expect(await screen.findByText(/"etl"/)).toBeInTheDocument();
  });
});

/**
 * Cache invalidation (final-review finding, Important #1).
 *
 * The service-account list key is shared by FIVE surfaces — this table,
 * Try-it's "Run as" picker, the Access checklist, the OpenAPI download dialog
 * and the Response preview. With `staleTime: 30000` and no `onSuccess` on the
 * generated hooks, a newly created account was missing from all five for
 * thirty seconds, and a deleted one stayed in all five.
 */
describe("Clients — cache invalidation", () => {
  const RESULT = { status: 200, data: {}, headers: new Headers() } as never;

  async function fireSuccess(
    hook: { mock: { calls: unknown[][] } },
    variables: Record<string, unknown>,
  ) {
    const opts = hook.mock.calls.at(-1)?.[0] as
      | { mutation?: { onSuccess?: (...a: never[]) => unknown } }
      | undefined;
    await opts?.mutation?.onSuccess?.(
      RESULT,
      variables as never,
      undefined as never,
      undefined as never,
    );
  }

  it("deleting a service account invalidates that same list, so the row leaves the table", async () => {
    const { qc } = renderClients();
    const spy = vi.spyOn(qc, "invalidateQueries");
    await fireSuccess(vi.mocked(api.useDataApiDeleteServiceAccount), { workspace: "eng", id: "sa-1" });
    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ queryKey: ["dataApiListServiceAccounts", "eng"] }),
    );
  });

  it("invalidates the list of the workspace the write actually went to", async () => {
    const { qc } = renderClients();
    const spy = vi.spyOn(qc, "invalidateQueries");
    await fireSuccess(vi.mocked(api.useDataApiDeleteServiceAccount), { workspace: "other-ws", id: "sa-1" });
    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ queryKey: ["dataApiListServiceAccounts", "other-ws"] }),
    );
  });

  /**
   * Revoke's key is per SERVICE ACCOUNT, and the mutation's variables carry the
   * CLIENT's id — so the owning account's id has to be carried through the
   * pending-revoke state. Keying off the client id instead leaves the revoked
   * row on screen with an Active badge.
   */
  it("revoking invalidates the owning ACCOUNT's client list, not a key built from the client id", async () => {
    const user = userEvent.setup();
    // TWO accounts, and the revoke happens in the SECOND. With one account the
    // fixture is degenerate: the handler's `a` and the first rendered account
    // are the same object, so substituting one for the other changes nothing
    // and the test passes over a page that always invalidates group zero.
    // Sorted by name, so `alpha-bot` renders first and `billing-bot` second.
    mockClientsByAccount({
      "sa-8": [makeClient({ id: "cl-alpha", name: "alpha-client" })],
      "sa-9": [makeClient({ id: "cl-billing", name: "billing-client" })],
    });
    const { qc } = renderClients([
      makeAccount({ id: "sa-8", name: "alpha-bot", email: "sa-8@service.egret.local" }),
      makeAccount({ id: "sa-9", name: "billing-bot", email: "sa-9@service.egret.local" }),
    ]);
    const spy = vi.spyOn(qc, "invalidateQueries");
    await screen.findByText("billing-client");

    await user.click(screen.getByRole("button", { name: "Actions for billing-client" }));
    await user.click(await screen.findByRole("menuitem", { name: /revoke/i }));
    await user.type(screen.getByPlaceholderText("revoke"), "revoke");
    await user.click(screen.getByRole("button", { name: /^revoke$/i }));

    await waitFor(() => {
      expect(revokeMutateAsync).toHaveBeenCalledWith({ workspace: "eng", id: "cl-billing" });
    });
    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ queryKey: ["dataApiListClients", "eng", "sa-9"] }),
    );
    // The other group's list is left alone. Without this, a handler hoisted out
    // of the `.map()` — invalidating whichever account happens to be first —
    // still satisfies the assertion above only by accident of ordering; here it
    // invalidates `sa-8` and dies.
    expect(spy).not.toHaveBeenCalledWith(
      expect.objectContaining({ queryKey: ["dataApiListClients", "eng", "sa-8"] }),
    );
  });
});

/**
 * Clients is a top-level sidebar item now, not a section nested under a
 * `DataApiLayout` shell — that shell was removed once Data API got its own
 * sidebar group, so this page owns its own `h1` and `p-4`.
 */
describe("Clients — page scaffold", () => {
  it("renders its title as the page's own h1", async () => {
    renderClients();
    const h1 = await screen.findByRole("heading", { level: 1, name: "Clients" });
    expect(h1.className).toContain("text-xl"); // PageTitle, FE-PAGE-7
    expect(screen.queryByRole("heading", { level: 2, name: "Clients" })).not.toBeInTheDocument();
  });

  it("carries its own p-4 padding, like every other top-level Explore list page", async () => {
    const { container } = renderClients();
    await screen.findByRole("heading", { level: 1, name: "Clients" });
    expect(container.firstElementChild?.className ?? "").toMatch(/\bp-4\b/);
  });

  /**
   * One noun, one explanation. The sidebar and the title say *Clients*; the
   * search box and the summary say *service accounts*. They are different
   * things — an account HOLDS clients — and the page says so once, in its
   * description, instead of letting the two labels read as synonyms that
   * disagree.
   */
  it("explains, once, that a service account holds clients", async () => {
    renderClients();
    await screen.findByRole("heading", { level: 1, name: "Clients" });
    const description = screen.getByText(/service account holds/i);
    expect(description).toBeInTheDocument();
    expect(description.textContent).toMatch(/client/i);
    // Once: the explanation is the page description, directly under the h1 —
    // not repeated as a caption over the table.
    expect(screen.getAllByText(/service account holds/i)).toHaveLength(1);
  });
});

/**
 * One service account is the ordinary state of this page — most workspaces run
 * a single bot — and it rendered "1 service accounts".
 */
describe("Clients — the summary strip agrees with itself about number", () => {
  it("counts one service account without an s", async () => {
    renderClients([makeAccount()]);
    await screen.findByText("reporting-bot");

    expect(screen.getByText("service account").textContent).toBe("1service account");
    expect(screen.queryByText("service accounts")).not.toBeInTheDocument();
  });

  it("keeps the s for two", async () => {
    renderClients([makeAccount(), makeAccount({ id: "sa-2", name: "etl-bot", email: "sa-2@service.egret.local" })]);
    await screen.findByText("etl-bot");

    expect(screen.getByText("service accounts").textContent).toBe("2service accounts");
  });

  it("never renders a zero count — the empty workspace gets the first-run CTA instead", async () => {
    renderClients([]);

    expect(await screen.findByText(/no service accounts yet/i)).toBeInTheDocument();
    expect(screen.queryByText("service accounts")).not.toBeInTheDocument();
    expect(screen.queryByText("service account")).not.toBeInTheDocument();
  });
});

/**
 * Each group fires its own `useDataApiListClients` on mount — one request per
 * rendered service account. `FilterableTable`'s `pageSize={25}` used to cap
 * that by only mounting one page of rows; grouping replaced that table, so the
 * cap has to be kept explicitly or a workspace with 200 accounts fires 200
 * requests on first paint.
 */
describe("Clients — the per-account request fan-out stays capped", () => {
  const MANY = Array.from({ length: 30 }, (_, i) =>
    makeAccount({ id: `sa-${i}`, name: `bot-${String(i).padStart(2, "0")}`, email: `sa-${i}@service.egret.local` }),
  );

  function queriedAccountIds() {
    return new Set(vi.mocked(api.useDataApiListClients).mock.calls.map((c) => c[1]));
  }

  it("mounts at most 25 groups, so at most 25 client queries, on first paint", async () => {
    renderClients(MANY);
    await screen.findByText("bot-00");
    expect(queriedAccountIds().size).toBe(25);
    expect(screen.queryByText("bot-29")).not.toBeInTheDocument();
  });

  it("names how many are left, and shows them on request", async () => {
    const user = userEvent.setup();
    renderClients(MANY);
    await screen.findByText("bot-00");
    await user.click(screen.getByRole("button", { name: /show 5 more service accounts/i }));
    expect(await screen.findByText("bot-29")).toBeInTheDocument();
    expect(queriedAccountIds().size).toBe(30);
    expect(screen.queryByRole("button", { name: /show .* more/i })).not.toBeInTheDocument();
  });

  it("offers no Show-more button when everything already fits", async () => {
    renderClients([makeAccount()]);
    await screen.findByText("reporting-bot");
    expect(screen.queryByRole("button", { name: /show .* more/i })).not.toBeInTheDocument();
  });

  it("puts the cap back when the search changes, in both directions", async () => {
    const user = userEvent.setup();
    renderClients(MANY);
    await screen.findByText("bot-00");
    await user.click(screen.getByRole("button", { name: /show 5 more service accounts/i }));
    await screen.findByText("bot-29");

    // Searching after a "Show more" must not carry the raised limit into a
    // result set it was never about…
    const box = screen.getByPlaceholderText(/search service accounts/i);
    await user.type(box, "bot-2");
    expect(screen.queryByRole("button", { name: /show .* more/i })).not.toBeInTheDocument();

    // …and clearing it must not leave the cap silently off, which is the
    // direction that matters: it is the per-account request fan-out.
    await user.clear(box);
    await screen.findByText("bot-00");
    expect(screen.queryByText("bot-29")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /show 5 more service accounts/i })).toBeInTheDocument();
  });
});

/**
 * THE STATUS BADGE IS BOUND TO `CHIP_TONE`, PER ARM.
 *
 * The accessibility pass moved `Active` and `Expired` off `bg-<token>/15
 * text-<token>` — 3.72:1 and 3.68:1 in light mode, under WCAG 2.2 1.4.3's
 * 4.5:1 for this 12px text — and onto the shared map in `chipTone.ts`. Nothing
 * then held them there: the browser spec measures `CHIP_TONE`'s VALUES, which
 * a call site can stop using without moving a single number, and the arm
 * assertions above ask only for the WORD.
 *
 * Read from `CHIP_TONE`, never typed out, so a future contrast change moves
 * the map and these tests together. jsdom resolves no `oklch()` and composites
 * nothing, so what is pinned here is the WIRING; the ratios are measured in
 * `tests/e2e/dataapi-badge-ink.spec.ts`.
 */
describe("Clients — every Status arm is wired to the shared chip tone", () => {
  it("paints Active with CHIP_TONE.success", async () => {
    mockClients({ data: { status: 200, data: [makeClient({ name: "live-etl" })] } });
    renderClients();
    const row = await screen.findByRole("row", { name: /live-etl/ });
    expectChipTone(within(row).getByText("Active"), "success", "Clients: Active");
  });

  it("paints Expired with CHIP_TONE.destructive", async () => {
    mockClients({
      data: { status: 200, data: [makeClient({ name: "stale-etl", expires_at: "2020-01-01T00:00:00Z" })] },
    });
    renderClients();
    const row = await screen.findByRole("row", { name: /stale-etl/ });
    expectChipTone(within(row).getByText("Expired"), "destructive", "Clients: Expired");
  });

  it("leaves Revoked untinted — it is not a state to scan a list for", async () => {
    // The third arm, and the one a tone map tempts you to fill in. A revoked
    // client is already carried by the WORD and by the whole row going muted
    // (asserted above); giving it a wash would put it in the same visual
    // register as the two live states it is the end of.
    mockClients({
      data: { status: 200, data: [makeClient({ name: "old-etl", revoked_at: "2026-08-01T00:00:00Z" })] },
    });
    renderClients();
    const row = await screen.findByRole("row", { name: /old-etl/ });
    expectNoChipTone(within(row).getByText("Revoked"), "Clients: Revoked");
  });
});
