import { Fragment, lazy, Suspense, useState, type ComponentType } from "react";
import { Link, Navigate, useNavigate, useParams } from "react-router-dom";
import { RotateCw } from "lucide-react";
import { ApiError } from "@/api/client";
import { useWorkflowGetNamespace } from "@/api/generated/workflow/workflow";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbSeparator,
} from "@/components/ui/breadcrumb";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { EMPTY_SEGMENT_LABEL, namespaceSegments } from "@/components/workflows/namespaceTree";
import { NamespaceOverview } from "@/components/workflows/namespaces/NamespaceOverview";
import {
  isNamespaceTab,
  NAMESPACE_TAB_LABELS,
  NAMESPACE_TABS,
  namespaceHref,
  type NamespaceTab,
  type NamespaceTabProps,
} from "@/components/workflows/namespaces/namespaceTabs";
import { Forbidden } from "@/routes/Forbidden";
import ExecutionsList from "@/routes/workflows/ExecutionsList";
import FlowsList from "@/routes/workflows/FlowsList";
import { workflowsKeys } from "@/routes/workflows/queryKeys";
import { PageTitle } from "@/components/layout/PageTitle";

/**
 * THE TAB SLOTS. One body per tab, rendered with `NamespaceTabProps` once the
 * namespace has been read; only the selected tab's body is mounted, except
 * Files (see `KEEP_MOUNTED`).
 *
 * Dependencies, Files and KV are lazy: each brings what no other tab needs
 * (xyflow and dagre; Monaco and the file tree; the KV forms). Tasks 5–8
 * replace the placeholder BODIES of those modules and leave this table as it
 * is; a new import here of their contents, static, would pull them into this
 * frame's chunk, which `scripts/assert-workflows-lazy.mjs` refuses.
 */
const TAB_BODIES: Record<NamespaceTab, ComponentType<NamespaceTabProps>> = {
  overview: NamespaceOverview,
  flows: NamespaceFlowsTab,
  executions: NamespaceExecutionsTab,
  dependencies: lazy(() => import("@/components/workflows/namespaces/NamespaceDependenciesTab")),
  files: lazy(() => import("@/components/workflows/namespaces/NamespaceFilesTab")),
  kv: lazy(() => import("@/components/workflows/namespaces/NamespaceKvTab")),
};

/**
 * Tabs whose body stays mounted, hidden, once it has been shown. Files holds
 * unsaved edits in its open editors; Radix unmounts an inactive tab's content,
 * so without this a visit to KV and back dropped every unsaved edit silently.
 */
const KEEP_MOUNTED: ReadonlySet<NamespaceTab> = new Set(["files"]);

/** W7a's flow list, fixed to the namespace and its children. */
function NamespaceFlowsTab({ namespace }: NamespaceTabProps) {
  return <FlowsList pinnedNamespace={namespace} />;
}

/** W7b's executions list, fixed to the namespace and its children (`namespace_prefix`). */
function NamespaceExecutionsTab({ namespace }: NamespaceTabProps) {
  return <ExecutionsList pinnedNamespace={namespace} />;
}

const errorMessage = (e: unknown) => (e instanceof Error && e.message ? e.message : "the request failed");

/**
 * `namespaces/:namespace/:tab` (W7d spec §4.2): the namespace's breadcrumb,
 * its name, and vertical tabs in Kestra's order. The tab is the URL's last
 * segment (FE-PAGE-2); an unknown one goes to Overview.
 */
export default function NamespaceDetail() {
  const { workspace = "", namespace = "", tab } = useParams();
  if (!isNamespaceTab(tab)) return <Navigate to={namespaceHref(workspace, namespace)} replace />;
  return <NamespaceFrame key={namespace} ws={workspace} namespace={namespace} tab={tab} />;
}

function NamespaceFrame({ ws, namespace, tab }: { ws: string; namespace: string; tab: NamespaceTab }) {
  const navigate = useNavigate();
  const listHref = `/explore/${ws}/workflows/namespaces`;
  const query = useWorkflowGetNamespace(ws, namespace, { query: { queryKey: workflowsKeys.namespace(ws, namespace) } });
  const detail = query.data?.status === 200 ? query.data.data : undefined;
  // The kept tabs shown so far (adjusted during render, React's documented
  // pattern for state derived from a prop).
  const [visited, setVisited] = useState<ReadonlySet<NamespaceTab>>(() => new Set([tab]));
  if (!visited.has(tab)) setVisited(new Set([...visited, tab]));

  const crumbs = (
    <Breadcrumb>
      <BreadcrumbList>
        <BreadcrumbItem>
          <BreadcrumbLink asChild>
            <Link to={listHref}>Namespaces</Link>
          </BreadcrumbLink>
        </BreadcrumbItem>
        {namespaceSegments(namespace).map((s, i, all) => {
          const empty = s.label === "";
          const text = empty ? <span className="italic">{EMPTY_SEGMENT_LABEL}</span> : s.label;
          return (
            <Fragment key={s.path}>
              <BreadcrumbSeparator />
              <BreadcrumbItem>
                {i === all.length - 1 ? (
                  // Not `BreadcrumbPage`, which claims `role="link"`: the current
                  // namespace is plain text, marked as the page.
                  <span aria-current="page" className="font-normal text-foreground">
                    {text}
                  </span>
                ) : (
                  <BreadcrumbLink asChild>
                    <Link
                      to={namespaceHref(ws, s.path)}
                      title={s.path}
                      aria-label={empty ? `${s.path} ${EMPTY_SEGMENT_LABEL}` : undefined}
                    >
                      {text}
                    </Link>
                  </BreadcrumbLink>
                )}
              </BreadcrumbItem>
            </Fragment>
          );
        })}
      </BreadcrumbList>
    </Breadcrumb>
  );

  // Error first (FE-STATE-1), while nothing has loaded.
  if (query.isError && !detail) {
    if (query.error instanceof ApiError && query.error.status === 403) return <Forbidden />;
    // 404: no flow, file or key lives in it or below it. 400: not a namespace name at all.
    const notFound = query.error instanceof ApiError && (query.error.status === 404 || query.error.status === 400);
    return (
      <div className="p-4">
        <h1 className="sr-only">{notFound ? "Namespace not found" : "Namespace"}</h1>
        {notFound ? (
          <EmptyState
            variant="dashed"
            title="Namespace not found"
            description={
              query.error instanceof ApiError && query.error.status === 400
                ? `${query.error.message}. Pick a namespace from the list.`
                : "Nothing lives in it any more, or the link points to another workspace."
            }
            action={
              <Button asChild variant="outline" size="sm">
                <Link to={listHref}>Back to namespaces</Link>
              </Button>
            }
          />
        ) : (
          <Alert variant="destructive">
            <AlertDescription className="flex flex-wrap items-center gap-3">
              <span>The namespace could not be loaded: {errorMessage(query.error)}. Try again.</span>
              <Button variant="outline" size="sm" onClick={() => void query.refetch()}>
                <RotateCw aria-hidden />
                Retry
              </Button>
            </AlertDescription>
          </Alert>
        )}
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="px-4 pt-4">
        {crumbs}
        <PageTitle className="mt-1 break-all">{namespace}</PageTitle>
      </header>
      {detail ? (
        <Tabs
          value={tab}
          orientation="vertical"
          // Manual: arrows move focus, Enter or Space selects. Automatic
          // activation navigated (one history entry) per arrow press.
          activationMode="manual"
          onValueChange={(t) => navigate(namespaceHref(ws, namespace, t as NamespaceTab))}
          className="mt-3 flex min-h-0 flex-1 flex-col gap-4 px-4 pb-4 md:flex-row"
        >
          <TabsList className="h-auto shrink-0 flex-row flex-wrap items-stretch justify-start self-start md:w-44 md:flex-col">
            {NAMESPACE_TABS.map((t) => (
              <TabsTrigger key={t} value={t} className="justify-start">
                {NAMESPACE_TAB_LABELS[t]}
              </TabsTrigger>
            ))}
          </TabsList>
          {NAMESPACE_TABS.map((t) => {
            const Body = TAB_BODIES[t];
            const keep = KEEP_MOUNTED.has(t) && visited.has(t);
            return (
              <TabsContent
                key={t}
                value={t}
                forceMount={keep ? true : undefined}
                // Radix spreads this after its own `hidden`, which is false
                // for a force-mounted panel: a kept panel hides itself.
                hidden={t !== tab}
                className="mt-0 min-h-0 min-w-0 flex-1 overflow-auto"
              >
                <Suspense fallback={<ListSkeleton cols={3} />}>
                  <Body ws={ws} namespace={namespace} detail={detail} canManage={detail.can_manage} />
                </Suspense>
              </TabsContent>
            );
          })}
        </Tabs>
      ) : (
        <div className="p-4">
          <ListSkeleton cols={3} />
        </div>
      )}
    </div>
  );
}
