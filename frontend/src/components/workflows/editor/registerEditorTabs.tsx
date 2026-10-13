import { lazy } from "react";
import { Code, FileText, TriangleAlert, Workflow } from "lucide-react";
import { Skeleton } from "@/components/ui/skeleton";
import { LazyRouteArea } from "@/components/layout/LazyRouteArea";
import { CodeTab } from "@/components/workflows/editor/CodeTab";
import { DocsTab } from "@/components/workflows/editor/DocsTab";
import type { EditorContext } from "@/components/workflows/editor/EditorContext";
import { ProblemsBadge, ProblemsTab, problemsBadgeLabel } from "@/components/workflows/editor/ProblemsTab";
import { CODE_TAB } from "@/components/workflows/panels/layout";
import { registerTab } from "@/components/workflows/panels/registry";

/**
 * The Topology tab's body (W7c spec §6.2), which brings `@xyflow/react` and
 * dagre: its own chunk, fetched the first time the tab is shown. Nothing here
 * imports `components/workflows/topology/` statically, so none of it lands in
 * the flow pages' chunk (`scripts/assert-workflows-lazy.mjs`).
 */
const TopologyTab = lazy(() => import("@/components/workflows/topology/TopologyTab").then((m) => ({ default: m.TopologyTab })));

/**
 * Register the Code, Docs, Problems and Topology tabs (spec §5.2) with the panel frame,
 * under the ids the default layout names (`code`, `docs`, `problems`). Every
 * tab receives the page's {@link EditorContext}. Registering again replaces,
 * so calling this twice is harmless; importing the module registers them.
 */
export function registerEditorTabs(): void {
  registerTab<EditorContext>({
    id: CODE_TAB,
    title: "Code",
    icon: Code,
    closable: false,
    render: (ctx) => <CodeTab ctx={ctx} />,
  });
  registerTab<EditorContext>({
    id: "docs",
    title: "Docs",
    icon: FileText,
    closable: true,
    render: (ctx) => <DocsTab ctx={ctx} />,
  });
  registerTab<EditorContext>({
    id: "problems",
    title: "Problems",
    icon: TriangleAlert,
    closable: true,
    render: (ctx) => <ProblemsTab ctx={ctx} />,
    badge: (ctx) => <ProblemsBadge ctx={ctx} />,
    badgeLabel: problemsBadgeLabel,
  });
  // Registered with the others rather than from the Topology chunk once it loads (the late
  // registration W7a §5.1 allows): only the body is lazy, so a stored layout that names
  // `topology` always finds it registered, and the chunk is fetched only when the tab is shown.
  registerTab<EditorContext>({
    id: "topology",
    title: "Topology",
    icon: Workflow,
    closable: true,
    render: (ctx) => (
      <LazyRouteArea area="Topology" fallback={<Skeleton className="m-2 h-48" />}>
        <TopologyTab ctx={ctx} />
      </LazyRouteArea>
    ),
  });
}

registerEditorTabs();
