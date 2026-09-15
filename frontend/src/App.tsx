import { QueryClientProvider } from "@tanstack/react-query";
import { BrowserRouter, Navigate, Route, Routes, useParams } from "react-router-dom";
import { ThemeProvider } from "@/components/theme-provider";
import { Toaster } from "@/components/ui/sonner";
import { AuthProvider } from "@/auth/AuthProvider";
import {
  RequireAuth,
  RequireControlPlaneAccess,
  RequireGlobalAdmin,
  RequireGovernanceAccess,
  RequirePermission,
  RequireWorkspace,
} from "@/auth/guards";
import { AppShell } from "@/components/layout/AppShell";
import { LoginPage } from "@/auth/LoginPage";
import { NotFound } from "@/routes/NotFound";
import { useAuth } from "@/auth/useAuth";
import { canEnterControlPlane, canEnterGovernance } from "@/auth/appAccess";
import { useMyWorkspaces } from "@/api/hooks";
import { queryClient } from "@/api/queryClient";
import { RequireFeature } from "@/components/explore/RequireFeature";
import { exploreRedirectTo, useDefaultExplorePath } from "@/hooks/useWorkspaceFeatures";
import PrintObject from "@/routes/print/PrintObject";

function LoginRoute() {
  const { me, isLoading } = useAuth();
  if (isLoading) return <div className="p-10">Loading…</div>;
  if (me) return <Navigate to="/" replace />;
  return <LoginPage />;
}
import SqlLab from "@/routes/explore/SqlLab";
import Datasets from "@/routes/explore/Datasets";
import DatasetDetail from "@/routes/explore/DatasetDetail";
import Charts from "@/routes/explore/Charts";
import ChartExplore from "@/routes/explore/ChartExplore";
import Dashboards from "@/routes/explore/Dashboards";
import Alerts from "@/routes/explore/Alerts";
import ScheduleDetail from "@/routes/explore/ScheduleDetail";
import Notifications from "@/routes/explore/settings/Notifications";
import DashboardView from "@/routes/explore/DashboardView";
import Notebooks from "@/routes/explore/Notebooks";
import NotebookPage from "@/routes/explore/NotebookPage";
import NotebookSettings from "@/routes/explore/NotebookSettings";
import DatabasesPage from "@/routes/explore/DatabasesPage";
import WorkspaceSettings from "@/routes/explore/settings/WorkspaceSettings";
import Members from "@/routes/explore/settings/Members";
import RolesPermissions from "@/routes/explore/settings/RolesPermissions";
import SharedObjects from "@/routes/explore/settings/SharedObjects";
import AccessRequests from "@/routes/explore/settings/AccessRequests";
import WorkspaceGeneral from "@/routes/explore/settings/WorkspaceGeneral";
import Endpoints from "@/routes/explore/dataapi/Endpoints";
import EndpointCreate from "@/routes/explore/dataapi/EndpointCreate";
import Clients from "@/routes/explore/dataapi/Clients";
import ServiceAccountCreate from "@/routes/explore/dataapi/ServiceAccountCreate";
import ClientCreate from "@/routes/explore/dataapi/ClientCreate";
import Activity from "@/routes/explore/dataapi/Activity";
import EndpointDetail from "@/routes/explore/dataapi/EndpointDetail";
import Query from "@/routes/explore/dataapi/detail/Query";
import Config from "@/routes/explore/dataapi/detail/Config";
import ResponseSection from "@/routes/explore/dataapi/detail/ResponseSection";
import AccessSection from "@/routes/explore/dataapi/detail/AccessSection";
import VersionsSection from "@/routes/explore/dataapi/detail/Versions";
import TryIt from "@/routes/explore/dataapi/detail/TryIt";
import { Placeholder } from "@/routes/Placeholder";
import Workspaces from "@/routes/control-plane/Workspaces";
import WorkspaceDetail from "@/routes/control-plane/WorkspaceDetail";
import ControlPlaneEngines from "@/routes/control-plane/ControlPlaneEngines";
import EngineDetail from "@/routes/control-plane/EngineDetail";
import EngineCreate from "@/routes/control-plane/EngineCreate";
import ControlPlaneStorage from "@/routes/control-plane/ControlPlaneStorage";
import StorageDetail from "@/routes/control-plane/StorageDetail";
import StorageBrowser from "@/routes/control-plane/StorageBrowser";
import ControlPlaneSecrets from "@/routes/control-plane/ControlPlaneSecrets";
import SecretDetail from "@/routes/control-plane/SecretDetail";
import Policies from "@/routes/governance/Policies";
import ProfileDetailPage from "@/routes/governance/ProfileDetailPage";
import PolicyEditorPage from "@/routes/governance/PolicyEditorPage";
import Warehouses from "@/routes/control-plane/Warehouses";
import WarehouseDetail from "@/routes/control-plane/WarehouseDetail";
import AuditLog from "@/routes/control-plane/AuditLog";
import UsersPage from "@/routes/control-plane/access/UsersPage";
import UserDetail from "@/routes/control-plane/access/UserDetail";
import GroupsPage from "@/routes/control-plane/access/GroupsPage";
import GroupDetail from "@/routes/control-plane/access/GroupDetail";
import RolesPage from "@/routes/control-plane/access/RolesPage";
import RoleDetail from "@/routes/control-plane/access/RoleDetail";
import PermissionsPage from "@/routes/control-plane/access/PermissionsPage";
import AccessExplorer from "@/routes/control-plane/access/AccessExplorer";
import AccessOverview from "@/routes/control-plane/access/AccessOverview";
import WhoCanAccess from "@/routes/control-plane/access/WhoCanAccess";
import Classifications from "@/routes/governance/Classifications";
import TagDetail from "@/routes/governance/TagDetail";
import { LegacyGovernanceRedirect } from "@/routes/governance/LegacyGovernanceRedirect";

function HomeRedirect() {
  const { me, isLoading } = useAuth();
  const { data, isLoading: wsLoading } = useMyWorkspaces(!!me);
  if (isLoading) return <div className="p-10">Loading…</div>;
  // Wait for the workspace list to settle (loaded or errored) before deciding
  // where a workspace-less user goes — otherwise a member with workspaces
  // would flash a Governance/Control Plane redirect while the fetch is
  // still in flight.
  if (wsLoading) return <div className="p-10">Loading…</div>;
  if (data && data.length > 0) return <Navigate to={`/explore/${data[0].slug}`} replace />;
  // No workspace yet. A steward or infra operator still has somewhere to go —
  // this page renders no header, so without the redirect they would be stranded.
  if (canEnterGovernance(me)) return <Navigate to="/governance" replace />;
  if (canEnterControlPlane(me)) return <Navigate to="/control-plane" replace />;
  return <div className="p-10">No workspaces yet. Ask an admin to add you to one.</div>;
}

function ExploreRoutes() {
  // Picks the first explore module (in nav order) this workspace has switched
  // on, falling back to Workspace settings when every module is off — so the
  // index/catch-all redirects never land a user on a route `RequireFeature`
  // will immediately bounce off. `ExploreRoutes` is a component, so calling
  // the hook here (unconditionally, on every render) is legal.
  const defaultPath = useDefaultExplorePath();
  // Absolute, not relative — see `exploreRedirectTo`. A relative target here sent the
  // catch-all into an endless redirect under react-router 7.
  const { workspace } = useParams();
  const redirectTo = exploreRedirectTo(workspace, defaultPath);
  return (
    <RequireWorkspace>
      <AppShell>
        <Routes>
          <Route index element={<Navigate to={redirectTo} replace />} />
          <Route path="sql-lab" element={<RequireFeature feature="sql_lab"><SqlLab /></RequireFeature>} />
          <Route path="datasets" element={<RequireFeature feature="datasets"><Datasets /></RequireFeature>} />
          <Route path="datasets/:id" element={<RequireFeature feature="datasets"><DatasetDetail /></RequireFeature>} />
          <Route path="charts" element={<RequireFeature feature="charts"><Charts /></RequireFeature>} />
          <Route path="charts/new" element={<RequireFeature feature="charts"><ChartExplore /></RequireFeature>} />
          <Route path="charts/:id" element={<RequireFeature feature="charts"><ChartExplore /></RequireFeature>} />
          <Route path="dashboards" element={<RequireFeature feature="dashboards"><Dashboards /></RequireFeature>} />
          <Route path="dashboards/:id" element={<RequireFeature feature="dashboards"><DashboardView /></RequireFeature>} />
          <Route path="notebooks" element={<RequireFeature feature="notebooks"><Notebooks /></RequireFeature>} />
          <Route path="notebooks/:id" element={<RequireFeature feature="notebooks"><NotebookPage /></RequireFeature>} />
          <Route path="notebook-settings" element={<RequireFeature feature="notebooks"><NotebookSettings /></RequireFeature>} />
          {/* Engine connections now live under Workspace settings; keep the old
              path working by redirecting into that tab. */}
          <Route path="engines" element={<Navigate to="../settings/engines" replace />} />
          {/* Data API is its own sidebar area (Sidebar.tsx), not a shell page: three
              sibling sections plus a detail route, each mounted flat here exactly
              like every other Explore feature (Charts, Datasets…). There used to be
              a `DataApiLayout` shell carrying the page's `h1` and a section nav that
              duplicated the sidebar; it was removed once the sidebar itself carried
              Endpoints/Clients/Activity as real items — see
              docs/en/specs/2026-08-16-data-api-p5-design.md §2.1–§2.2. `/data-api`
              itself keeps a bare redirect so an old bookmark to the shell's own URL
              still lands somewhere real. */}
          <Route path="data-api" element={<Navigate to="endpoints" replace />} />
          <Route path="data-api/endpoints" element={<RequireFeature feature="data_api"><Endpoints /></RequireFeature>} />
          {/* Creating an endpoint is a PAGE, not a dialog: same five fields,
              but one of them has a URL, so it can be linked to, bookmarked and
              reached with Back. (It carries no draft persistence — the form is
              plain component state, and a reload empties it.)

              It is a dedicated component (`EndpointCreate`) rather than
              `EndpointDetail` rendered without an id, which is what
              `charts/new` and `alerts/new` do: six of the detail route's seven
              sections read that id (Preview POSTs to
              `…/endpoints/{id}/preview`; Response, Access, Try it, Versions and
              Config all load the saved row), so reusing that shell here would
              ship a page whose Preview cannot run and whose right-hand tabs are
              meaningless until the first save. The precedent that fits is
              `control-plane/engines/new` → `EngineCreate`.

              `new` is written before `:id` as a CONVENTION, for readers — NOT
              as a fix. Measured on the installed react-router-dom (7.18.2):
              with `:id` registered FIRST, `/endpoints/new` still matches the
              static route, because v7 ranks segments by specificity rather than
              by declaration order. */}
          <Route path="data-api/endpoints/new" element={<RequireFeature feature="data_api"><EndpointCreate /></RequireFeature>} />
          <Route path="data-api/endpoints/:id" element={<RequireFeature feature="data_api"><EndpointDetail /></RequireFeature>}>
            {/* Query (SQL, database, params, preview) is the landing section —
                everything that decides what the query RETURNS. */}
            <Route index element={<Query />} />
            {/* Each section is its own child route (not a query param) so
                the open section is in the URL and survives a refresh.
                `Access` links into `try` with `?service_account=`; `TryIt`
                reads that exact key to preselect the account a row is
                about. Nothing fails if it doesn't — the link still
                navigates, just without the preselection — which is why the
                contract is written down here and in the plan ledger.
                `Config` holds everything about the endpoint as an HTTP
                object — name/path/method and cache/limits/rate-limit. */}
            <Route path="config" element={<Config />} />
            <Route path="response" element={<ResponseSection />} />
            <Route path="access" element={<AccessSection />} />
            <Route path="try" element={<TryIt />} />
            <Route path="versions" element={<VersionsSection />} />
          </Route>
          <Route path="data-api/clients" element={<RequireFeature feature="data_api"><Clients /></RequireFeature>} />
          {/* Creating a service account is a PAGE too, for the same reasons as
              `endpoints/new` above: a URL that can be linked, bookmarked and
              opened in a tab. (It carries no draft persistence either — the
              form is plain component state, and a reload empties it.)

              Unlike `endpoints/new` it navigates BACK to its list on success
              rather than into the created object, because a service account
              has no detail route to open: it is a row in the Clients table
              with an actions menu, so that table is its detail view.

              Written before any `:id` sibling as a CONVENTION, for readers —
              NOT as a fix. There is no `clients/:id` today, and if one is
              added, v7 ranks segments by specificity rather than declaration
              order, so this literal wins wherever it sits. Measured on the
              installed react-router-dom (7.18.2); see `endpoints/new`. */}
          <Route path="data-api/clients/new" element={<RequireFeature feature="data_api"><ServiceAccountCreate /></RequireFeature>} />
          {/* Issuing a client credential is a PAGE for a reason the other two
              create routes do not have: the response contains a secret shown
              ONCE, and as dialogs that was a modal opening on top of another
              modal (the issue dialog was unmounted while still open as the
              secret dialog mounted — the classic stuck-`pointer-events` shape).
              The secret now lands on this same route in a created state, which
              cannot be dismissed by an outside click or an accidental Escape.

              `:accountId` is in the URL because the credential belongs to one
              service account; the page names that account by reading the
              service-account list, since an id is not something a reader
              recognises. */}
          <Route path="data-api/clients/:accountId/new-client" element={<RequireFeature feature="data_api"><ClientCreate /></RequireFeature>} />
          <Route path="data-api/activity" element={<RequireFeature feature="data_api"><Activity /></RequireFeature>} />
          <Route
            path="alerts"
            element={
              <RequireFeature feature="alerts">
                <Alerts />
              </RequireFeature>
            }
          />
          {/* `new` is written before `:id` as a CONVENTION, for readers — not
              as a fix. Measured on react-router-dom 7.18.2: with `:id`
              registered FIRST, `/alerts/new` still matches the static route,
              because v7 ranks by segment specificity, not declaration order.
              This comment used to claim the literal would be captured as an id;
              it was wrong. See `data-api/endpoints/new` above. */}
          <Route
            path="alerts/new"
            element={
              <RequireFeature feature="alerts">
                <ScheduleDetail />
              </RequireFeature>
            }
          />
          <Route
            path="alerts/:id"
            element={
              <RequireFeature feature="alerts">
                <ScheduleDetail />
              </RequireFeature>
            }
          />
          <Route path="settings" element={<WorkspaceSettings />}>
            <Route index element={<Navigate to="members" replace />} />
            <Route path="members" element={<Members />} />
            <Route path="roles" element={<RolesPermissions />} />
            <Route path="shares" element={<SharedObjects />} />
            <Route path="requests" element={<AccessRequests />} />
            <Route path="engines" element={<DatabasesPage />} />
            <Route path="general" element={<WorkspaceGeneral />} />
            <Route path="notifications" element={<Notifications />} />
          </Route>
          <Route path="*" element={<Navigate to={redirectTo} replace />} />
        </Routes>
      </AppShell>
    </RequireWorkspace>
  );
}

// First section (in preference order) each control-plane read permission
// unlocks. Used to redirect a scoped (non platform_admin) user landing on
// `/control-plane` to a section they can actually see, instead of the
// platform_admin-only "workspaces" default.
const FIRST_SECTION_BY_PERM: { path: string; perm: string }[] = [
  { path: "engines", perm: "engine:read" },
  { path: "warehouses", perm: "warehouse:read" },
  { path: "storage", perm: "storage:read" },
  { path: "secrets", perm: "secret:read" },
  { path: "users", perm: "user:read" },
  { path: "groups", perm: "group:read" },
  { path: "roles", perm: "role:read" },
  { path: "permissions", perm: "role:read" },
];

function ControlPlaneIndex() {
  const { me } = useAuth();
  if (me?.platform_roles.includes("platform_admin")) return <Navigate to="workspaces" replace />;
  const perms = me?.permissions ?? [];
  const first = FIRST_SECTION_BY_PERM.find((s) => perms.includes(s.perm));
  if (first) return <Navigate to={first.path} replace />;
  // `RequireControlPlaneAccess` admits anyone holding a Control Plane read
  // permission (see `CONTROL_PLANE_PERMS`), but not every one of those unlocks
  // a section above (e.g. `workspace:read` alone) — so a non-admin can land
  // here matching nothing and 403 on the platform_admin-only "workspaces"
  // fallback. If they can also enter Governance, send them there instead.
  if (canEnterGovernance(me)) return <Navigate to="/governance" replace />;
  return <Navigate to="workspaces" replace />;
}

function ControlPlaneRoutes() {
  return (
    <RequireControlPlaneAccess>
      <AppShell>
        <Routes>
          <Route index element={<ControlPlaneIndex />} />
          <Route path="workspaces" element={<RequireGlobalAdmin><Workspaces /></RequireGlobalAdmin>} />
          <Route path="workspaces/:id" element={<RequireGlobalAdmin><WorkspaceDetail /></RequireGlobalAdmin>} />
          <Route path="engines" element={<RequirePermission perm="engine:read"><ControlPlaneEngines /></RequirePermission>} />
          <Route path="engines/new" element={<RequirePermission perm="engine:create"><EngineCreate /></RequirePermission>} />
          <Route path="engines/:id" element={<RequirePermission perm="engine:read"><EngineDetail /></RequirePermission>} />
          <Route path="warehouses" element={<RequirePermission perm="warehouse:read"><Warehouses /></RequirePermission>} />
          <Route path="warehouses/:id" element={<RequirePermission perm="warehouse:read"><WarehouseDetail /></RequirePermission>} />
          <Route path="storage" element={<RequirePermission perm="storage:read"><ControlPlaneStorage /></RequirePermission>} />
          <Route path="storage/:id" element={<RequirePermission perm="storage:read"><StorageDetail /></RequirePermission>} />
          <Route path="storage/:id/browse" element={<RequirePermission perm="storage:read"><StorageBrowser /></RequirePermission>} />
          <Route path="secrets" element={<RequirePermission perm="secret:read"><ControlPlaneSecrets /></RequirePermission>} />
          <Route path="secrets/:id" element={<RequirePermission perm="secret:read"><SecretDetail /></RequirePermission>} />
          <Route path="users" element={<RequirePermission perm="user:read"><UsersPage /></RequirePermission>} />
          <Route path="users/:id" element={<RequirePermission perm="user:read"><UserDetail /></RequirePermission>} />
          <Route path="groups" element={<RequirePermission perm="group:read"><GroupsPage /></RequirePermission>} />
          <Route path="groups/:id" element={<RequirePermission perm="group:read"><GroupDetail /></RequirePermission>} />
          <Route path="roles" element={<RequirePermission perm="role:read"><RolesPage /></RequirePermission>} />
          <Route path="roles/:id" element={<RequirePermission perm="role:read"><RoleDetail /></RequirePermission>} />
          <Route path="permissions" element={<RequirePermission perm="role:read"><PermissionsPage /></RequirePermission>} />
          <Route path="access-overview" element={<RequirePermission perm="user:read"><AccessOverview /></RequirePermission>} />
          <Route path="access-explorer" element={<RequirePermission perm="role:read"><AccessExplorer /></RequirePermission>} />
          <Route path="who-can-access" element={<RequirePermission perm="role:read"><WhoCanAccess /></RequirePermission>} />
          <Route path="audit-log" element={<RequireGlobalAdmin><AuditLog /></RequireGlobalAdmin>} />
          <Route path="*" element={<ControlPlaneIndex />} />
        </Routes>
      </AppShell>
    </RequireControlPlaneAccess>
  );
}

// Governance (SP2): the data-plane rules a data steward manages, split out of
// the system-admin Control Plane. Same shape as `ControlPlaneRoutes` — one area
// gate, then a per-route permission. First section (in preference order) each
// Governance read permission unlocks, for the landing below.
const GOVERNANCE_FIRST_SECTION: { path: string; perm: string }[] = [
  { path: "/governance/policies", perm: "policy:read" },
  { path: "/governance/classifications", perm: "tag:read" },
];

function GovernanceIndex() {
  const { me } = useAuth();
  if (me?.platform_roles.includes("platform_admin")) return <Navigate to="/governance/policies" replace />;
  const perms = me?.permissions ?? [];
  const first = GOVERNANCE_FIRST_SECTION.find((s) => perms.includes(s.perm));
  // Absolute targets: this also renders for the `*` catch-all, where react-router 7
  // resolves a relative target against the unmatched URL and loops (see
  // `exploreRedirectTo`). `RequireGovernanceAccess` already admitted this user,
  // so `first` exists; the fallback only satisfies the type.
  return <Navigate to={first ? first.path : "/governance/policies"} replace />;
}

function GovernanceRoutes() {
  return (
    <RequireGovernanceAccess>
      <AppShell>
        <Routes>
          <Route index element={<GovernanceIndex />} />
          <Route path="policies" element={<RequirePermission perm="policy:read"><Policies /></RequirePermission>} />
          <Route path="policies/:service" element={<RequirePermission perm="policy:read"><ProfileDetailPage /></RequirePermission>} />
          <Route path="policies/:service/:kind/new" element={<RequirePermission perm="policy:read"><PolicyEditorPage /></RequirePermission>} />
          <Route path="policies/:service/:kind/:id" element={<RequirePermission perm="policy:read"><PolicyEditorPage /></RequirePermission>} />
          <Route path="classifications" element={<RequirePermission perm="tag:read"><Classifications /></RequirePermission>} />
          <Route path="classifications/:id" element={<RequirePermission perm="tag:read"><TagDetail /></RequirePermission>} />
          {/* Platform-wide (cross-workspace) placeholder; admin-only until the real feature and its permission land. */}
          <Route path="data-lineage" element={<RequireGlobalAdmin><Placeholder title="Data Lineage" /></RequireGlobalAdmin>} />
          <Route path="*" element={<GovernanceIndex />} />
        </Routes>
      </AppShell>
    </RequireGovernanceAccess>
  );
}

export default function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <ThemeProvider defaultTheme="system" storageKey="egret-theme">
        <BrowserRouter>
          <AuthProvider>
            <Routes>
              <Route path="/login" element={<LoginRoute />} />
              {/* The headless renderer's browser has no session — that is the entire point of the
                  single-use `?t=` token this route reads for itself. It must stay OUTSIDE
                  `RequireAuth`/`RequireWorkspace`: either guard would redirect an unauthenticated
                  renderer to `/login` and it would screenshot that instead of the report. */}
              <Route path="/embed/print/:kind/:id" element={<PrintObject />} />
              <Route path="/" element={<RequireAuth><HomeRedirect /></RequireAuth>} />
              <Route path="/explore/:workspace/*" element={<RequireAuth><ExploreRoutes /></RequireAuth>} />
              <Route path="/governance/*" element={<RequireAuth><GovernanceRoutes /></RequireAuth>} />
              <Route path="/control-plane/*" element={<RequireAuth><ControlPlaneRoutes /></RequireAuth>} />
              {/* Old Control Plane URLs of the pages that moved to Governance. Mounted
                  here, not inside ControlPlaneRoutes: that tree is behind
                  RequireControlPlaneAccess, which a governance-only user fails, so
                  they would get a 403 for their own bookmark. More static segments
                  outrank `/control-plane/*` in react-router 7, so order does not
                  matter. No RequireAuth: the destination carries it. */}
              <Route path="/control-plane/policies/*" element={<LegacyGovernanceRedirect />} />
              <Route path="/control-plane/classifications/*" element={<LegacyGovernanceRedirect />} />
              <Route path="/control-plane/data-lineage" element={<LegacyGovernanceRedirect />} />
              <Route path="*" element={<NotFound />} />
            </Routes>
          </AuthProvider>
        </BrowserRouter>
        {/* The single global toast host. Every `toast()` call across the app
            (32 call sites) is a no-op until this is mounted — it was defined in
            components/ui/sonner.tsx but never rendered. Sits inside ThemeProvider
            so it picks up light/dark; one instance, app-wide. */}
        <Toaster />
      </ThemeProvider>
    </QueryClientProvider>
  );
}
