import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { BrowserRouter, Navigate, Route, Routes } from "react-router-dom";
import { ThemeProvider } from "@/components/theme-provider";
import { AuthProvider } from "@/auth/AuthProvider";
import { RequireAuth, RequireGlobalAdmin, RequireWorkspace } from "@/auth/guards";
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
import StoragePage from "@/routes/explore/StoragePage";
import DatabasesPage from "@/routes/explore/DatabasesPage";
import SecretsPage from "@/routes/explore/SecretsPage";
import { Placeholder } from "@/routes/Placeholder";
import ControlPlaneEngines from "@/routes/control-plane/ControlPlaneEngines";

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
          <Route path="storage" element={<StoragePage />} />
          <Route path="engines" element={<DatabasesPage />} />
          <Route path="secrets" element={<SecretsPage />} />
          <Route path="data-api" element={<Placeholder title="Data API" />} />
          <Route path="pipelines" element={<Placeholder title="Pipelines" />} />
          <Route path="workflows" element={<Placeholder title="Workflows" />} />
          <Route path="alerts" element={<Placeholder title="Alerts" />} />
          <Route path="policies" element={<Placeholder title="Policies" />} />
          <Route path="data-lineage" element={<Placeholder title="Data Lineage" />} />
          <Route path="*" element={<Navigate to="sql-lab" replace />} />
        </Routes>
      </AppShell>
    </RequireWorkspace>
  );
}

function ControlPlaneRoutes() {
  return (
    <RequireGlobalAdmin>
      <AppShell>
        <Routes>
          <Route index element={<Navigate to="workspaces" replace />} />
          <Route path="workspaces" element={<Placeholder title="Workspaces" />} />
          <Route path="engines" element={<ControlPlaneEngines />} />
          <Route path="users" element={<Placeholder title="Users" />} />
          <Route path="*" element={<Navigate to="workspaces" replace />} />
        </Routes>
      </AppShell>
    </RequireGlobalAdmin>
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
