import type { ReactNode } from "react";
import { render, screen } from "@testing-library/react";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, test, vi } from "vitest";
import { AuthContext } from "@/auth/useAuth";
import type { Me } from "@/api/hooks";
import {
  RequireAuth,
  RequireControlPlaneAccess,
  RequireGlobalAdmin,
  RequireGovernanceAccess,
  RequirePermission,
  RequirePlatform,
} from "@/auth/guards";

afterEach(() => vi.restoreAllMocks());
const qc = () => new QueryClient({ defaultOptions: { queries: { retry: false } } });

test("RequireAuth redirects to /login when unauthenticated", () => {
  render(
    <QueryClientProvider client={qc()}>
      <AuthContext.Provider value={{ me: null, isLoading: false }}>
        <MemoryRouter initialEntries={["/secret"]}>
          <Routes>
            <Route path="/secret" element={<RequireAuth><span>secret</span></RequireAuth>} />
            <Route path="/login" element={<span>login page</span>} />
          </Routes>
        </MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
  expect(screen.getByText("login page")).toBeInTheDocument();
});

test("RequirePlatform blocks a non-platform user", () => {
  render(
    <QueryClientProvider client={qc()}>
      <AuthContext.Provider value={{ me: { user_id: "1", email: "a@b.c", platform_roles: [], permissions: [] }, isLoading: false }}>
        <MemoryRouter><RequirePlatform><span>admin area</span></RequirePlatform></MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
  expect(screen.queryByText("admin area")).not.toBeInTheDocument();
  expect(screen.getByText(/Forbidden/i)).toBeInTheDocument();
});

test("RequireGlobalAdmin blocks a platform role that is not platform_admin", () => {
  render(
    <QueryClientProvider client={qc()}>
      <AuthContext.Provider value={{ me: { user_id: "1", email: "a@b.c", platform_roles: ["data_steward"], permissions: [] }, isLoading: false }}>
        <MemoryRouter><RequireGlobalAdmin><span>control plane</span></RequireGlobalAdmin></MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
  expect(screen.queryByText("control plane")).not.toBeInTheDocument();
  expect(screen.getByText(/Forbidden/i)).toBeInTheDocument();
});

test("RequireGlobalAdmin allows platform_admin", () => {
  render(
    <QueryClientProvider client={qc()}>
      <AuthContext.Provider value={{ me: { user_id: "1", email: "a@b.c", platform_roles: ["platform_admin"], permissions: [] }, isLoading: false }}>
        <MemoryRouter><RequireGlobalAdmin><span>control plane</span></RequireGlobalAdmin></MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
  expect(screen.getByText("control plane")).toBeInTheDocument();
});

test("RequirePermission allows a user holding the required permission", () => {
  render(
    <QueryClientProvider client={qc()}>
      <AuthContext.Provider value={{ me: { user_id: "1", email: "a@b.c", platform_roles: [], permissions: ["policy:read"] }, isLoading: false }}>
        <MemoryRouter><RequirePermission perm="policy:read"><span>policies</span></RequirePermission></MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
  expect(screen.getByText("policies")).toBeInTheDocument();
});

test("RequirePermission allows platform_admin regardless of permissions", () => {
  render(
    <QueryClientProvider client={qc()}>
      <AuthContext.Provider value={{ me: { user_id: "1", email: "a@b.c", platform_roles: ["platform_admin"], permissions: [] }, isLoading: false }}>
        <MemoryRouter><RequirePermission perm="policy:read"><span>policies</span></RequirePermission></MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
  expect(screen.getByText("policies")).toBeInTheDocument();
});

test("RequirePermission denies a user missing the required permission", () => {
  render(
    <QueryClientProvider client={qc()}>
      <AuthContext.Provider value={{ me: { user_id: "1", email: "a@b.c", platform_roles: [], permissions: ["engine:read"] }, isLoading: false }}>
        <MemoryRouter><RequirePermission perm="policy:read"><span>policies</span></RequirePermission></MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
  expect(screen.queryByText("policies")).not.toBeInTheDocument();
  expect(screen.getByText(/Forbidden/i)).toBeInTheDocument();
});

test("RequirePermission names every permission it accepts, and links home", () => {
  render(
    <QueryClientProvider client={qc()}>
      <AuthContext.Provider value={{ me: { user_id: "1", email: "a@b.c", platform_roles: [], permissions: ["engine:read"] }, isLoading: false }}>
        <MemoryRouter><RequirePermission perm={["policy:read", "tag:read"]}><span>policies</span></RequirePermission></MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
  expect(screen.getByRole("heading", { level: 1, name: "You don't have access to this page." })).toBeInTheDocument();
  expect(screen.getByText("This page needs the policy:read or tag:read permission.")).toBeInTheDocument();
  expect(screen.getByRole("link", { name: "Go to the home page" })).toHaveAttribute("href", "/");
});

test("RequireControlPlaneAccess allows a user holding any control-plane permission", () => {
  render(
    <QueryClientProvider client={qc()}>
      <AuthContext.Provider value={{ me: { user_id: "1", email: "a@b.c", platform_roles: [], permissions: ["engine:read"] }, isLoading: false }}>
        <MemoryRouter><RequireControlPlaneAccess><span>control plane</span></RequireControlPlaneAccess></MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
  expect(screen.getByText("control plane")).toBeInTheDocument();
});

test("RequireControlPlaneAccess denies a user with no control-plane permissions", () => {
  render(
    <QueryClientProvider client={qc()}>
      <AuthContext.Provider value={{ me: { user_id: "1", email: "a@b.c", platform_roles: [], permissions: [] }, isLoading: false }}>
        <MemoryRouter><RequireControlPlaneAccess><span>control plane</span></RequireControlPlaneAccess></MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
  expect(screen.queryByText("control plane")).not.toBeInTheDocument();
  expect(screen.getByText(/Forbidden/i)).toBeInTheDocument();
});

const withPerms = (permissions: string[], platform_roles: string[] = []): Me => ({
  user_id: "1",
  email: "a@b.c",
  platform_roles,
  permissions,
});

/** Mount a guard under `/control-plane/*` and `/governance/*`, like App does. */
function renderArea(path: string, me: Me | null, guarded: ReactNode) {
  return render(
    <QueryClientProvider client={qc()}>
      <AuthContext.Provider value={{ me, isLoading: false }}>
        <MemoryRouter initialEntries={[path]}>
          <Routes>
            <Route path="/control-plane/*" element={guarded} />
            <Route path="/governance/*" element={guarded} />
            <Route path="/governance" element={<span>governance landing</span>} />
          </Routes>
        </MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
}

test("RequireControlPlaneAccess no longer admits a policy-only user to a Control Plane page", () => {
  renderArea(
    "/control-plane/engines",
    withPerms(["policy:read"]),
    <RequireControlPlaneAccess><span>control plane</span></RequireControlPlaneAccess>,
  );
  expect(screen.queryByText("control plane")).not.toBeInTheDocument();
  expect(screen.getByText(/Forbidden/i)).toBeInTheDocument();
});

test.each(["/control-plane", "/control-plane/"])(
  "RequireControlPlaneAccess sends a governance-only user on %s to /governance",
  (path) => {
    // Rendered at `/governance` by the route above only if the guard redirected.
    render(
      <QueryClientProvider client={qc()}>
        <AuthContext.Provider value={{ me: withPerms(["tag:read"]), isLoading: false }}>
          <MemoryRouter initialEntries={[path]}>
            <Routes>
              <Route
                path="/control-plane/*"
                element={<RequireControlPlaneAccess><span>control plane</span></RequireControlPlaneAccess>}
              />
              <Route path="/governance" element={<span>governance landing</span>} />
            </Routes>
          </MemoryRouter>
        </AuthContext.Provider>
      </QueryClientProvider>,
    );
    expect(screen.getByText("governance landing")).toBeInTheDocument();
    expect(screen.queryByText(/Forbidden/i)).not.toBeInTheDocument();
  },
);

test("RequireControlPlaneAccess 403s a governance-only user on a Control Plane deep link", () => {
  renderArea(
    "/control-plane/audit-log",
    withPerms(["policy:read", "tag:read"]),
    <RequireControlPlaneAccess><span>control plane</span></RequireControlPlaneAccess>,
  );
  expect(screen.getByText(/Forbidden/i)).toBeInTheDocument();
  expect(screen.queryByText("governance landing")).not.toBeInTheDocument();
});

test("RequireControlPlaneAccess 403s a user with neither app on the landing", () => {
  renderArea(
    "/control-plane",
    withPerms([]),
    <RequireControlPlaneAccess><span>control plane</span></RequireControlPlaneAccess>,
  );
  expect(screen.getByText(/Forbidden/i)).toBeInTheDocument();
});

test.each([
  ["tag:read only", withPerms(["tag:read"])],
  ["policy:read only", withPerms(["policy:read"])],
  ["platform_admin", withPerms([], ["platform_admin"])],
])("RequireGovernanceAccess admits %s", (_label, me) => {
  renderArea("/governance/policies", me, <RequireGovernanceAccess><span>governance</span></RequireGovernanceAccess>);
  expect(screen.getByText("governance")).toBeInTheDocument();
});

test.each([
  ["an infra-only user", withPerms(["engine:read"])],
  ["no session", null],
])("RequireGovernanceAccess denies %s", (_label, me) => {
  renderArea("/governance/policies", me, <RequireGovernanceAccess><span>governance</span></RequireGovernanceAccess>);
  expect(screen.queryByText("governance")).not.toBeInTheDocument();
  expect(screen.getByText(/Forbidden/i)).toBeInTheDocument();
});
