import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ServiceAccountDto } from "@/api/serviceAccounts";
import { TooltipProvider } from "@/components/ui/tooltip";
import ServiceAccountsRoutes from "@/routes/governance/service-accounts/ServiceAccountsRoutes";

const api = vi.hoisted(() => ({
  listServiceAccounts: vi.fn(),
  createServiceAccount: vi.fn(),
  disableServiceAccount: vi.fn(),
  enableServiceAccount: vi.fn(),
  deleteServiceAccount: vi.fn(),
}));

vi.mock("@/api/serviceAccounts", async (importActual) => ({
  ...(await importActual<typeof import("@/api/serviceAccounts")>()),
  ...api,
}));

vi.mock("@/api/generated/admin-workspaces/admin-workspaces", async (importActual) => ({
  ...(await importActual<typeof import("@/api/generated/admin-workspaces/admin-workspaces")>()),
  useAdminWorkspacesListWs: () => ({
    data: {
      status: 200,
      data: [
        { id: "ws-1", slug: "acme", name: "Acme", status: "active", created_at: "2026-01-01T00:00:00Z" },
        { id: "ws-2", slug: "beta", name: "Beta", status: "active", created_at: "2026-01-01T00:00:00Z" },
      ],
    },
  }),
}));

const sa = (over: Partial<ServiceAccountDto>): ServiceAccountDto => ({
  user_id: "sa-1",
  scope: "platform",
  workspace_id: null,
  name: "ci-bot",
  description: "",
  created_at: "2026-03-04T00:00:00Z",
  status: "active",
  ...over,
});

const ROWS = [
  sa({}),
  sa({ user_id: "sa-2", scope: "workspace", workspace_id: "ws-1", name: "etl", status: "disabled" }),
];

function Where() {
  const loc = useLocation();
  return <output data-testid="where">{loc.pathname + loc.search}</output>;
}
const where = () => screen.getByTestId("where").textContent;

function open(url = "/governance/service-accounts") {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const invalidate = vi.spyOn(qc, "invalidateQueries");
  render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MemoryRouter initialEntries={[url]}>
          <Routes>
            <Route path="/governance/service-accounts/*" element={<ServiceAccountsRoutes />} />
          </Routes>
          <Where />
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
  return { invalidate };
}

beforeEach(() => {
  api.listServiceAccounts.mockResolvedValue(ROWS);
});
afterEach(() => {
  vi.clearAllMocks();
});

describe("ServiceAccountsIndex", () => {
  it("lists the service accounts with scope, workspace and status", async () => {
    open();
    const row = (await screen.findByRole("link", { name: "etl" })).closest("tr") as HTMLElement;
    expect(within(row).getByText("Workspace", { selector: "span" })).toBeInTheDocument();
    expect(within(row).getByText("Acme")).toBeInTheDocument();
    expect(within(row).getByText("Disabled")).toBeInTheDocument();
    const first = screen.getByRole("link", { name: "ci-bot" }).closest("tr") as HTMLElement;
    expect(within(first).getByText("Active")).toBeInTheDocument();
  });

  it("shows an error with a retry when the list fails", async () => {
    api.listServiceAccounts.mockRejectedValue(new Error("boom"));
    open();
    expect(await screen.findByRole("button", { name: /retry/i })).toBeInTheDocument();
  });

  it("shows the empty state", async () => {
    api.listServiceAccounts.mockResolvedValue([]);
    open();
    expect(await screen.findByText("No service accounts.")).toBeInTheDocument();
  });

  it("puts the scope filter in the URL and queries with it", async () => {
    const user = userEvent.setup();
    open();
    await screen.findByRole("link", { name: "etl" });
    await user.click(screen.getByRole("combobox", { name: "Scope" }));
    await user.click(await screen.findByRole("option", { name: "Platform" }));
    await waitFor(() => expect(where()).toBe("/governance/service-accounts?scope=platform"));
    await waitFor(() => expect(api.listServiceAccounts).toHaveBeenLastCalledWith({ scope: "platform" }));
  });

  it("reads the filters from the URL", async () => {
    open("/governance/service-accounts?scope=workspace&workspace=ws-2");
    await screen.findByRole("link", { name: "etl" });
    expect(api.listServiceAccounts).toHaveBeenCalledWith({ scope: "workspace", workspace_id: "ws-2" });
  });

  it("requires a workspace when the scope is workspace, then creates, invalidates and opens the account", async () => {
    const user = userEvent.setup();
    api.createServiceAccount.mockResolvedValue(sa({ user_id: "sa-9", name: "new-bot", scope: "workspace", workspace_id: "ws-2" }));
    const { invalidate } = open();
    await screen.findByRole("link", { name: "etl" });
    await user.click(screen.getByRole("button", { name: "Create service account" }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("combobox", { name: "Account scope" }));
    await user.click(await screen.findByRole("option", { name: "Workspace" }));
    await user.type(within(dialog).getByLabelText("Name"), "new-bot");
    await user.click(within(dialog).getByRole("button", { name: "Create" }));
    expect(await within(dialog).findByText("Choose the workspace this account belongs to.")).toBeInTheDocument();
    expect(api.createServiceAccount).not.toHaveBeenCalled();

    await user.click(within(dialog).getByRole("combobox", { name: "Account workspace" }));
    await user.click(await screen.findByRole("option", { name: "Beta" }));
    await user.click(within(dialog).getByRole("button", { name: "Create" }));

    await waitFor(() =>
      expect(api.createServiceAccount).toHaveBeenCalledWith({
        scope: "workspace",
        workspace_id: "ws-2",
        name: "new-bot",
        description: null,
      }),
    );
    await waitFor(() => expect(invalidate).toHaveBeenCalled());
    await waitFor(() => expect(where()).toBe("/governance/service-accounts/sa-9"));
  });

  it("creates a platform account without a workspace", async () => {
    const user = userEvent.setup();
    api.createServiceAccount.mockResolvedValue(sa({ user_id: "sa-8", name: "p" }));
    open();
    await screen.findByRole("link", { name: "etl" });
    await user.click(screen.getByRole("button", { name: "Create service account" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).queryByLabelText("Account workspace")).not.toBeInTheDocument();
    await user.type(within(dialog).getByLabelText("Name"), "p");
    await user.click(within(dialog).getByRole("button", { name: "Create" }));
    await waitFor(() =>
      expect(api.createServiceAccount).toHaveBeenCalledWith({ scope: "platform", workspace_id: null, name: "p", description: null }),
    );
  });

  it("shows a server refusal inside the create dialog", async () => {
    const user = userEvent.setup();
    api.createServiceAccount.mockRejectedValue(new Error("name already taken"));
    open();
    await screen.findByRole("link", { name: "etl" });
    await user.click(screen.getByRole("button", { name: "Create service account" }));
    const dialog = await screen.findByRole("dialog");
    await user.type(within(dialog).getByLabelText("Name"), "ci-bot");
    await user.click(within(dialog).getByRole("button", { name: "Create" }));
    expect(await within(dialog).findByText("name already taken")).toBeInTheDocument();
  });

  it("disables an active account and enables a disabled one, invalidating the list", async () => {
    const user = userEvent.setup();
    api.disableServiceAccount.mockResolvedValue(undefined);
    api.enableServiceAccount.mockResolvedValue(undefined);
    const { invalidate } = open();
    await user.click(await screen.findByRole("button", { name: "Disable ci-bot" }));
    await waitFor(() => expect(api.disableServiceAccount).toHaveBeenCalledWith("sa-1"));
    await waitFor(() => expect(invalidate).toHaveBeenCalled());
    await user.click(screen.getByRole("button", { name: "Enable etl" }));
    await waitFor(() => expect(api.enableServiceAccount).toHaveBeenCalledWith("sa-2"));
  });

  it("surfaces a 409 from disable inline", async () => {
    const user = userEvent.setup();
    api.disableServiceAccount.mockRejectedValue(new Error("still in use"));
    open();
    await user.click(await screen.findByRole("button", { name: "Disable ci-bot" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("still in use");
  });

  it("deletes only after the confirmation, then invalidates", async () => {
    const user = userEvent.setup();
    api.deleteServiceAccount.mockResolvedValue(undefined);
    const { invalidate } = open();
    await user.click(await screen.findByRole("button", { name: "Delete etl" }));
    expect(api.deleteServiceAccount).not.toHaveBeenCalled();
    const dialog = await screen.findByRole("alertdialog");
    await user.type(within(dialog).getByRole("textbox"), "delete");
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(api.deleteServiceAccount).toHaveBeenCalledWith("sa-2"));
    await waitFor(() => expect(invalidate).toHaveBeenCalled());
  });
});
