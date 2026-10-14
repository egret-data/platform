import { lazy } from "react";
import { Code, FileCode, FileText, FolderTree, TriangleAlert, Workflow } from "lucide-react";
import { Skeleton } from "@/components/ui/skeleton";
import { LazyRouteArea } from "@/components/layout/LazyRouteArea";
import { CodeTab } from "@/components/workflows/editor/CodeTab";
import { DocsTab } from "@/components/workflows/editor/DocsTab";
import type { EditorContext } from "@/components/workflows/editor/EditorContext";
import { FILE_TAB_PREFIX, fileNameOf, filePathOf } from "@/components/workflows/editor/fileTabs";
import { ProblemsBadge, ProblemsTab, problemsBadgeLabel } from "@/components/workflows/editor/ProblemsTab";
import { CODE_TAB } from "@/components/workflows/panels/layout";
import { registerTab, registerTabFamily } from "@/components/workflows/panels/registry";

/**
 * The Topology tab's body (W7c spec §6.2), which brings `@xyflow/react` and
 * dagre: its own chunk, fetched the first time the tab is shown. Nothing here
 * imports `components/workflows/topology/` statically, so none of it lands in
 * the flow pages' chunk (`scripts/assert-workflows-lazy.mjs`).
 */
const TopologyTab = lazy(() => import("@/components/workflows/topology/TopologyTab").then((m) => ({ default: m.TopologyTab })));

/**
 * The Files tab's body and a file tab's body (W7d spec §4.3), which bring the
 * namespace file explorer, the file editor and Monaco's wrapper: reached
 * through `lazy()` only, like Topology's, so none of `namespaces/files/` lands
 * in the flow pages' chunk (`scripts/assert-workflows-lazy.mjs`).
 */
const FilesTab = lazy(() => import("@/components/workflows/editor/files/FilesTab").then((m) => ({ default: m.FilesTab })));
const FileTab = lazy(() => import("@/components/workflows/editor/files/FileTab").then((m) => ({ default: m.FileTab })));

/** A file tab's path, from an id the `file:` family answers for (never empty). */
const pathOf = (id: string) => filePathOf(id) ?? `/${id}`;

/**
 * Register the Code, Docs, Problems, Topology and Files tabs, and the `file:` family (spec §5.2) with the panel frame,
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
  // W7d spec §4.3: the Files tab, registered as Topology is (a stub whose body loads lazily), so
  // a stored layout that names `files` always finds it.
  registerTab<EditorContext>({
    id: "files",
    title: "Files",
    icon: FolderTree,
    closable: true,
    render: (ctx) => (
      <LazyRouteArea area="Files" fallback={<Skeleton className="m-2 h-48" />}>
        <FilesTab ctx={ctx} />
      </LazyRouteArea>
    ),
  });
  // One tab per open namespace file, `file:<path>`: titled with the file's name, named with its
  // path, and dotted while it has unsaved edits. Registered here too, so a stored layout that names
  // a file tab keeps it; the tab then reads the file again.
  registerTabFamily<EditorContext>({
    prefix: FILE_TAB_PREFIX,
    title: (id) => fileNameOf(pathOf(id)),
    label: (id) => `${fileNameOf(pathOf(id))} (${pathOf(id)})`,
    icon: FileCode,
    closable: true,
    render: (id, ctx) => (
      <LazyRouteArea area="Files" fallback={<Skeleton className="m-2 h-48" />}>
        <FileTab ctx={ctx} path={pathOf(id)} />
      </LazyRouteArea>
    ),
    // A dot, and ", unsaved" in the tab's name: never the dot alone (FE-A11Y-4).
    badge: (id, ctx) =>
      ctx.files?.dirtyPaths.includes(pathOf(id)) ? <span aria-hidden data-testid="file-tab-dirty" className="size-2 shrink-0 rounded-full bg-foreground" /> : null,
    badgeLabel: (id, ctx) => (ctx.files?.dirtyPaths.includes(pathOf(id)) ? "unsaved" : undefined),
  });
}

registerEditorTabs();
