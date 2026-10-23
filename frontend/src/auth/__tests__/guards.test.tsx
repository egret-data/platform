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
  RequireWorkspace,
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

// KI-230: an area refusal used to replace the whole page with a bare card: no
// header, no sidebar, no way back but the browser's. It now renders inside the
// app shell, says why, and offers Back and a workspace the reader can open.
const serveMyWorkspaces = () =>
  vi.spyOn(globalThis, "fetch").mockImplementation(() =>
    Promise.resolve(
      new Response(JSON.stringify([{ id: "1", slug: "eng", name: "Engineering", features: [], permissions: [] }]), {
        status: 200,
        headers: { "content-type": "application/json" },
      }),
    ),
  );

test.each([
  ["/control-plane/users", "Control Plane needs a platform administrator or a Control Plane permission such as user:read."],
  ["/governance/data", "Governance needs a governance permission such as tag:read or policy:read."],
])("an area refusal at %s renders inside the shell, with a reason and a way back", async (path, reason) => {
  serveMyWorkspaces();
  const guarded = path.startsWith("/governance") ? (
    <RequireGovernanceAccess><span>area</span></RequireGovernanceAccess>
  ) : (
    <RequireControlPlaneAccess><span>area</span></RequireControlPlaneAccess>
  );
  render(
    <QueryClientProvider client={qc()}>
      <AuthContext.Provider value={{ me: withPerms([]), isLoading: false }}>
        <MemoryRouter initialEntries={["/explore/eng/datasets", path]} initialIndex={1}>
          <Routes>
            <Route path="/control-plane/*" element={guarded} />
            <Route path="/governance/*" element={guarded} />
          </Routes>
        </MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
  expect(screen.queryByText("area")).not.toBeInTheDocument();
  expect(screen.getByTestId("app-header")).toBeInTheDocument();
  expect(screen.getByRole("heading", { level: 1, name: "You don't have access to this page." })).toBeInTheDocument();
  expect(screen.getByText(reason)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Go back" })).toBeInTheDocument();
  expect(await screen.findByRole("link", { name: "Open Engineering" })).toHaveAttribute("href", "/explore/eng");
});

test("RequireWorkspace refuses a workspace the reader is not in, inside the shell, with no sidebar", async () => {
  serveMyWorkspaces();
  render(
    <QueryClientProvider client={qc()}>
      <AuthContext.Provider value={{ me: withPerms([]), isLoading: false }}>
        <MemoryRouter initialEntries={["/explore/other/datasets"]}>
          <Routes>
            <Route path="/explore/:workspace/*" element={<RequireWorkspace><span>workspace page</span></RequireWorkspace>} />
          </Routes>
        </MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
  expect(
    await screen.findByRole("heading", { level: 1, name: "You don't have access to this page." }),
  ).toBeInTheDocument();
  expect(screen.queryByText("workspace page")).not.toBeInTheDocument();
  expect(screen.getByTestId("app-header")).toBeInTheDocument();
  expect(screen.queryByTestId("app-sidebar")).not.toBeInTheDocument();
  expect(screen.getByText(/you are not a member of it/)).toBeInTheDocument();
  expect(screen.getByRole("link", { name: "Open Engineering" })).toHaveAttribute("href", "/explore/eng");
  // The first page of the tab: there is nothing to go back to.
  expect(screen.queryByRole("button", { name: "Go back" })).not.toBeInTheDocument();
});
