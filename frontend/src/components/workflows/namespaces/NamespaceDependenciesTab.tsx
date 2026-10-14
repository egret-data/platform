import { Link } from "react-router-dom";
import { Network, RotateCw, TriangleAlert } from "lucide-react";
import { ApiError } from "@/api/client";
import { useWorkflowNamespaceDependencies } from "@/api/generated/workflow/workflow";
import { EmptyState } from "@/components/explore/EmptyState";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { DependenciesView } from "@/components/workflows/namespaces/dependencies/DependenciesView";
import { namespaceHref, type NamespaceTabProps } from "@/components/workflows/namespaces/namespaceTabs";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

/** The code of the 422 a workspace over the server's flow limit (2,000) answers (W7d spec §3.4). */
const TOO_LARGE = "dependencies_too_large";

const errorMessage = (e: unknown) => (e instanceof Error && e.message ? e.message : "the request failed");

/** The workspace-wide count of drafts that do not parse, with its own plural. */
export function unparsableNote(n: number): string {
  return n === 1
    ? "1 flow in this workspace has a draft that does not parse, so the flows it calls or triggers are unknown and its edges are missing here."
    : `${n} flows in this workspace have a draft that does not parse, so the flows they call or trigger are unknown and their edges are missing here.`;
}

/**
 * THE DEPENDENCIES TAB (W7d spec §4.2): the namespace's flows, its children's, and every flow
 * elsewhere with a direct edge to or from one of them, as a graph (`DependenciesView`) built by
 * `GET …/namespaces/{ns}/dependencies` from the saved drafts.
 *
 * - **States.** A skeleton while loading (FE-STATE-2); an error first (FE-STATE-1): the 422 of a
 *   workspace over 2,000 flows shows the server's message and points to the Flows tab, any other
 *   failure offers Retry; an empty namespace has an `EmptyState`.
 * - **Unparsable drafts.** The server counts them across the WORKSPACE, not the namespace, so the
 *   note says "in this workspace".
 *
 * `NamespaceDetail.tsx` reaches this module through `lazy()` only, so it, the graph and
 * `@xyflow/react` and dagre stay out of the namespace frame's chunk (`scripts/assert-workflows-lazy.mjs`).
 */
export default function NamespaceDependenciesTab({ ws, namespace }: NamespaceTabProps) {
  const query = useWorkflowNamespaceDependencies(ws, namespace, {
    query: {
      queryKey: workflowsKeys.nsDependencies(ws, namespace),
      // A refusal (the 422 above all) answers the same again; a failure on the way may not.
      retry: (count, err) => !(err instanceof ApiError && err.status < 500) && count < 1,
    },
  });
  const deps = query.data?.status === 200 ? query.data.data : undefined;
  const flowHref = (id: string) => `/explore/${ws}/workflows/flows/${encodeURIComponent(id)}/edit`;

  let body;
  if (query.isError && !deps) {
    const tooLarge = query.error instanceof ApiError && query.error.status === 422 && query.error.code === TOO_LARGE;
    body = (
      <Alert variant="destructive">
        <AlertDescription className="flex flex-wrap items-center gap-3">
          {tooLarge ? (
            <>
              <span>
                The dependencies graph cannot be drawn: {errorMessage(query.error)}. The Flows tab still lists this namespace's flows.
              </span>
              <Button asChild variant="outline" size="sm">
                <Link to={namespaceHref(ws, namespace, "flows")}>Open the Flows tab</Link>
              </Button>
            </>
          ) : (
            <>
              <span>The dependencies could not be loaded: {errorMessage(query.error)}. Try again.</span>
              <Button variant="outline" size="sm" onClick={() => void query.refetch()}>
                <RotateCw aria-hidden />
                Retry
              </Button>
            </>
          )}
        </AlertDescription>
      </Alert>
    );
  } else if (!deps) {
    body = <Skeleton className="h-full min-h-96 w-full" />;
  } else if (deps.nodes.length === 0) {
    body = (
      <EmptyState
        icon={Network}
        title="No flows in this namespace"
        description="A flow saved in this namespace, or in one below it, appears here with the flows it calls and the flows its end triggers."
      />
    );
  } else {
    body = (
      <div className="flex h-full min-h-0 flex-col gap-2">
        {deps.unparsable > 0 ? (
          <p className="flex items-start gap-2 text-sm text-muted-foreground">
            <TriangleAlert aria-hidden className="mt-0.5 size-4 shrink-0" />
            <span className="tabular-nums">{unparsableNote(deps.unparsable)}</span>
          </p>
        ) : null}
        <div className="min-h-0 flex-1">
          <DependenciesView deps={deps} flowHref={flowHref} />
        </div>
      </div>
    );
  }

  return (
    <div data-testid="namespace-tab-dependencies" className="flex h-full min-h-[32rem] flex-col">
      {body}
    </div>
  );
}
