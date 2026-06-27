import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { BrowserRouter, Navigate, Route, Routes } from "react-router-dom";
import { ThemeProvider } from "@/components/theme-provider";
import { AuthProvider } from "@/auth/AuthProvider";
import { RequireAuth, RequirePlatform, RequireWorkspace } from "@/auth/guards";
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
import CatalogArea from "@/routes/explore/catalog/CatalogArea";
import CatalogBrowse from "@/routes/explore/catalog/CatalogBrowse";
import CatalogAccess from "@/routes/explore/catalog/CatalogAccess";
import StoragePage from "@/routes/explore/StoragePage";
import Databases from "@/routes/admin/Databases";
import Catalog from "@/routes/admin/Catalog";
import Policies from "@/routes/admin/Policies";
import Users from "@/routes/admin/Users";
import Workspaces from "@/routes/admin/Workspaces";
import Monitoring from "@/routes/admin/Monitoring";

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
          <Route path="catalog" element={<CatalogArea />}>
            <Route index element={<CatalogBrowse />} />
            <Route path="access" element={<CatalogAccess />} />
          </Route>
          <Route path="storage" element={<StoragePage />} />
          <Route path="*" element={<Navigate to="sql-lab" replace />} />
        </Routes>
      </AppShell>
    </RequireWorkspace>
  );
}

function AdminRoutes() {
  return (
    <RequirePlatform>
      <AppShell>
        <Routes>
          <Route index element={<Navigate to="databases" replace />} />
          <Route path="databases" element={<Databases />} />
          <Route path="catalog" element={<Catalog />} />
          <Route path="policies" element={<Policies />} />
          <Route path="users" element={<Users />} />
          <Route path="workspaces" element={<Workspaces />} />
          <Route path="monitoring" element={<Monitoring />} />
          <Route path="*" element={<Navigate to="databases" replace />} />
        </Routes>
      </AppShell>
    </RequirePlatform>
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
              <Route path="/admin/*" element={<RequireAuth><AdminRoutes /></RequireAuth>} />
              <Route path="*" element={<NotFound />} />
            </Routes>
          </AuthProvider>
        </BrowserRouter>
      </ThemeProvider>
    </QueryClientProvider>
  );
}
