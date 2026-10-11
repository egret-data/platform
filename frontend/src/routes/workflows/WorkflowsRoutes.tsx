import { lazy, Suspense, type ReactNode } from "react";
import { Navigate, Route, Routes, useParams } from "react-router-dom";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import FlowsList from "@/routes/workflows/FlowsList";
import ExecutionsList from "@/routes/workflows/ExecutionsList";

/**
 * The flow pages, a lazy chunk of their own inside this one: they bring the
 * multi-panel frame, the editor tabs (which they register) and `yaml`, none
 * of which the flow list needs. `scripts/assert-workflows-lazy.mjs` checks
 * they stay out of the list's chunk as well as the entry.
 */
const FlowDetail = lazy(() => import("@/routes/workflows/FlowDetail"));
const FlowNew = lazy(() => import("@/routes/workflows/FlowNew"));
/**
 * The execution detail (W7b), split the same way: it reads the frozen source
 * with `yaml` to decide Resume, and Tasks 4–6 hang the gantt, logs and
 * outputs off it, none of which the lists need.
 */
const ExecutionDetail = lazy(() => import("@/routes/workflows/ExecutionDetail"));

/**
 * The loading state of the flow and execution pages (FE-STATE-2). A failed
 * chunk reaches `LazyRouteArea`'s boundary.
 */
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
      {/* `FlowDetail` sends an unknown tab to `edit`. Its tabs are `edit`,
          `revisions` and `executions` (W7b: the flow's executions list). */}
      <Route path="flows/:id/:tab" element={<FlowPage><FlowDetail /></FlowPage>} />
      <Route path="executions" element={<ExecutionsList />} />
      {/* The execution detail (W7b spec §4). `ExecutionDetail` sends an unknown tab to
          `overview`; its tabs are `overview`, `gantt`, `logs` and `outputs`. */}
      <Route path="executions/:id" element={<ExecutionIndexRedirect base={base} />} />
      <Route path="executions/:id/:tab" element={<FlowPage><ExecutionDetail /></FlowPage>} />
      <Route path="*" element={<Navigate to={`${base}/flows`} replace />} />
    </Routes>
  );
}

function FlowIndexRedirect({ base }: { base: string }) {
  const { id } = useParams();
  return <Navigate to={`${base}/flows/${encodeURIComponent(id ?? "")}/edit`} replace />;
}

function ExecutionIndexRedirect({ base }: { base: string }) {
  const { id } = useParams();
  return <Navigate to={`${base}/executions/${encodeURIComponent(id ?? "")}/overview`} replace />;
}
