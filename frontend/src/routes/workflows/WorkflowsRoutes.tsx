import { lazy, Suspense, type ReactNode } from "react";
import { Navigate, Route, Routes, useParams } from "react-router-dom";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Placeholder } from "@/routes/Placeholder";
import FlowsList from "@/routes/workflows/FlowsList";
import ExecutionsPlaceholder from "@/routes/workflows/ExecutionsPlaceholder";

/**
 * The flow pages, a lazy chunk of their own inside this one: they bring the
 * multi-panel frame, the editor tabs (which they register) and `yaml`, none
 * of which the flow list needs. `scripts/assert-workflows-lazy.mjs` checks
 * they stay out of the list's chunk as well as the entry.
 */
const FlowDetail = lazy(() => import("@/routes/workflows/FlowDetail"));
const FlowNew = lazy(() => import("@/routes/workflows/FlowNew"));

/** The flow pages' loading state (FE-STATE-2). A failed chunk reaches `LazyRouteArea`'s boundary. */
function FlowPage({ children }: { children: ReactNode }) {
  return (
    <Suspense
      fallback={
        <div className="p-4" data-testid="flow-page-loading">
          <ListSkeleton cols={2} />
        </div>
      }
    >
      {children}
    </Suspense>
  );
}

/**
 * The Workflows area, `/explore/:workspace/workflows/*` (spec §3).
 *
 * `App.tsx` reaches this module only through `lazy()`, so everything it
 * imports is split out of the entry chunk (FE-ROUTE-2) and
 * `scripts/assert-workflows-lazy.mjs` checks that it stays out. The sub-routes
 * are declared here rather than in `App.tsx` for that reason: a page named in
 * `App.tsx` would be imported eagerly. The guard (`RequireFeature
 * feature="workflows"`) wraps the whole area in `App.tsx`.
 *
 * Redirects are absolute (FE-ROUTE-1).
 */
export default function WorkflowsRoutes() {
  const ws = useParams().workspace!;
  const base = `/explore/${ws}/workflows`;
  return (
    <Routes>
      <Route index element={<Navigate to={`${base}/flows`} replace />} />
      <Route path="flows" element={<FlowsList />} />
      {/* `new` before `:id`, as the convention in `App.tsx` records. */}
      <Route path="flows/new" element={<FlowPage><FlowNew /></FlowPage>} />
      <Route path="flows/:id" element={<FlowIndexRedirect base={base} />} />
      {/* `FlowDetail` sends an unknown tab to `edit`. */}
      <Route path="flows/:id/:tab" element={<FlowPage><FlowDetail /></FlowPage>} />
      <Route path="executions" element={<ExecutionsPlaceholder />} />
      <Route path="executions/:id" element={<Placeholder title="Execution" />} />
      <Route path="*" element={<Navigate to={`${base}/flows`} replace />} />
    </Routes>
  );
}

function FlowIndexRedirect({ base }: { base: string }) {
  const { id } = useParams();
  return <Navigate to={`${base}/flows/${encodeURIComponent(id ?? "")}/edit`} replace />;
}
