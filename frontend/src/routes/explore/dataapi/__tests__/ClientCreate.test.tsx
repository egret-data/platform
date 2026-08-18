import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// There is no msw in this repo — tests mock the API module directly and hand the
// hooks a return value with `vi.mocked(...).mockReturnValue(...)`, the same way
// `Clients.test.tsx` and `ServiceAccountCreate.test.tsx` do.
vi.mock("@/api/generated/data-api/data-api");

import * as api from "@/api/generated/data-api/data-api";
import type { DataApiServiceAccountDto } from "@/api/generated/model";
import ClientCreate from "@/routes/explore/dataapi/ClientCreate";

/**
 * THREE accounts, and the route below points at the middle one.
 *
 * A one-row fixture is the trap this project has paid for seven times: with a
 * single account, `accounts[0].name` — a page that ignores the id in the URL
 * entirely — passes the "names the account" test. The emails deliberately do
 * NOT contain the ids, so "the id is not on screen" stays an assertion about
 * the id and not about whichever field happens to embed it.
 */
const ACCOUNTS: DataApiServiceAccountDto[] = [
  {
    id: "sa-1",
    name: "reporting-bot",
    description: "",
    email: "reporting-bot@service.egret.local",
    created_at: "2026-08-01T00:00:00Z",
  },
  {
    id: "sa-2",
    name: "nightly-exporter",
    description: "Nightly finance export",
    email: "nightly-exporter@service.egret.local",
    created_at: "2026-08-02T00:00:00Z",
  },
  {
    id: "sa-3",
    name: "audit-sync",
    description: "",
    email: "audit-sync@service.egret.local",
    created_at: "2026-08-03T00:00:00Z",
  },
];

const issueMutateAsync = vi.fn();

/** What the SERVER returned, per test. No two tests use the same secret. */
function serverIssues(client: { client_id: string; client_secret: string; token_endpoint: string }) {
  issueMutateAsync.mockResolvedValue({
    status: 201,
    data: { id: "cl-new", ...client },
    headers: new Headers(),
  });
}

function renderCreate(ws = "ws1", accountId = "sa-2") {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const utils = render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[`/explore/${ws}/data-api/clients/${accountId}/new-client`]}>
        <Routes>
          {/* The real pattern `App.tsx` registers, `:workspace` and `:accountId`
              included. This repo has already shipped a `useParams().ws`-versus-
              `:workspace` mismatch that no test could see, because the test
              mounted a route tree the app did not have. */}
          <Route
            path="/explore/:workspace/data-api/clients/:accountId/new-client"
            element={<ClientCreate />}
          />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return { ...utils, qc };
}

async function fillAndCreate(name: string) {
  await userEvent.type(screen.getByLabelText("Name"), name);
  await userEvent.click(screen.getByRole("button", { name: /create client/i }));
}

beforeEach(() => {
  vi.clearAllMocks();
  // The one-shot marker lives in `sessionStorage`, which jsdom shares across
  // every test in this file — without this, the first test that issues a client
  // leaves every later test on the same route in the already-issued state.
  window.sessionStorage.clear();

  vi.mocked(api.useDataApiListServiceAccounts).mockReturnValue(
    { data: { status: 200, data: ACCOUNTS }, isLoading: false, isError: false, error: null } as unknown as
      ReturnType<typeof api.useDataApiListServiceAccounts>,
  );
  vi.mocked(api.useDataApiIssueClient).mockReturnValue(
    { mutateAsync: issueMutateAsync, isPending: false } as unknown as
      ReturnType<typeof api.useDataApiIssueClient>,
  );
  // Automocking turns the query-key builder into a `vi.fn()` returning
  // `undefined`; an argument-sensitive fake is what lets a test tell "THIS
  // account's client list was invalidated" from "something was".
  vi.mocked(api.getDataApiListClientsQueryKey).mockImplementation(
    (ws, id) => ["dataApiListClients", ws, id] as unknown as
      ReturnType<typeof api.getDataApiListClientsQueryKey>,
  );

  serverIssues({
    client_id: "client_default",
    client_secret: "cs_default_unused",
    token_endpoint: "https://egret.local/oauth/token",
  });
});

describe("ClientCreate", () => {
  /**
   * The URL carries an id; a credential is issued FOR something, and an id is
   * not something a reader recognises. Asserted twice, over two different
   * accounts in two fresh mounts: with a single mount, a hardcoded
   * `"nightly-exporter"` — or `accounts[0].name` — satisfies the assertion.
   */
  it("names the account it is issuing for, not the first row and not the raw id", () => {
    const first = renderCreate("ws1", "sa-2");
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("nightly-exporter");
    expect(document.body.textContent).not.toContain("reporting-bot");
    expect(document.body.textContent).not.toContain("sa-2");
    first.unmount();

    renderCreate("ws1", "sa-3");
    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("audit-sync");
    expect(document.body.textContent).not.toContain("nightly-exporter");
    expect(document.body.textContent).not.toContain("sa-3");
  });

  /**
   * The created state: the form is GONE, replaced in place by the credential.
   * Every rendered value is the one the response carried — a page that printed
   * its own placeholder, or the request's own name, would pass a
   * `getByText(/cs_/)`-shaped assertion and fail these.
   */
  it("replaces the form with the credential the server returned, plus a copy control and a way back", async () => {
    serverIssues({
      client_id: "client_alpha_1",
      client_secret: "cs_live_alpha_7a1f",
      token_endpoint: "https://alpha.egret.local/oauth/token",
    });
    renderCreate();
    await fillAndCreate("etl-alpha");

    expect(await screen.findByTestId("client-secret")).toHaveTextContent("cs_live_alpha_7a1f");
    expect(screen.getByTestId("client-id")).toHaveTextContent("client_alpha_1");
    expect(screen.getByTestId("token-endpoint")).toHaveTextContent("https://alpha.egret.local/oauth/token");

    // Exactly one copy affordance, and it is for the secret — the id and the
    // token endpoint are plain text, so `/copy/i` is unambiguous for a reader
    // and for this query.
    expect(screen.getAllByRole("button", { name: /^copy$/i })).toHaveLength(1);

    // The form is replaced, not merely scrolled past.
    expect(screen.queryByLabelText("Name")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /create client/i })).not.toBeInTheDocument();

    // A page cannot be dismissed by an outside click, but it must still be
    // leavable — and by a real link, so it can be middle-clicked.
    expect(screen.getByRole("link", { name: /done/i })).toHaveAttribute(
      "href",
      "/explore/ws1/data-api/clients",
    );
  });

  /**
   * THE property. The secret must never be refetchable.
   *
   * The remount is a FRESH tree, which is what a browser reload does — asserted
   * on the same component instance, a `useState` that happens to be empty would
   * pass while proving nothing. The pre-remount half is what makes the value
   * falsifiable: it is a DIFFERENT secret from the test above, so no single
   * hardcoded constant satisfies both.
   */
  it("shows the secret once: a fresh mount of the same route says it is gone, and does not re-issue", async () => {
    serverIssues({
      client_id: "client_beta_2",
      client_secret: "cs_live_beta_2b9c",
      token_endpoint: "https://beta.egret.local/oauth/token",
    });
    const first = renderCreate("ws1", "sa-2");
    await fillAndCreate("etl-beta");
    expect(await screen.findByTestId("client-secret")).toHaveTextContent("cs_live_beta_2b9c");

    first.unmount();
    renderCreate("ws1", "sa-2");

    // An honest state, named in the heading — not the created state with a
    // blank value in it, and not a silently re-offered form.
    expect(
      await screen.findByRole("heading", { level: 1, name: /no longer available/i }),
    ).toBeInTheDocument();
    expect(screen.getByText(/cannot be shown again/i)).toBeInTheDocument();

    // Not an empty field: there is no secret element at all, and no input
    // masquerading as one.
    expect(screen.queryByTestId("client-secret")).not.toBeInTheDocument();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^copy$/i })).not.toBeInTheDocument();
    expect(document.body.textContent).not.toContain("cs_live_beta_2b9c");

    // And no stale secret AND no fresh one: a reload must not mint a second
    // credential behind the reader's back.
    expect(issueMutateAsync).toHaveBeenCalledTimes(1);
  });

  /**
   * The other half of the one-shot state: a URL you cannot leave forward is
   * worse than a dialog. The recovery is explicit and it does NOT issue
   * anything on its own.
   */
  it("offers a way to issue another client rather than dead-ending on the spent route", async () => {
    serverIssues({
      client_id: "client_gamma_3",
      client_secret: "cs_live_gamma_5d4e",
      token_endpoint: "https://gamma.egret.local/oauth/token",
    });
    const first = renderCreate("ws1", "sa-2");
    await fillAndCreate("etl-gamma");
    await screen.findByTestId("client-secret");
    first.unmount();

    renderCreate("ws1", "sa-2");
    await screen.findByRole("heading", { level: 1, name: /no longer available/i });
    await userEvent.click(screen.getByRole("button", { name: /issue another client/i }));

    // Back to an empty form, and still exactly one issue call: the recovery is
    // an affordance, not a second mint.
    expect(screen.getByLabelText("Name")).toHaveValue("");
    expect(issueMutateAsync).toHaveBeenCalledTimes(1);
  });

  it("keeps every value and shows the server's message when the issue fails", async () => {
    issueMutateAsync.mockRejectedValue(new Error("a client with that name already exists"));
    renderCreate();

    await userEvent.type(screen.getByLabelText("Name"), "half-typed-etl");
    await userEvent.type(screen.getByLabelText("Expires (optional)"), "2027-03-04");
    await userEvent.click(screen.getByRole("button", { name: /create client/i }));

    // In a live region, so it is announced and not merely painted.
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(/a client with that name already exists/i);
    // A dialog could be dismissed and retried; a page that clears itself loses
    // work the author cannot get back.
    expect(screen.getByLabelText("Name")).toHaveValue("half-typed-etl");
    expect(screen.getByLabelText("Expires (optional)")).toHaveValue("2027-03-04");
    expect(screen.queryByTestId("client-secret")).not.toBeInTheDocument();
  });

  /**
   * A failed issue must leave NOTHING behind. Marking the route as spent before
   * (or regardless of) success would lock the author out of the only page that
   * can issue this account a client, with no credential to show for it.
   */
  it("leaves no trace when the issue fails: a fresh mount still offers the form", async () => {
    issueMutateAsync.mockRejectedValue(new Error("nope"));
    const first = renderCreate("ws1", "sa-2");
    await fillAndCreate("doomed-etl");
    await screen.findByRole("alert");
    first.unmount();

    renderCreate("ws1", "sa-2");
    expect(screen.getByLabelText("Name")).toBeInTheDocument();
    expect(screen.queryByRole("heading", { level: 1, name: /no longer available/i })).not.toBeInTheDocument();
  });

  /**
   * Mounted on a DIFFERENT workspace and a different account from every other
   * test here: `const ws = "ws1"` / `accounts[0].id` — a page that ignored the
   * route — passes all of them.
   */
  it("sends the workspace and account id from the route, a trimmed name, and no expiry key when blank", async () => {
    renderCreate("other-ws", "sa-3");

    await userEvent.type(screen.getByLabelText("Name"), "  etl-nightly  ");
    await userEvent.click(screen.getByRole("button", { name: /create client/i }));

    await waitFor(() => expect(issueMutateAsync).toHaveBeenCalledTimes(1));
    expect(issueMutateAsync.mock.calls[0][0].workspace).toBe("other-ws");
    expect(issueMutateAsync.mock.calls[0][0].id).toBe("sa-3");
    // `toEqual` on the whole body: blank expiry means the key is ABSENT, not
    // `""` and not `undefined`. Asserting only the typed-date case would move
    // which constant is unfalsifiable instead of removing it.
    expect(issueMutateAsync.mock.calls[0][0].data).toEqual({ name: "etl-nightly" });
  });

  it("sends a picked expiry as an ISO instant", async () => {
    renderCreate();

    await userEvent.type(screen.getByLabelText("Name"), "etl-expiring");
    await userEvent.type(screen.getByLabelText("Expires (optional)"), "2027-03-04");
    await userEvent.click(screen.getByRole("button", { name: /create client/i }));

    await waitFor(() => expect(issueMutateAsync).toHaveBeenCalledTimes(1));
    expect(issueMutateAsync.mock.calls[0][0].data).toEqual({
      name: "etl-expiring",
      expires_at: new Date("2027-03-04").toISOString(),
    });
  });

  it("cannot be submitted until a name is typed", async () => {
    renderCreate();
    const create = screen.getByRole("button", { name: /create client/i });
    expect(create).toBeDisabled();
    // Whitespace is not a name: the gate trims, so it must still hold.
    await userEvent.type(screen.getByLabelText("Name"), "   ");
    expect(create).toBeDisabled();
    await userEvent.type(screen.getByLabelText("Name"), "etl");
    expect(create).toBeEnabled();
  });

  /**
   * The new client must be in the table behind this page when the author walks
   * back to it. The key is per service account, so "something was invalidated"
   * is not enough — it must be THIS account's list, in THIS workspace.
   */
  it("invalidates that account's client list so the new client is in the table behind it", async () => {
    const { qc } = renderCreate("other-ws", "sa-3");
    const spy = vi.spyOn(qc, "invalidateQueries");
    await fillAndCreate("etl-invalidate");
    await screen.findByTestId("client-secret");

    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ queryKey: ["dataApiListClients", "other-ws", "sa-3"] }),
    );
  });

  it("renders exactly one h1 in the form state, the created state, and the spent state", async () => {
    const first = renderCreate();
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    first.unmount();

    window.sessionStorage.clear();
    const second = renderCreate();
    await fillAndCreate("etl-one-h1");
    await screen.findByTestId("client-secret");
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
    second.unmount();

    // A fresh mount of the same route, marker already written by the mint
    // above — the spent state, which `ListPageShell` alone renders a heading
    // for. This is the state a real reload lands on, and the one this
    // invariant had not yet been asserted for.
    renderCreate();
    await screen.findByRole("heading", { level: 1, name: /no longer available/i });
    expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
  });
});
