import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { toast } from "sonner";
import { ApiError } from "@/api/client";
import { credentialsQueryKey, type CreatedCredential, type Credential, type ServiceAccountDetailDto } from "@/api/serviceAccounts";
import { CredentialsPanel } from "@/routes/governance/service-accounts/CredentialsPanel";

const api = vi.hoisted(() => ({
  listCredentials: vi.fn(),
  createCredential: vi.fn(),
  rotateCredential: vi.fn(),
  revokeCredential: vi.fn(),
  setCredentialExpiry: vi.fn(),
}));
const cp = vi.hoisted(() => ({ listWarehouses: vi.fn(), listRoles: vi.fn() }));

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), warning: vi.fn() } }));
vi.mock("@/api/serviceAccounts", async (importActual) => ({
  ...(await importActual<typeof import("@/api/serviceAccounts")>()),
  ...api,
}));
vi.mock("@/api/controlPlane", async (importActual) => ({
  ...(await importActual<typeof import("@/api/controlPlane")>()),
  ...cp,
}));
// The real picker is a calendar popover; a plain field is enough to drive the value.
vi.mock("@/components/ui/date-time-picker", () => ({
  DATE_TIME_FORMAT: "YYYY-MM-DD HH:mm",
  DateTimePicker: ({ id, onChange }: { id?: string; onChange: (d: Date | null) => void }) => (
    <input id={id} aria-label="expiry-input" onChange={(e) => onChange(e.target.value ? new Date(e.target.value) : null)} />
  ),
}));

const FAKE_SECRET = "fake-secret-not-real-0000";

const SA: ServiceAccountDetailDto = {
  user_id: "sa-1",
  scope: "workspace",
  workspace_id: "ws-1",
  name: "ci-bot",
  description: "",
  created_at: "2026-03-04T00:00:00Z",
  status: "active",
  groups: [],
  roles: [],
};

const cred = (over: Partial<Credential>): Credential => ({
  id: "c-1",
  service_account_id: "sa-1",
  audience: "data-api",
  name: "api key",
  client_id: "client-1",
  status: "active",
  created_at: "2026-03-05T00:00:00Z",
  ...over,
});
const created = (over: Partial<Credential>): CreatedCredential => ({ ...cred(over), client_secret: FAKE_SECRET });

const CATALOG_CRED = cred({
  id: "c-2",
  audience: "catalog",
  name: "lake reader",
  catalog: { warehouse_id: "wh-1", catalog_role_id: "role-1", consumer_type: "engine" },
});
const REVOKED = cred({ id: "c-3", name: "old key", status: "revoked" });
const EXPIRED = cred({ id: "c-4", name: "stale key", status: "expired" });

function renderPanel(account: ServiceAccountDetailDto = SA) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const invalidate = vi.spyOn(qc, "invalidateQueries");
  render(
    <QueryClientProvider client={qc}>
      <CredentialsPanel account={account} />
    </QueryClientProvider>,
  );
  return { invalidate };
}

async function choose(user: ReturnType<typeof userEvent.setup>, label: string, option: string) {
  await user.click(await screen.findByRole("combobox", { name: label }));
  await user.click(await screen.findByRole("option", { name: option }));
}

beforeEach(() => {
  vi.resetAllMocks();
  api.listCredentials.mockResolvedValue([cred({}), CATALOG_CRED, REVOKED, EXPIRED]);
  cp.listWarehouses.mockResolvedValue([
    { id: "wh-1", name: "Lake", workspaceId: "ws-1", storageConnId: "s", lakekeeperWarehouseId: null },
    { id: "wh-other", name: "Elsewhere", workspaceId: "ws-2", storageConnId: "s", lakekeeperWarehouseId: null },
  ]);
  cp.listRoles.mockResolvedValue([
    { id: "role-1", name: "reader", consumerType: "engine", description: null },
    { id: "role-2", name: "ext-reader", consumerType: "external", description: null },
  ]);
});

describe("CredentialsPanel", () => {
  it("lists both audiences with audience and status badges, resolving catalog names", async () => {
    renderPanel();
    const row = async (name: string) => (await screen.findByText(name)).closest("tr") as HTMLElement;
    const apiRow = await row("api key");
    expect(within(apiRow).getByText("Data API")).toBeInTheDocument();
    expect(within(apiRow).getByText("Active")).toBeInTheDocument();
    const lakeRow = await row("lake reader");
    expect(within(lakeRow).getByText("Catalog")).toBeInTheDocument();
    await waitFor(() => expect(within(lakeRow).getByText(/Lake \/ reader/)).toBeInTheDocument());
    expect(within(await row("old key")).getByText("Revoked")).toBeInTheDocument();
    expect(within(await row("stale key")).getByText("Expired")).toBeInTheDocument();
  });

  it("shows the catalog / data-governance note", async () => {
    renderPanel();
    expect(await screen.findByText(/read the data lake \(Iceberg\) directly/)).toBeInTheDocument();
    expect(screen.getByText(/outside egret's data-governance masking/)).toBeInTheDocument();
  });

  it("shows an empty state and a retryable error", async () => {
    api.listCredentials.mockResolvedValueOnce([]);
    renderPanel();
    expect(await screen.findByText("No credentials yet.")).toBeInTheDocument();
  });

  it("surfaces a load failure", async () => {
    api.listCredentials.mockRejectedValue(new ApiError(500, "boom"));
    renderPanel();
    expect(await screen.findByText(/boom/)).toBeInTheDocument();
  });

  it("creates a Data API credential and shows the secret once with the warning", async () => {
    const user = userEvent.setup();
    api.createCredential.mockResolvedValue(created({ id: "c-9", name: "new" }));
    const { invalidate } = renderPanel();
    await user.click(await screen.findByRole("button", { name: "Create credential" }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "Create credential" }));
    await waitFor(() =>
      expect(api.createCredential).toHaveBeenCalledWith("sa-1", { audience: "data-api", name: null, expires_at: null, catalog: null }),
    );
    expect(await screen.findByText("This secret won't be shown again")).toBeInTheDocument();
    expect(screen.getByLabelText("Client secret")).toHaveValue(FAKE_SECRET);
    expect(screen.getByRole("button", { name: "Copy secret" })).toBeInTheDocument();
    expect(invalidate).toHaveBeenCalledWith({ queryKey: credentialsQueryKey("sa-1") });
    // Closing drops it for good.
    await user.click(screen.getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.queryByDisplayValue(FAKE_SECRET)).not.toBeInTheDocument());
    expect(screen.queryByText(FAKE_SECRET)).not.toBeInTheDocument();
  });

  it("requires warehouse, consumer type and role for a catalog credential", async () => {
    const user = userEvent.setup();
    api.createCredential.mockResolvedValue(created({ id: "c-9", audience: "catalog" }));
    renderPanel();
    await user.click(await screen.findByRole("button", { name: "Create credential" }));
    await choose(user, "Audience", "Catalog");
    const dialog = await screen.findByRole("dialog");
    const submit = within(dialog).getByRole("button", { name: "Create credential" });
    expect(submit).toBeDisabled();
    await choose(user, "Warehouse", "Lake");
    expect(screen.queryByRole("option", { name: "Elsewhere" })).not.toBeInTheDocument();
    expect(submit).toBeDisabled();
    await choose(user, "Consumer type", "Engine");
    expect(submit).toBeDisabled();
    await choose(user, "Catalog role", "reader");
    expect(submit).toBeEnabled();
    await user.click(submit);
    await waitFor(() =>
      expect(api.createCredential).toHaveBeenCalledWith("sa-1", {
        audience: "catalog",
        name: null,
        expires_at: null,
        catalog: { warehouse_id: "wh-1", catalog_role_id: "role-1", consumer_type: "engine" },
      }),
    );
    expect(await screen.findByText("This secret won't be shown again")).toBeInTheDocument();
  });

  it("disables the Data API audience, with a reason, for a platform-scoped account", async () => {
    const user = userEvent.setup();
    renderPanel({ ...SA, scope: "platform", workspace_id: null });
    await user.click(await screen.findByRole("button", { name: "Create credential" }));
    expect(await screen.findByText(/platform-scoped service account can't have one/)).toBeInTheDocument();
    await user.click(screen.getByRole("combobox", { name: "Audience" }));
    expect(await screen.findByRole("option", { name: "Data API" })).toHaveAttribute("aria-disabled", "true");
  });

  it("shows a refused create inline", async () => {
    const user = userEvent.setup();
    api.createCredential.mockRejectedValue(new ApiError(403, "Forbidden"));
    renderPanel();
    await user.click(await screen.findByRole("button", { name: "Create credential" }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "Create credential" }));
    expect(await within(dialog).findByText(/don't have permission/)).toBeInTheDocument();
    expect(screen.queryByText("This secret won't be shown again")).not.toBeInTheDocument();
  });

  it("rotates: shows the new secret once, and the refetch shows the old one revoked", async () => {
    const user = userEvent.setup();
    api.rotateCredential.mockResolvedValue(created({ id: "c-new", name: "api key (rotated)" }));
    renderPanel();
    await user.click(await screen.findByRole("button", { name: "Rotate api key" }));
    api.listCredentials.mockResolvedValue([cred({ status: "revoked" }), cred({ id: "c-new", name: "api key (rotated)" })]);
    await waitFor(() => expect(api.rotateCredential).toHaveBeenCalledWith("sa-1", "c-1"));
    expect(await screen.findByLabelText("Client secret")).toHaveValue(FAKE_SECRET);
    expect(screen.getByText("This secret won't be shown again")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.queryByLabelText("Client secret")).not.toBeInTheDocument());
  });

  it("shows a refused rotate (409) inline", async () => {
    const user = userEvent.setup();
    api.rotateCredential.mockRejectedValue(new ApiError(409, "Conflict"));
    renderPanel();
    await user.click(await screen.findByRole("button", { name: "Rotate api key" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/already revoked/);
  });

  it("revokes after a confirmation and refetches", async () => {
    const user = userEvent.setup();
    api.revokeCredential.mockResolvedValue(undefined);
    const { invalidate } = renderPanel();
    await user.click(await screen.findByRole("button", { name: "Revoke api key" }));
    expect(api.revokeCredential).not.toHaveBeenCalled();
    await user.type(await screen.findByPlaceholderText("revoke"), "revoke");
    await user.click(screen.getByRole("button", { name: "Revoke" }));
    await waitFor(() => expect(api.revokeCredential).toHaveBeenCalledWith("sa-1", "c-1"));
    await waitFor(() => expect(invalidate).toHaveBeenCalled());
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Credential revoked."));
  });

  it("sets and clears an expiry", async () => {
    const user = userEvent.setup();
    api.setCredentialExpiry.mockResolvedValue(undefined);
    const { invalidate } = renderPanel();
    await user.click(await screen.findByRole("button", { name: "Set expiry for api key" }));
    await user.type(await screen.findByLabelText("expiry-input"), "2030-01-02T03:04");
    await user.click(screen.getByRole("button", { name: "Save expiry" }));
    await waitFor(() =>
      expect(api.setCredentialExpiry).toHaveBeenCalledWith("sa-1", "c-1", { expires_at: new Date("2030-01-02T03:04").toISOString() }),
    );
    await waitFor(() => expect(invalidate).toHaveBeenCalled());
    await waitFor(() => expect(toast.success).toHaveBeenCalledWith("Expiry updated."));

    await user.click(await screen.findByRole("button", { name: "Set expiry for api key" }));
    await user.click(await screen.findByRole("button", { name: "Clear expiry" }));
    await waitFor(() => expect(api.setCredentialExpiry).toHaveBeenLastCalledWith("sa-1", "c-1", { expires_at: null }));
  });
});
