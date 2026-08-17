import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// There is no msw in this repo — tests mock the API module directly (see
// Endpoints.test.tsx / AuditLog.test.tsx).
vi.mock("@/api/generated/data-api/data-api");

import * as api from "@/api/generated/data-api/data-api";
import type { DataApiClientDto, DataApiServiceAccountDto } from "@/api/generated/model";
import Clients from "@/routes/explore/dataapi/Clients";

// The one rule this task exists to hold: `DataApiClientDto` (the list shape)
// has no secret field — not a skipped one, an absent one. This string stands
// in for one; it is planted directly on the raw mock object (bypassing the
// DTO type via a cast) so a mutation that makes the row renderer reach past
// the declared fields and print it would be caught here, even though nothing
// in the real backend response could ever carry it.
const SECRET_FIXTURE = "sk_live_do_not_leak_9f3c2b";

const SERVICE_ACCOUNT: DataApiServiceAccountDto = {
  id: "sa-1",
  name: "reporting-bot",
  description: "",
  email: "sa-1@service.egret.local",
  created_at: "2026-08-01T00:00:00Z",
};

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

const createAccountMutateAsync = vi.fn();
const deleteAccountMutateAsync = vi.fn();
const issueMutateAsync = vi.fn();
const revokeMutateAsync = vi.fn();

function renderClients() {
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

  vi.mocked(api.useDataApiCreateServiceAccount).mockReturnValue(
    { mutateAsync: createAccountMutateAsync, isPending: false } as unknown as
      ReturnType<typeof api.useDataApiCreateServiceAccount>,
  );
  vi.mocked(api.useDataApiDeleteServiceAccount).mockReturnValue(
    { mutateAsync: deleteAccountMutateAsync, isPending: false } as unknown as
      ReturnType<typeof api.useDataApiDeleteServiceAccount>,
  );
  vi.mocked(api.useDataApiIssueClient).mockReturnValue(
    { mutateAsync: issueMutateAsync, isPending: false } as unknown as
      ReturnType<typeof api.useDataApiIssueClient>,
  );
  vi.mocked(api.useDataApiRevokeClient).mockReturnValue(
    { mutateAsync: revokeMutateAsync, isPending: false } as unknown as
      ReturnType<typeof api.useDataApiRevokeClient>,
  );

  // Automocking turns the query-key builder into a `vi.fn()` returning
  // `undefined`; an argument-sensitive fake is what lets a test tell "the
  // service-account list was invalidated" from "something was".
  vi.mocked(api.getDataApiListServiceAccountsQueryKey).mockImplementation(
    (ws) => ["dataApiListServiceAccounts", ws] as unknown as
      ReturnType<typeof api.getDataApiListServiceAccountsQueryKey>,
  );

  createAccountMutateAsync.mockResolvedValue({
    status: 201,
    data: { ...SERVICE_ACCOUNT, id: "sa-new" },
    headers: new Headers(),
  });
  deleteAccountMutateAsync.mockResolvedValue({ status: 200, data: { ok: true }, headers: new Headers() });
  issueMutateAsync.mockResolvedValue({
    status: 201,
    data: {
      id: "cl-new",
      client_id: "client_new123",
      client_secret: SECRET_FIXTURE,
      token_endpoint: "https://egret.local/oauth/token",
    },
    headers: new Headers(),
  });
  revokeMutateAsync.mockResolvedValue({ status: 200, data: { ok: true }, headers: new Headers() });
});

describe("Clients", () => {
  it("shows an error state, not the first-run CTA, when the list fails", async () => {
    mockAccounts({ isError: true });
    renderClients();
    expect(await screen.findByText(/could not load/i)).toBeInTheDocument();
    expect(screen.queryByText(/no service accounts yet/i)).not.toBeInTheDocument();
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
    // "etl" (the client) renders in the same row as its owning account, with
    // no expand/select step required to reveal it.
    expect(screen.getByText("etl")).toBeInTheDocument();
  });

  it("shows the secret once, with a copy button and a warning", async () => {
    renderClients();
    await userEvent.click(await screen.findByRole("button", { name: /new client/i }));
    await userEvent.type(screen.getByLabelText("Name"), "etl");
    await userEvent.click(screen.getByRole("button", { name: /create/i }));
    expect(await screen.findByText(/cannot be shown again/i)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /copy/i })).toBeInTheDocument();
  });

  it("does not show the secret anywhere in the list", async () => {
    renderClients();
    await screen.findByText("etl");
    expect(document.body.textContent).not.toContain(SECRET_FIXTURE);
  });

  it("issuing a client sends the owning account id and trimmed name, with no expiry by default", async () => {
    const user = userEvent.setup();
    renderClients();
    await user.click(await screen.findByRole("button", { name: /new client/i }));
    await user.type(screen.getByLabelText("Name"), "etl-2");
    await user.click(screen.getByRole("button", { name: /^create$/i }));

    await waitFor(() => {
      expect(issueMutateAsync).toHaveBeenCalledWith({
        workspace: "eng",
        id: "sa-1",
        data: { name: "etl-2", expires_at: undefined },
      });
    });
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
    expect(screen.getByText(/loading/i)).toBeInTheDocument();
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
    await screen.findByText("etl");
    expect(screen.getByText(/expires/i)).toBeInTheDocument();
    expect(screen.getByText(/last used/i)).toBeInTheDocument();
  });

  it("shows a Revoked badge and no Revoke action for an already-revoked client", async () => {
    mockClients({
      data: { status: 200, data: [makeClient({ name: "old-etl", revoked_at: "2026-08-01T00:00:00Z" })] },
    });
    renderClients();
    await screen.findByText("old-etl");
    expect(screen.getByText("Revoked")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /revoke old-etl/i })).not.toBeInTheDocument();
  });

  it("revoking a client goes through ConfirmDeleteDialog (type-to-confirm) and calls useDataApiRevokeClient", async () => {
    const user = userEvent.setup();
    renderClients();
    await screen.findByText("etl");

    await user.click(screen.getByRole("button", { name: /revoke etl/i }));
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

    await user.click(screen.getByRole("button", { name: /delete reporting-bot/i }));
    expect(await screen.findByText(/grants and every client/i)).toBeInTheDocument();

    const input = screen.getByPlaceholderText("delete");
    await user.type(input, "delete");
    await user.click(screen.getByRole("button", { name: /^delete$/i }));

    await waitFor(() => {
      expect(deleteAccountMutateAsync).toHaveBeenCalledWith({ workspace: "eng", id: "sa-1" });
    });
  });

  it("creating a service account collects a name and calls useDataApiCreateServiceAccount", async () => {
    const user = userEvent.setup();
    renderClients();

    await user.click(screen.getByRole("button", { name: /service account/i }));
    await screen.findByText("New service account");
    await user.type(screen.getByLabelText("Name"), "new-bot");
    await user.click(screen.getByRole("button", { name: /^create$/i }));

    await waitFor(() => {
      expect(createAccountMutateAsync).toHaveBeenCalledWith({
        workspace: "eng",
        data: { name: "new-bot", description: undefined },
      });
    });
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
 *
 * These drive the `onSuccess` the page's mutations were constructed with —
 * the one `src/api/dataApi.ts` attaches — the same way `EndpointDetail.test.tsx`
 * pins Save's.
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

  it("creating a service account invalidates the list every picker on this feature reads", async () => {
    const { qc } = renderClients();
    const spy = vi.spyOn(qc, "invalidateQueries");
    await fireSuccess(vi.mocked(api.useDataApiCreateServiceAccount), { workspace: "eng", data: {} });
    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ queryKey: ["dataApiListServiceAccounts", "eng"] }),
    );
  });

  it("deleting a service account invalidates that same list, so the row leaves the table", async () => {
    const { qc } = renderClients();
    const spy = vi.spyOn(qc, "invalidateQueries");
    await fireSuccess(vi.mocked(api.useDataApiDeleteServiceAccount), { workspace: "eng", id: "sa-1" });
    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ queryKey: ["dataApiListServiceAccounts", "eng"] }),
    );
  });

  // The workspace is read from the mutation VARIABLES, never from a separate
  // hook argument that could drift from where the write went.
  it("invalidates the list of the workspace the write actually went to", async () => {
    const { qc } = renderClients();
    const spy = vi.spyOn(qc, "invalidateQueries");
    await fireSuccess(vi.mocked(api.useDataApiCreateServiceAccount), { workspace: "other-ws", data: {} });
    expect(spy).toHaveBeenCalledWith(
      expect.objectContaining({ queryKey: ["dataApiListServiceAccounts", "other-ws"] }),
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
    expect(h1.className).toContain("text-lg");
    expect(screen.queryByRole("heading", { level: 2, name: "Clients" })).not.toBeInTheDocument();
  });

  it("carries its own p-4 padding, like every other top-level Explore list page", async () => {
    const { container } = renderClients();
    await screen.findByRole("heading", { level: 1, name: "Clients" });
    expect(container.firstElementChild?.className ?? "").toMatch(/\bp-4\b/);
  });
});
