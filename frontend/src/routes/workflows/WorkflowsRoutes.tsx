import { Navigate, Route, Routes, useParams } from "react-router-dom";
import { Placeholder } from "@/routes/Placeholder";
import FlowsList from "@/routes/workflows/FlowsList";
import ExecutionsPlaceholder from "@/routes/workflows/ExecutionsPlaceholder";

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
      <Route path="flows/new" element={<Placeholder title="New flow" />} />
      <Route path="flows/:id" element={<FlowIndexRedirect base={base} />} />
      <Route path="flows/:id/:tab" element={<FlowDetailPlaceholder base={base} />} />
      <Route path="executions" element={<ExecutionsPlaceholder />} />
      <Route path="executions/:id" element={<Placeholder title="Execution" />} />
      <Route path="*" element={<Navigate to={`${base}/flows`} replace />} />
    </Routes>
  );
}

/** The flow detail's tabs in W7a; W7b and W7e add theirs here. */
const FLOW_TABS = new Set(["edit", "revisions"]);

function FlowIndexRedirect({ base }: { base: string }) {
  const { id } = useParams();
  return <Navigate to={`${base}/flows/${encodeURIComponent(id ?? "")}/edit`} replace />;
}

/** Holds `flows/:id/:tab` until the flow editor lands; an unknown tab goes to `edit`. */
function FlowDetailPlaceholder({ base }: { base: string }) {
  const { id, tab } = useParams();
  if (!tab || !FLOW_TABS.has(tab)) {
    return <Navigate to={`${base}/flows/${encodeURIComponent(id ?? "")}/edit`} replace />;
  }
  return <Placeholder title="Flow" />;
}
