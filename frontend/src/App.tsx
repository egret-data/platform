import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { BrowserRouter, Navigate, Route, Routes } from "react-router-dom";
import { ThemeProvider } from "@/components/theme-provider";
import { AuthProvider } from "@/auth/AuthProvider";
import { RequireAuth, RequireControlPlaneAccess, RequireGlobalAdmin, RequirePermission, RequireWorkspace } from "@/auth/guards";
import { AppShell } from "@/components/layout/AppShell";
import { LoginPage } from "@/auth/LoginPage";
import { NotFound } from "@/routes/NotFound";
import { useAuth } from "@/auth/useAuth";
import { useMyWorkspaces } from "@/api/hooks";

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
import { Placeholder } from "@/routes/Placeholder";
import ControlPlaneEngines from "@/routes/control-plane/ControlPlaneEngines";
import EngineDetail from "@/routes/control-plane/EngineDetail";
import ControlPlaneStorage from "@/routes/control-plane/ControlPlaneStorage";
import StorageBrowser from "@/routes/control-plane/StorageBrowser";
import ControlPlaneSecrets from "@/routes/control-plane/ControlPlaneSecrets";
import Policies from "@/routes/control-plane/Policies";
import ProfileDetailPage from "@/routes/control-plane/ProfileDetailPage";
import PolicyEditorPage from "@/routes/control-plane/PolicyEditorPage";
import Warehouses from "@/routes/control-plane/Warehouses";
import WarehouseDetail from "@/routes/control-plane/WarehouseDetail";
import AuditLog from "@/routes/control-plane/AuditLog";
import UsersPage from "@/routes/control-plane/access/UsersPage";
import GroupsPage from "@/routes/control-plane/access/GroupsPage";
import RolesPage from "@/routes/control-plane/access/RolesPage";
import PermissionsPage from "@/routes/control-plane/access/PermissionsPage";
import AccessExplorer from "@/routes/control-plane/access/AccessExplorer";
import AccessOverview from "@/routes/control-plane/access/AccessOverview";
import WhoCanAccess from "@/routes/control-plane/access/WhoCanAccess";
import Classifications from "@/routes/control-plane/governance/Classifications";

const qc = new QueryClient({ defaultOptions: { queries: { refetchOnWindowFocus: false, retry: 1, staleTime: 30000 } } });

function HomeRedirect() {
  const { me, isLoading } = useAuth();
  const { data } = useMyWorkspaces(!!me);
  if (isLoading) return <div className="p-10">Loading…</div>;
  if (data && data.length > 0) return <Navigate to={`/explore/${data[0].slug}`} replace />;
  return <div className="p-10">No workspaces yet. Ask an admin to add you to one.</div>;
}

function ExploreRoutes() {
  return (
    <RequireWorkspace>
      <AppShell>
        <Routes>
          <Route index element={<Navigate to="sql-lab" replace />} />
          <Route path="sql-lab" element={<SqlLab />} />
          <Route path="datasets" element={<Datasets />} />
          <Route path="datasets/:id" element={<DatasetDetail />} />
          <Route path="charts" element={<Charts />} />
          <Route path="charts/new" element={<ChartExplore />} />
          <Route path="charts/:id" element={<ChartExplore />} />
          <Route path="dashboards" element={<Dashboards />} />
          <Route path="dashboards/:id" element={<DashboardView />} />
          <Route path="notebooks" element={<Notebooks />} />
          <Route path="notebooks/:id" element={<NotebookPage />} />
          <Route path="notebook-settings" element={<NotebookSettings />} />
          {/* Engine connections now live under Workspace settings; keep the old
              path working by redirecting into that tab. */}
          <Route path="engines" element={<Navigate to="../settings/engines" replace />} />
          <Route path="data-api" element={<Placeholder title="Data API" />} />
          <Route path="pipelines" element={<Placeholder title="Pipelines" />} />
          <Route path="workflows" element={<Placeholder title="Workflows" />} />
          <Route path="alerts" element={<Placeholder title="Alerts" />} />
          <Route path="data-lineage" element={<Placeholder title="Data Lineage" />} />
          <Route path="settings" element={<WorkspaceSettings />}>
            <Route index element={<Navigate to="members" replace />} />
            <Route path="members" element={<Members />} />
            <Route path="roles" element={<RolesPermissions />} />
            <Route path="shares" element={<SharedObjects />} />
            <Route path="requests" element={<AccessRequests />} />
            <Route path="engines" element={<DatabasesPage />} />
            <Route path="general" element={<WorkspaceGeneral />} />
          </Route>
          <Route path="*" element={<Navigate to="sql-lab" replace />} />
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
  { path: "policies", perm: "policy:read" },
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
  return <Navigate to={first ? first.path : "workspaces"} replace />;
}

function ControlPlaneRoutes() {
  return (
    <RequireControlPlaneAccess>
      <AppShell>
        <Routes>
          <Route index element={<ControlPlaneIndex />} />
          <Route path="workspaces" element={<RequireGlobalAdmin><Placeholder title="Workspaces" /></RequireGlobalAdmin>} />
          <Route path="engines" element={<RequirePermission perm="engine:read"><ControlPlaneEngines /></RequirePermission>} />
          <Route path="engines/:id" element={<RequirePermission perm="engine:read"><EngineDetail /></RequirePermission>} />
          <Route path="warehouses" element={<RequirePermission perm="warehouse:read"><Warehouses /></RequirePermission>} />
          <Route path="warehouses/:id" element={<RequirePermission perm="warehouse:read"><WarehouseDetail /></RequirePermission>} />
          <Route path="storage" element={<RequirePermission perm="storage:read"><ControlPlaneStorage /></RequirePermission>} />
          <Route path="storage/:id/browse" element={<RequirePermission perm="storage:read"><StorageBrowser /></RequirePermission>} />
          <Route path="secrets" element={<RequirePermission perm="secret:read"><ControlPlaneSecrets /></RequirePermission>} />
          <Route path="policies" element={<RequirePermission perm="policy:read"><Policies /></RequirePermission>} />
          <Route path="policies/:service" element={<RequirePermission perm="policy:read"><ProfileDetailPage /></RequirePermission>} />
          <Route path="policies/:service/:kind/new" element={<RequirePermission perm="policy:read"><PolicyEditorPage /></RequirePermission>} />
          <Route path="policies/:service/:kind/:id" element={<RequirePermission perm="policy:read"><PolicyEditorPage /></RequirePermission>} />
          <Route path="classifications" element={<RequirePermission perm="tag:read"><Classifications /></RequirePermission>} />
          <Route path="users" element={<RequirePermission perm="user:read"><UsersPage /></RequirePermission>} />
          <Route path="groups" element={<RequirePermission perm="group:read"><GroupsPage /></RequirePermission>} />
          <Route path="roles" element={<RequirePermission perm="role:read"><RolesPage /></RequirePermission>} />
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

export default function App() {
  return (
    <QueryClientProvider client={qc}>
      <ThemeProvider defaultTheme="system" storageKey="egret-theme">
        <BrowserRouter>
          <AuthProvider>
            <Routes>
              <Route path="/login" element={<LoginRoute />} />
              <Route path="/" element={<RequireAuth><HomeRedirect /></RequireAuth>} />
              <Route path="/explore/:workspace/*" element={<RequireAuth><ExploreRoutes /></RequireAuth>} />
              <Route path="/control-plane/*" element={<RequireAuth><ControlPlaneRoutes /></RequireAuth>} />
              <Route path="*" element={<NotFound />} />
            </Routes>
          </AuthProvider>
        </BrowserRouter>
      </ThemeProvider>
    </QueryClientProvider>
  );
}
