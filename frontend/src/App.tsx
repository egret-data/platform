import { lazy } from "react";
import { QueryClientProvider } from "@tanstack/react-query";
import {
  BrowserRouter,
  Link,
  Navigate,
  Route,
  Routes,
  useParams,
  useSearchParams,
} from "react-router-dom";
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
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { LazyRouteArea } from "@/components/layout/LazyRouteArea";

/**
 * The Workflows area, code-split (FE-ROUTE-2): the first lazy route area in
 * this file. Nothing under `routes/workflows/` or `components/workflows/` may
 * be imported statically from here or from anything the entry chunk reaches;
 * `npm run test:bundle:workflows` (`scripts/assert-workflows-lazy.mjs`)
 * fails the build's check if it is.
 */
const WorkflowsRoutes = lazy(() => import("@/routes/workflows/WorkflowsRoutes"));

/**
 * The Governance Data area (DG redesign #1), code-split like Workflows
 * (FE-ROUTE-2): the catalog tree and the object pages. Nothing under
 * `routes/governance/data/` may be imported statically from the entry graph,
 * except `objectPath.ts`, which old list pages link through;
 * `npm run test:bundle:governance-data` checks it.
 */
const DataRoutes = lazy(() => import("@/routes/governance/data/DataRoutes"));

/**
 * The Governance People area (DG redesign #2): the People & groups index and
 * the principal pages, code-split like Data (FE-ROUTE-2). Nothing under
 * `routes/governance/people/` may be imported statically from the entry graph,
 * except `peopleUrl.ts`, which pages outside the area link through;
 * `npm run test:bundle:governance-people` checks it.
 */
const PeopleRoutes = lazy(() => import("@/routes/governance/people/PeopleRoutes"));

/** What the People area shows while its chunk loads: a list skeleton in the page frame. */
function PeopleFallback() {
  return (
    <div className="p-4" data-testid="governance-people-loading">
      <ListSkeleton cols={3} />
    </div>
  );
}

/** What the Data area shows while its chunk loads: a skeleton in the page frame. */
function DataFallback() {
  return (
    <div className="p-4" data-testid="governance-data-loading">
      <ListSkeleton cols={1} />
    </div>
  );
}

/** What the Workflows area shows while its chunk loads: a list skeleton in the page frame. */
function WorkflowsFallback() {
  return (
    <div className="p-4" data-testid="workflows-loading">
      <ListSkeleton cols={4} />
    </div>
  );
}

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
import Apps from "@/routes/explore/Apps";
import AppEditPage from "@/routes/explore/AppEditPage";
import AppViewPage from "@/routes/explore/AppViewPage";
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
import EngineRegister from "@/routes/control-plane/EngineRegister";
import ControlPlaneStorage from "@/routes/control-plane/ControlPlaneStorage";
import StorageDetail from "@/routes/control-plane/StorageDetail";
import StorageBrowser from "@/routes/control-plane/StorageBrowser";
import ControlPlaneSecrets from "@/routes/control-plane/ControlPlaneSecrets";
import SecretDetail from "@/routes/control-plane/SecretDetail";
import DataPermissions from "@/routes/governance/DataPermissions";
import MaskFilterRules from "@/routes/governance/MaskFilterRules";
import ExplainAccess from "@/routes/governance/ExplainAccess";
import Policies from "@/routes/governance/Policies";
import ProfileDetailPage from "@/routes/governance/ProfileDetailPage";
import PolicyMigrationPage from "@/routes/governance/PolicyMigrationPage";
import PolicyEditorPage from "@/routes/governance/PolicyEditorPage";
import { useControlPlaneCatalogRegistryListRegistryCatalogs } from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import Catalogs from "@/routes/control-plane/catalogs/Catalogs";
import CatalogCreate from "@/routes/control-plane/catalogs/CatalogCreate";
import CatalogDetail from "@/routes/control-plane/catalogs/CatalogDetail";
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
          <Route path="apps" element={<RequireFeature feature="apps"><Apps /></RequireFeature>} />
          <Route path="apps/:id/edit" element={<RequireFeature feature="apps"><AppEditPage /></RequireFeature>} />
          <Route path="apps/:id" element={<RequireFeature feature="apps"><AppViewPage /></RequireFeature>} />
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
          <Route
            path="workflows/*"
            element={
              <RequireFeature feature="workflows">
                <LazyRouteArea area="Workflows" fallback={<WorkflowsFallback />}>
                  <WorkflowsRoutes />
                </LazyRouteArea>
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

/**
 * SP3c §3, narrowed by D5, and the one Warehouses page Task 8 KEEPS:
 * `/control-plane/warehouses/:id` redirects to the matching catalog. The
 * Warehouses LIST and its sidebar entry are gone (Task 8); this route stays,
 * unlisted, because it already answers the warehouse-id → catalog-id lookup
 * that `WorkspaceDetail` and `StorageDetail` would otherwise each have to
 * grow, and it keeps a bookmarked warehouse URL working — its five tabs now
 * render on `CatalogDetail` (D1), so a link to the old warehouse detail page
 * opens them there instead.
 *
 * The id in the old URL is a WAREHOUSE id. `RegistryCatalogDto.warehouseId` is
 * the only field that maps it to a catalog id, and it is only on the list, so
 * the redirect has to wait for that list rather than resolve the id locally.
 *
 * A warehouse with no registry row is rare but NOT impossible —
 * `ensure_warehouse_catalog` is non-fatal on failure, and SP3c-2 Task 1 removed
 * the boot backfill that used to be a second chance at it. A 404 here is not a
 * crash: it lands on the Catalogs list, which is where the operator would have
 * gone next anyway. `?tab=` is carried across because the
 * five tabs this page now redirects INTO kept their original `value`s, so a
 * bookmarked `?tab=browse` still opens Browse.
 *
 * THE MISSING DESTINATION KEY WAS EXPLAINED HERE, NOT 403'D (whole-branch
 * review, F5) — through SP3c-1, while this route was gated `warehouse:read`,
 * the SAME key as the list that linked to it, so an account could reach this
 * component without holding `catalog_registry:read` (migration 0148's grant
 * was one-time, and a role created after it never received the new key).
 * SP3c-2 (Task 7) retired `warehouse:read` and re-gated this route on
 * `catalog_registry:read` (migration 0153 re-ran 0148's grant first, so
 * nobody lost the capability in the move) — every account that could reach
 * this route through `RequirePermission` already holds that key, so the
 * branch below can no longer fire from normal navigation: it is
 * defense-in-depth against a client-side `me.permissions` snapshot that is
 * stale relative to the guard's own check, not a reachable steady state.
 */
function WarehouseRedirect() {
  const { id = "" } = useParams();
  const { me } = useAuth();
  const canReadRegistry =
    !!me &&
    (me.platform_roles.includes("platform_admin") ||
      me.permissions.includes("catalog_registry:read"));
  const q = useControlPlaneCatalogRegistryListRegistryCatalogs(undefined, {
    // The registry list is `catalog_registry:read`. Not sent without it: the
    // 403 would tell this page nothing it does not already know, and an
    // `isError` arm behind an `enabled` that is false never fires — the exact
    // inert-fix shape Task 11 found on the Engines tab.
    query: { retry: false, enabled: canReadRegistry },
  });
  const [sp] = useSearchParams();
  if (!canReadRegistry)
    return (
      <div className="space-y-3 p-10" data-testid="warehouse-redirect-blocked">
        <h1 className="text-xl font-semibold">This page moved into Catalogs</h1>
        <p className="max-w-2xl text-sm text-muted-foreground">
          A warehouse&apos;s roles and grants, Catalog Clients, namespace browser, connection
          details and settings are now tabs on its catalog. Finding that catalog needs the{" "}
          <code className="font-mono">catalog_registry:read</code> permission, which this account
          does not have. A platform administrator can grant it.
        </p>
        <p className="text-sm">
          <Link to="/control-plane/catalogs" className="text-primary hover:underline">
            Back to Catalogs
          </Link>
        </p>
      </div>
    );
  if (q.isLoading) return <div className="p-10">Loading…</div>;
  const rows = q.data?.status === 200 ? q.data.data : [];
  const match = rows.find((c) => c.warehouseId === id);
  const tab = sp.get("tab");
  return (
    <Navigate
      replace
      to={
        match
          ? `/control-plane/catalogs/${match.id}${tab ? `?tab=${encodeURIComponent(tab)}` : ""}`
          : "/control-plane/catalogs"
      }
    />
  );
}

// First section (in preference order) each control-plane read permission
// unlocks. Used to redirect a scoped (non platform_admin) user landing on
// `/control-plane` to a section they can actually see, instead of the
// platform_admin-only "workspaces" default.
const FIRST_SECTION_BY_PERM: { path: string; perm: string }[] = [
  { path: "engines", perm: "engine:read" },
  { path: "catalogs", perm: "catalog_registry:read" },
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
          <Route path="engines/register" element={<RequirePermission perm="engine:create"><EngineRegister /></RequirePermission>} />
          <Route path="engines/:id" element={<RequirePermission perm="engine:read"><EngineDetail /></RequirePermission>} />
          <Route path="catalogs" element={<RequirePermission perm="catalog_registry:read"><Catalogs /></RequirePermission>} />
          <Route path="catalogs/new" element={<RequirePermission perm="catalog_registry:create"><CatalogCreate /></RequirePermission>} />
          <Route path="catalogs/:id" element={<RequirePermission perm="catalog_registry:read"><CatalogDetail /></RequirePermission>} />
          {/* D5, completed by Task 8: the warehouse LIST is gone; only this
              detail path survives, unlisted, as a compatibility redirect.

              The warehouse DETAIL page is the one that could not go: its five
              tabs moved onto `CatalogDetail` (D1), so this path resolves the
              warehouse id to its catalog and sends the operator to the same
              tabs there — see `WarehouseRedirect`'s own doc comment for why
              it stays instead of pushing that lookup onto every caller. */}
          <Route path="warehouses/:id" element={<RequirePermission perm="catalog_registry:read"><WarehouseRedirect /></RequirePermission>} />
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
// gate, then a per-route permission.
//
// The landing is Data (DG redesign spec Part 1, "until #3 lands, the Governance
// index opens Data"). Data takes either Governance read permission, which is
// exactly what `RequireGovernanceAccess` admits, so every user who reaches this
// index can open it. The target is absolute: this also renders for the `*`
// catch-all, where react-router 7 resolves a relative target against the
// unmatched URL and loops (see `exploreRedirectTo`).
function GovernanceIndex() {
  return <Navigate to="/governance/data" replace />;
}

function GovernanceRoutes() {
  return (
    <RequireGovernanceAccess>
      <AppShell>
        <Routes>
          <Route index element={<GovernanceIndex />} />
          {/* DG redesign #1: the catalog tree and the object pages. Either read
              permission opens it: the tree reveals catalog structure, which the
              browse API serves to `policy:read` or `tag:read`. */}
          <Route
            path="data/*"
            element={
              <RequirePermission perm={["policy:read", "tag:read"]}>
                <LazyRouteArea area="Data" fallback={<DataFallback />}>
                  <DataRoutes />
                </LazyRouteArea>
              </RequirePermission>
            }
          />
          {/* DG redesign #2 (spec P1, P2): people, groups and roles, gated
              `policy:read`; a steward holding only `tag:read` is refused. */}
          <Route
            path="people/*"
            element={
              <RequirePermission perm="policy:read">
                <LazyRouteArea area="People & groups" fallback={<PeopleFallback />}>
                  <PeopleRoutes />
                </LazyRouteArea>
              </RequirePermission>
            }
          />
          {/* SP4b-2: data-plane permission UI, gated the same way the existing
              governance routes are — see the design spec's D4/D6 for why these
              are separate pages rather than a mode switch on Policies. */}
          <Route path="data-permissions" element={<RequirePermission perm="policy:read"><DataPermissions /></RequirePermission>} />
          <Route path="mask-filter-rules" element={<RequirePermission perm="policy:read"><MaskFilterRules /></RequirePermission>} />
          <Route path="explain-access" element={<RequirePermission perm="policy:read"><ExplainAccess /></RequirePermission>} />
          <Route path="policies" element={<RequirePermission perm="policy:read"><Policies /></RequirePermission>} />
          <Route path="policies/:service" element={<RequirePermission perm="policy:read"><ProfileDetailPage /></RequirePermission>} />
          {/* SP4d-3: reading is `policy:read`; Preview, Migrate and Undo need
              `policy:update`, which the page checks per control. */}
          <Route path="policies/:service/migrate" element={<RequirePermission perm="policy:read"><PolicyMigrationPage /></RequirePermission>} />
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
