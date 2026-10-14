import type { ReactNode } from "react";
import { Link } from "react-router-dom";
import { History, RotateCw } from "lucide-react";
import { useWorkflowListExecutions, useWorkflowListNamespaces } from "@/api/generated/workflow/workflow";
import type { ExecutionListItemDto } from "@/api/generated/model";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Table, TableBody, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { isTerminalState } from "@/components/workflows/executionState";
import { useNow } from "@/components/workflows/executions/polling";
import { directChildNamespaces, EMPTY_SEGMENT_LABEL } from "@/components/workflows/namespaceTree";
import { namespaceHref, type NamespaceTabProps } from "@/components/workflows/namespaces/namespaceTabs";
import { ExecutionRow } from "@/routes/workflows/ExecutionsList";
import { LIST_POLL_MS } from "@/routes/workflows/executionsListModel";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

/** How many executions the Overview shows (W7d spec §4.2). */
export const RECENT_EXECUTIONS = 10;

const errorMessage = (e: unknown) => (e instanceof Error && e.message ? e.message : "the request failed");

/**
 * The namespace's Overview (W7d spec §4.2; §2 records why it is not Kestra's
 * executions dashboard): its own counts, its 10 most recent executions with
 * its children's, and its direct child namespaces.
 */
export function NamespaceOverview({ ws, namespace, detail }: NamespaceTabProps) {
  return (
    <div className="grid gap-6">
      <section aria-labelledby="ns-counts">
        <h2 id="ns-counts" className="mb-1 text-base font-semibold">
          In this namespace
        </h2>
        <div data-testid="namespace-counts">
          <SummaryStrip
            items={[
              { label: "flows", singular: "flow", value: detail.flows },
              { label: "files", singular: "file", value: detail.files },
              { label: "KV keys", singular: "KV key", value: detail.kv_keys },
            ]}
          />
        </div>
        <p className="mt-1 text-sm text-muted-foreground">
          Counted in this namespace only; each child namespace counts its own.
        </p>
      </section>
      <RecentExecutions ws={ws} namespace={namespace} />
      <ChildNamespaces ws={ws} namespace={namespace} />
    </div>
  );
}

function Section({ id, title, action, children }: { id: string; title: string; action?: ReactNode; children: ReactNode }) {
  return (
    <section aria-labelledby={id}>
      <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
        <h2 id={id} className="text-base font-semibold">
          {title}
        </h2>
        {action}
      </div>
      {children}
    </section>
  );
}

/**
 * W7b's executions list, cut to its first 10 rows: root executions of the
 * namespace and its children (`namespace_prefix`), newest first, polled while
 * one of them is not finished, as the list is.
 */
function RecentExecutions({ ws, namespace }: { ws: string; namespace: string }) {
  const base = `/explore/${ws}/workflows`;
  const query = useWorkflowListExecutions(
    ws,
    { namespace_prefix: namespace, root_only: true, limit: RECENT_EXECUTIONS, offset: 0 },
    {
      query: {
        queryKey: workflowsKeys.executions(ws, { recentInNamespace: namespace, limit: RECENT_EXECUTIONS }),
        refetchInterval: (q) => {
          const data = q.state.data;
          return data?.status === 200 && data.data.rows.some((r) => !isTerminalState(r.state)) ? LIST_POLL_MS : false;
        },
        refetchIntervalInBackground: false,
      },
    },
  );
  const now = useNow();
  const rows: ExecutionListItemDto[] | undefined = query.data?.status === 200 ? query.data.data.rows : undefined;

  let body: ReactNode;
  if (query.isError && !rows) {
    body = (
      <Alert variant="destructive">
        <AlertDescription className="flex flex-wrap items-center gap-3">
          <span>The executions could not be loaded: {errorMessage(query.error)}. Try again.</span>
          <Button variant="outline" size="sm" onClick={() => void query.refetch()}>
            <RotateCw aria-hidden />
            Retry
          </Button>
        </AlertDescription>
      </Alert>
    );
  } else if (!rows) {
    body = <ListSkeleton rows={3} cols={7} />;
  } else if (rows.length === 0) {
    body = (
      <EmptyState
        icon={History}
        title="No executions in this namespace yet"
        description="Each run of a flow in this namespace, or in a namespace below it, appears here."
      />
    );
  } else {
    body = (
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>ID</TableHead>
            <TableHead>Flow</TableHead>
            <TableHead>Namespace</TableHead>
            <TableHead>State</TableHead>
            <TableHead>Start</TableHead>
            <TableHead>Duration</TableHead>
            <TableHead>Trigger</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map((row) => (
            <ExecutionRow key={row.id} row={row} base={base} showFlow now={now} />
          ))}
        </TableBody>
      </Table>
    );
  }

  return (
    <div data-testid="namespace-recent-executions">
      <Section
        id="ns-recent"
        title="Recent executions"
        action={
          <Button asChild variant="outline" size="sm">
            <Link to={namespaceHref(ws, namespace, "executions")}>See all executions</Link>
          </Button>
        }
      >
        {body}
      </Section>
    </div>
  );
}

/** The namespaces one dot below, each linked to its own Overview, with its own counts. */
function ChildNamespaces({ ws, namespace }: { ws: string; namespace: string }) {
  const query = useWorkflowListNamespaces(ws, { prefix: namespace }, {
    query: { queryKey: workflowsKeys.namespaces(ws, namespace) },
  });
  const rows = query.data?.status === 200 ? directChildNamespaces(query.data.data, namespace) : undefined;

  let body: ReactNode;
  if (query.isError) {
    body = (
      <Alert variant="destructive">
        <AlertDescription className="flex flex-wrap items-center gap-3">
          <span>The child namespaces could not be loaded: {errorMessage(query.error)}. Try again.</span>
          <Button variant="outline" size="sm" onClick={() => void query.refetch()}>
            <RotateCw aria-hidden />
            Retry
          </Button>
        </AlertDescription>
      </Alert>
    );
  } else if (!rows) {
    body = <ListSkeleton rows={2} cols={2} />;
  } else if (rows.length === 0) {
    body = <p className="text-sm text-muted-foreground">This namespace has no child namespaces.</p>;
  } else {
    body = (
      <ul role="list" className="grid gap-1">
        {rows.map((r) => {
          const segment = r.namespace.slice(namespace.length + 1);
          return (
            <li key={r.namespace} className="flex flex-wrap items-baseline gap-x-3 text-sm">
              <Link
                to={namespaceHref(ws, r.namespace)}
                title={r.namespace}
                aria-label={segment === "" ? `${r.namespace} ${EMPTY_SEGMENT_LABEL}` : r.namespace}
                className={`rounded-sm font-medium hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring ${
                  segment === "" ? "italic" : ""
                }`}
              >
                {segment === "" ? EMPTY_SEGMENT_LABEL : segment}
              </Link>
              <span className="tabular-nums text-muted-foreground">
                {r.own
                  ? `${r.flows} ${r.flows === 1 ? "flow" : "flows"} · ${r.files} ${r.files === 1 ? "file" : "files"} · ${r.kv_keys} ${r.kv_keys === 1 ? "KV key" : "KV keys"}`
                  : "Parent only"}
              </span>
            </li>
          );
        })}
      </ul>
    );
  }

  return (
    <div data-testid="namespace-children">
      <Section id="ns-children" title="Child namespaces">
        {body}
      </Section>
    </div>
  );
}
