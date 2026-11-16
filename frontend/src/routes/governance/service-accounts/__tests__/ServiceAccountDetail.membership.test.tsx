import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ApiError } from "@/api/client";
import type { ServiceAccountDetailDto } from "@/api/serviceAccounts";
import { TooltipProvider } from "@/components/ui/tooltip";
import ServiceAccountsRoutes from "@/routes/governance/service-accounts/ServiceAccountsRoutes";

const api = vi.hoisted(() => ({
  getServiceAccount: vi.fn(),
  addServiceAccountGroup: vi.fn(),
  removeServiceAccountGroup: vi.fn(),
  addServiceAccountRole: vi.fn(),
  removeServiceAccountRole: vi.fn(),
}));
const access = vi.hoisted(() => ({ listGroups: vi.fn(), listRoles: vi.fn() }));

vi.mock("@/api/serviceAccounts", async (importActual) => ({
  ...(await importActual<typeof import("@/api/serviceAccounts")>()),
  ...api,
}));
vi.mock("@/api/access", async (importActual) => ({
  ...(await importActual<typeof import("@/api/access")>()),
  ...access,
}));
vi.mock("@/api/generated/admin-workspaces/admin-workspaces", async (importActual) => ({
  ...(await importActual<typeof import("@/api/generated/admin-workspaces/admin-workspaces")>()),
  useAdminWorkspacesListWs: () => ({
    data: { status: 200, data: [{ id: "ws-1", slug: "acme", name: "Acme", status: "active", created_at: "2026-01-01T00:00:00Z" }] },
  }),
}));

const SA: ServiceAccountDetailDto = {
  user_id: "sa-1",
  scope: "workspace",
  workspace_id: "ws-1",
  name: "ci-bot",
  description: "Runs the nightly build",
  created_at: "2026-03-04T00:00:00Z",
  status: "active",
  groups: ["etl"],
  roles: ["viewer"],
};

const group = (id: string, name: string) => ({ id, name, source: "local", memberCount: 0, roles: [] });
const role = (id: string, name: string, scope = "platform") => ({
  id,
  name,
  scope,
  description: "",
  permissionCount: 0,
  assigneeCount: 0,
  inheritedPermissions: [],
});

function open() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const invalidate = vi.spyOn(qc, "invalidateQueries");
  render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MemoryRouter initialEntries={["/governance/service-accounts/sa-1"]}>
          <Routes>
            <Route path="/governance/service-accounts/*" element={<ServiceAccountsRoutes />} />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
  return { invalidate };
}

async function choose(user: ReturnType<typeof userEvent.setup>, label: string, option: string) {
  await user.click(await screen.findByRole("combobox", { name: label }));
  await user.click(await screen.findByRole("option", { name: option }));
}

beforeEach(() => {
  api.getServiceAccount.mockResolvedValue(SA);
  api.addServiceAccountGroup.mockResolvedValue(undefined);
  api.removeServiceAccountGroup.mockResolvedValue(undefined);
  api.addServiceAccountRole.mockResolvedValue(undefined);
  api.removeServiceAccountRole.mockResolvedValue(undefined);
  access.listGroups.mockResolvedValue([group("g-1", "etl"), group("g-2", "analysts"), group("g-3", "okta-sync")]);
  access.listRoles.mockResolvedValue([
    role("r-1", "viewer"),
    role("r-2", "editor"),
    role("r-3", "platform_admin"),
    role("r-4", "ws-role", "workspace"),
  ]);
});
afterEach(() => vi.clearAllMocks());

describe("ServiceAccountDetail identity and membership", () => {
  it("shows the identity and the current memberships", async () => {
    open();
    expect(await screen.findByRole("heading", { level: 1, name: "ci-bot" })).toBeInTheDocument();
    expect(screen.getByText("Runs the nightly build")).toBeInTheDocument();
    expect(screen.getAllByText("Workspace").length).toBeGreaterThan(0);
    expect(screen.getByText("Acme")).toBeInTheDocument();
    expect(screen.getByText("Active")).toBeInTheDocument();
    expect(screen.getByText(/never signs in and can never hold platform_admin/)).toBeInTheDocument();
    expect(await screen.findByRole("button", { name: "Remove etl" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Remove viewer" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Credentials" })).toBeInTheDocument();
  });

  it("adds a group and invalidates the account", async () => {
    const user = userEvent.setup();
    const { invalidate } = open();
    await choose(user, "Add to group", "analysts");
    await user.click(screen.getByRole("button", { name: "Add group" }));
    await waitFor(() => expect(api.addServiceAccountGroup).toHaveBeenCalledWith("sa-1", { group_id: "g-2" }));
    await waitFor(() => expect(invalidate).toHaveBeenCalled());
  });

  it("adds a platform role, offering no workspace roles", async () => {
    const user = userEvent.setup();
    const { invalidate } = open();
    await user.click(await screen.findByRole("combobox", { name: "Grant role" }));
    expect(screen.queryByRole("option", { name: "ws-role" })).not.toBeInTheDocument();
    await user.click(await screen.findByRole("option", { name: "editor" }));
    await user.click(screen.getByRole("button", { name: "Add role" }));
    await waitFor(() =>
      expect(api.addServiceAccountRole).toHaveBeenCalledWith("sa-1", { role_id: "r-2", scope_type: "platform" }),
    );
    await waitFor(() => expect(invalidate).toHaveBeenCalled());
  });

  it("shows an OIDC-owned group's 409 inline", async () => {
    api.addServiceAccountGroup.mockRejectedValue(
      new ApiError(409, "membership owned by the identity provider", undefined, { error: "membership owned by the identity provider" }),
    );
    const user = userEvent.setup();
    open();
    await choose(user, "Add to group", "okta-sync");
    await user.click(screen.getByRole("button", { name: "Add group" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("membership owned by the identity provider");
  });

  it("shows the platform_admin refusal inline, with a fallback when the body has no message", async () => {
    api.addServiceAccountRole.mockRejectedValue(new ApiError(400, "Bad Request"));
    const user = userEvent.setup();
    open();
    await choose(user, "Grant role", "platform_admin");
    await user.click(screen.getByRole("button", { name: "Add role" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(/can never hold platform_admin, directly or through a group/);
    expect(screen.getByRole("button", { name: "Remove viewer" })).toBeInTheDocument();
  });

  it("removes a group and a role by id", async () => {
    const user = userEvent.setup();
    const { invalidate } = open();
    await user.click(await screen.findByRole("button", { name: "Remove etl" }));
    await waitFor(() => expect(api.removeServiceAccountGroup).toHaveBeenCalledWith("sa-1", "g-1"));
    await user.click(screen.getByRole("button", { name: "Remove viewer" }));
    await waitFor(() => expect(api.removeServiceAccountRole).toHaveBeenCalledWith("sa-1", "r-1"));
    await waitFor(() => expect(invalidate).toHaveBeenCalled());
  });

  it("says so, with a retry, when the account fails to load", async () => {
    api.getServiceAccount.mockRejectedValue(new Error("boom"));
    open();
    expect(await screen.findByRole("button", { name: /retry/i })).toBeInTheDocument();
  });
});
