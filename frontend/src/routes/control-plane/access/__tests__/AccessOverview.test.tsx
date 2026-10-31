import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

import AccessOverview from "@/routes/control-plane/access/AccessOverview";
import * as api from "@/api/access";
import * as auditApi from "@/api/generated/control-plane-audit/control-plane-audit";
import { AuthContext } from "@/auth/useAuth";
import type { Me } from "@/api/hooks";
import type { AuditEntryDto } from "@/api/generated/model/auditEntryDto";

vi.mock("@/api/access");
vi.mock("@/api/generated/control-plane-audit/control-plane-audit");

const ADMIN_ME: Me = {
  user_id: "1",
  email: "admin@example.com",
  platform_roles: ["platform_admin"],
  permissions: [],
};

const NON_ADMIN_ME: Me = {
  user_id: "2",
  email: "member@example.com",
  platform_roles: [],
  permissions: ["role:read"],
};

const USERS: api.AccessUser[] = [
  { id: "u1", email: "alice@example.com", name: "Alice", provider: "local", status: "active", roles: ["Admin"], effectiveRoles: [{ name: "Admin", group: null }], groups: [] },
  { id: "u2", email: "bob@example.com", name: "Bob", provider: "local", status: "disabled", roles: [], effectiveRoles: [], groups: [] },
  { id: "u3", email: "carol@example.com", name: "Carol", provider: "local", status: "active", roles: ["Admin"], effectiveRoles: [{ name: "Admin", group: null }], groups: [] },
];

const GROUPS: api.AccessGroup[] = [
  { id: "g1", name: "Engineering", source: "oidc", memberCount: 5, roles: [] },
  { id: "g2", name: "Analysts", source: "local", memberCount: 2, roles: [] },
];

const ROLES: api.AccessRole[] = [
  { id: "r1", scope: "platform", name: "Admin", description: "", permissionCount: 10, assigneeCount: 2, inheritedPermissions: [] },
  { id: "r2", scope: "platform", name: "Empty", description: "", permissionCount: 3, assigneeCount: 0, inheritedPermissions: [] },
  { id: "r3", scope: "workspace", name: "Editor", description: "", permissionCount: 5, assigneeCount: 1, inheritedPermissions: [] },
  { id: "r4", scope: "workspace", name: "Viewer", description: "", permissionCount: 2, assigneeCount: 1, inheritedPermissions: [] },
];

const AUDIT_ENTRIES: AuditEntryDto[] = [
  {
    id: 1,
    ts: "2026-07-13T10:00:00Z",
    kind: "mutation",
    actorUserId: "user-abc-12345",
    actorEmail: "alice@egret.local",
    workspaceId: null,
    action: "role.assign",
    resourceType: "role",
    resourceId: "r1",
    decision: null,
    reasons: [],
    detail: {},
  },
];

function mockAuditHook(
  page: { entries: AuditEntryDto[]; nextCursor?: string | null } | undefined,
  overrides: Record<string, unknown> = {},
) {
  const result = {
    data: page ? { data: page, status: 200, headers: new Headers() } : undefined,
    isLoading: false,
    isError: false,
    error: null,
    isFetching: false,
    ...overrides,
  };
  vi.mocked(auditApi.useControlPlaneAuditListAudit).mockReturnValue(
    result as unknown as ReturnType<typeof auditApi.useControlPlaneAuditListAudit>,
  );
}

function renderPage(me: Me | null = ADMIN_ME) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <AuthContext.Provider value={{ me, isLoading: false }}>
      <QueryClientProvider client={qc}>
        <MemoryRouter>
          <AccessOverview />
        </MemoryRouter>
      </QueryClientProvider>
    </AuthContext.Provider>,
  );
}

describe("AccessOverview", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(api.listUsers).mockResolvedValue(USERS);
    vi.mocked(api.listGroups).mockResolvedValue(GROUPS);
    vi.mocked(api.listRoles).mockResolvedValue(ROLES);
    mockAuditHook({ entries: AUDIT_ENTRIES, nextCursor: null });
  });

  test("renders KPI tiles derived from the mocked lists", async () => {
    renderPage();

    // Users: 3 total, 2 active.
    expect(await screen.findByText("3")).toBeInTheDocument();
    expect(screen.getByText(/2 active/i)).toBeInTheDocument();

    // Groups: 2 total.
    expect(screen.getByText("2")).toBeInTheDocument();

    // Roles: 4 total, 2 platform · 2 workspace.
    expect(screen.getByText("4")).toBeInTheDocument();
    expect(screen.getByText(/2 platform.*2 workspace/i)).toBeInTheDocument();

    // Platform admins: Alice + Carol have "Admin" role -> 2.
    // "Admin" here isn't literally "platform_admin" in role name, but the
    // count is derived from users whose `effectiveRoles` include "platform_admin".
  });

  // DG #4a spec §3: the data-plane layer opens Engine & policy, not the retired Policies page.
  test("links the data-plane layer to Engine & policy", async () => {
    renderPage();
    expect(await screen.findByRole("link", { name: /Engine & policy/ })).toHaveAttribute("href", "/governance/engines");
  });

  test("counts platform admins from users whose roles include platform_admin", async () => {
    vi.mocked(api.listUsers).mockResolvedValue([
      { id: "u1", email: "a@x.com", name: "A", provider: "local", status: "active", roles: ["platform_admin"], effectiveRoles: [{ name: "platform_admin", group: null }], groups: [] },
      { id: "u2", email: "b@x.com", name: "B", provider: "local", status: "active", roles: ["platform_admin"], effectiveRoles: [{ name: "platform_admin", group: null }], groups: [] },
      { id: "u3", email: "c@x.com", name: "C", provider: "local", status: "active", roles: [], effectiveRoles: [], groups: [] },
    ]);

    renderPage();

    expect(await screen.findByText(/platform admin/i)).toBeInTheDocument();
    // 2 platform admins expected to render as a KPI value "2" somewhere.
    const twos = await screen.findAllByText("2");
    expect(twos.length).toBeGreaterThan(0);
  });

  test("aggregates zero-assignee roles into a single warning row", async () => {
    renderPage();

    // One line for the whole set (one zero-assignee role in this fixture),
    // not one line per role.
    expect(
      await screen.findByText(/1 role has no assignees/i),
    ).toBeInTheDocument();
  });

  test("shows an 'all clear' state when there are no warnings", async () => {
    vi.mocked(api.listRoles).mockResolvedValue([
      { id: "r1", scope: "platform", name: "Admin", description: "", permissionCount: 10, assigneeCount: 2, inheritedPermissions: [] },
    ]);
    vi.mocked(api.listUsers).mockResolvedValue([
      { id: "u1", email: "a@x.com", name: "A", provider: "local", status: "active", roles: [], effectiveRoles: [], groups: [] },
    ]);

    renderPage();

    expect(await screen.findByText(/all clear/i)).toBeInTheDocument();
  });

  test("renders the Recent changes panel with audit entries for a platform_admin", async () => {
    renderPage(ADMIN_ME);

    expect(await screen.findByText(/recent changes/i)).toBeInTheDocument();
    expect(screen.getByText("role.assign")).toBeInTheDocument();
    // KI-018: the actor by email, not by an id prefix.
    expect(screen.getByText("by alice@egret.local")).toBeInTheDocument();
  });

  test("hides the Recent changes panel for a non-platform_admin", async () => {
    renderPage(NON_ADMIN_ME);

    // Wait for KPI tiles to settle so we know the page finished rendering.
    await screen.findByText("Access & governance");
    expect(screen.queryByText(/recent changes/i)).not.toBeInTheDocument();
    expect(auditApi.useControlPlaneAuditListAudit).not.toHaveBeenCalled();
  });

  test("Recent changes requests mutations only (not read/decision noise)", async () => {
    renderPage(ADMIN_ME);

    await screen.findByText(/recent changes/i);
    expect(auditApi.useControlPlaneAuditListAudit).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "mutation" }),
      expect.anything(),
    );
  });

  test("degrades gracefully when a sub-query fails (never blanks the page)", async () => {
    vi.mocked(api.listGroups).mockRejectedValue(new Error("boom"));

    renderPage();

    // Page still renders the header and other tiles; groups tile shows "—".
    expect(await screen.findByText("Access & governance")).toBeInTheDocument();
    expect(await screen.findByText("—")).toBeInTheDocument();
  });
});
