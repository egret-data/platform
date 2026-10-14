import { useMemo } from "react";
import { Link } from "react-router-dom";
import { Layers, RotateCw } from "lucide-react";
import type { KvInheritedDto } from "@/api/generated/model";
import { useWorkflowListInheritedKv } from "@/api/generated/workflow/workflow";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { namespaceHref } from "@/components/workflows/namespaces/namespaceTabs";
import { workflowsKeys } from "@/routes/workflows/queryKeys";
import { kvErrorMessage } from "./kvApi";
import { formatInstant } from "./KvTable";

/**
 * The keys a namespace's parents hold, which its flows read through `kv()`
 * (W7d spec §4.4; W9c `GET …/kv/inheritance`). `kv('key')` reads the flow's
 * own namespace first, then each parent up the dots: the nearest namespace
 * holding the key wins. The server lists the parents' keys deepest first,
 * each once; this keeps the first of any key named twice, so the rule holds
 * here whatever the order, and marks a key the namespace itself holds, which
 * its own copy overrides.
 */
export function nearestFirst(rows: KvInheritedDto[], namespace: string): KvInheritedDto[] {
  // Deepest first: a longer ancestor of `namespace` is nearer. Stable for equals.
  const depth = (ns: string) => (namespace === ns || namespace.startsWith(`${ns}.`) ? ns.length : -1);
  const ordered = [...rows].sort((a, b) => depth(b.namespace) - depth(a.namespace));
  const seen = new Set<string>();
  return ordered
    .filter((r) => {
      if (seen.has(r.key)) return false;
      seen.add(r.key);
      return true;
    })
    .sort((a, b) => a.key.localeCompare(b.key));
}

export function InheritedKvDrawer({
  ws,
  namespace,
  ownKeys,
  onClose,
}: {
  ws: string;
  namespace: string;
  ownKeys: Set<string>;
  onClose: () => void;
}) {
  const query = useWorkflowListInheritedKv(ws, namespace, { query: { queryKey: workflowsKeys.kvInherited(ws, namespace) } });
  const raw = query.data?.status === 200 ? query.data.data : undefined;
  const rows = useMemo(() => (raw ? nearestFirst(raw, namespace) : undefined), [raw, namespace]);

  let body;
  if (query.isError && !rows) {
    body = (
      <Alert variant="destructive">
        <AlertDescription className="flex flex-wrap items-center gap-3">
          <span>{kvErrorMessage(query.error, "The inherited keys could not be loaded")}</span>
          <Button variant="outline" size="sm" onClick={() => void query.refetch()}>
            <RotateCw aria-hidden />
            Retry
          </Button>
        </AlertDescription>
      </Alert>
    );
  } else if (!rows) {
    body = <ListSkeleton cols={4} />;
  } else if (rows.length === 0) {
    body = (
      <EmptyState
        variant="dashed"
        icon={Layers}
        title="No inherited keys"
        description={
          namespace.includes(".")
            ? `No parent namespace of ${namespace} holds a live key.`
            : `${namespace} has no parent namespace to inherit keys from.`
        }
      />
    );
  } else {
    body = (
      <div className="overflow-x-auto rounded-md border">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Key</TableHead>
              <TableHead>Namespace</TableHead>
              <TableHead>Type</TableHead>
              <TableHead>Description</TableHead>
              <TableHead>Last modified</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.map((row) => (
              <TableRow key={row.key} data-key={row.key}>
                <TableCell className="max-w-48 break-all">
                  <span className="font-mono text-xs">{row.key}</span>
                  {ownKeys.has(row.key) ? (
                    <span className="block text-xs text-muted-foreground">Overridden by this namespace</span>
                  ) : null}
                </TableCell>
                <TableCell className="break-all">
                  <Link
                    to={namespaceHref(ws, row.namespace, "kv")}
                    onClick={onClose}
                    className="rounded-sm hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                  >
                    {row.namespace}
                  </Link>
                </TableCell>
                <TableCell>
                  <Badge variant="outline">{row.type}</Badge>
                </TableCell>
                <TableCell className="text-muted-foreground">{row.description || "—"}</TableCell>
                <TableCell className="whitespace-nowrap tabular-nums">{formatInstant(row.updated)}</TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
    );
  }

  return (
    <Sheet open onOpenChange={(open) => (open ? undefined : onClose())}>
      <SheetContent className="max-w-3xl">
        <SheetHeader>
          <SheetTitle>Inherited KV</SheetTitle>
          <SheetDescription>
            Keys of the parent namespaces of {namespace}. A flow's {"{{ kv('key') }}"} reads its own namespace first, then each
            parent up the dots: the nearest namespace that holds the key wins.
          </SheetDescription>
        </SheetHeader>
        <div className="min-h-0 flex-1 overflow-auto">{body}</div>
      </SheetContent>
    </Sheet>
  );
}
