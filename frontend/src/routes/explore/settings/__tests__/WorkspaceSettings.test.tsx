import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { MemoryRouter, Routes, Route, Navigate } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";

// ── Mock factories ────────────────────────────────────────────────────────────

const roleCatalogMock = vi.fn();
const sharesMock = vi.fn();
const membersMock = vi.fn();
const rolesMock = vi.fn();
const authMock = vi.fn();

vi.mock("@/api/workspaceAccess", () => ({
  useWorkspaceRoleCatalog: (...args: unknown[]) => roleCatalogMock(...args),
  useWorkspaceShares: (...args: unknown[]) => sharesMock(...args),
}));

vi.mock("@/api/members", () => ({
  membersKey: (ws: string) => ["ws", ws, "members"],
  useWorkspaceMembers: (...args: unknown[]) => membersMock(...args),
  useWorkspaceRoles: () => rolesMock(),
  useAddMember: () => ({ mutate: vi.fn(), mutateAsync: vi.fn(), isPending: false }),
  useRemoveMember: () => ({ mutate: vi.fn(), mutateAsync: vi.fn(), isPending: false }),
  useWorkspaceInvites: () => ({ data: [], isLoading: false, error: null }),
  useDeleteInvite: () => ({ mutate: vi.fn(), isPending: false }),
}));

vi.mock("@/api/hooks", async () => ({
  ...(await vi.importActual<typeof import("@/api/hooks")>("@/api/hooks")),
  useAuthConfig: () => ({ data: { oidc_enabled: false } }),
}));

vi.mock("@/api/policies", () => ({
  listPrincipals: vi.fn(() => Promise.resolve([])),
}));

vi.mock("@/auth/useAuth", () => ({
  useAuth: () => authMock(),
}));

vi.mock("@/components/sharing/ShareDialog", () => ({
  ShareDialog: () => null,
}));

const requestsMock = vi.fn();

vi.mock("@/api/accessRequests", () => ({
  useWorkspaceRequests: (...args: unknown[]) => requestsMock(...args),
  useApproveRequest: () => ({ mutate: vi.fn(), isPending: false }),
  useDenyRequest: () => ({ mutate: vi.fn(), isPending: false }),
}));

import WorkspaceSettings from "@/routes/explore/settings/WorkspaceSettings";
import Members from "@/routes/explore/settings/Members";
import RolesPermissions from "@/routes/explore/settings/RolesPermissions";
import SharedObjects from "@/routes/explore/settings/SharedObjects";
import AccessRequests from "@/routes/explore/settings/AccessRequests";

// ── Render helper ─────────────────────────────────────────────────────────────

function renderAt(path: string) {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MemoryRouter initialEntries={[path]}>
          <Routes>
            <Route path="/explore/:workspace/settings" element={<WorkspaceSettings />}>
              <Route index element={<Navigate to="members" replace />} />
              <Route path="members" element={<Members />} />
              <Route path="roles" element={<RolesPermissions />} />
              <Route path="shares" element={<SharedObjects />} />
              <Route path="requests" element={<AccessRequests />} />
            </Route>
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

// ── Setup ─────────────────────────────────────────────────────────────────────

beforeEach(() => {
  roleCatalogMock.mockReset();
  sharesMock.mockReset();
  membersMock.mockReset();
  rolesMock.mockReset();
  authMock.mockReset();
  requestsMock.mockReset();

  roleCatalogMock.mockReturnValue({ data: [], isLoading: false, isError: false, error: null });
  sharesMock.mockReturnValue({ data: [], isLoading: false, isError: false, error: null });
  requestsMock.mockReturnValue({ data: [], isLoading: false, isError: false, error: null });
  membersMock.mockReturnValue({ data: [], isLoading: false, isError: false, error: null });
  rolesMock.mockReturnValue({ data: [], isLoading: false, isError: false });
  authMock.mockReturnValue({
    me: { user_id: "me-id", email: "me@egret.local", platform_roles: [], permissions: [] },
    isLoading: false,
  });
});

// ── Tests ─────────────────────────────────────────────────────────────────────

// The shell renders two navs — a grouped vertical one for desktop and a
// horizontal strip for mobile — and CSS (absent in jsdom) shows only one at a
// time. So each section link appears twice here; query with getAllByRole and
// assert across every copy.
const links = (name: string) => screen.getAllByRole("link", { name });
const isActive = (el: Element) => el.getAttribute("aria-current") === "page";

describe("WorkspaceSettings section navigation", () => {
  it("renders every section link", () => {
    renderAt("/explore/demo/settings/members");

    for (const name of ["Members", "Roles & permissions", "Shared objects", "Access requests"]) {
      expect(links(name).length).toBeGreaterThan(0);
    }
  });

  it("marks Roles & permissions active at /roles, and Members not active", () => {
    renderAt("/explore/demo/settings/roles");

    expect(links("Roles & permissions").every(isActive)).toBe(true);
    expect(links("Members").some(isActive)).toBe(false);

    // Not color-only: the active section is distinguished by more than color
    // (weight/background/border), so its class differs structurally from idle.
    expect(links("Roles & permissions")[0].className).not.toEqual(links("Members")[0].className);

    // The Roles & permissions page itself renders.
    expect(screen.getByText(/what each workspace role can do/i)).toBeInTheDocument();
  });

  it("redirects the settings index to members", () => {
    renderAt("/explore/demo/settings");

    expect(links("Members").some(isActive)).toBe(true);
  });

  it("marks Shared objects active at /shares", () => {
    renderAt("/explore/demo/settings/shares");

    expect(links("Shared objects").every(isActive)).toBe(true);
    expect(screen.getByText(/everything shared inside/i)).toBeInTheDocument();
  });
});
