import { useQuery } from "@tanstack/react-query";
import { Copy, Pencil, RotateCw } from "lucide-react";
import type { KvEntryDto } from "@/api/generated/model";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { Skeleton } from "@/components/ui/skeleton";
import { DisabledReasons, ReasonButton } from "@/components/workflows/ReasonButton";
import { workflowsKeys } from "@/routes/workflows/queryKeys";
import { kvErrorMessage, readKvValue } from "./kvApi";
import { MANAGE_REASON } from "./kvCopy";
import { formatInstant } from "./KvTable";
import { kvExpression, valueForDisplay } from "./kvValue";

/**
 * One key's latest value (W7d spec §4.4, "View the value"), read when the
 * drawer opens, every digit as the server keeps it, with its type,
 * description, revision and expiry. Copy and Edit are here too; Edit says why
 * when it is off (FE-ACTION-4).
 */
export function KvValueDrawer({
  ws,
  namespace,
  entry,
  canManage,
  onEdit,
  onCopy,
  onClose,
}: {
  ws: string;
  namespace: string;
  entry: KvEntryDto;
  canManage: boolean;
  onEdit: () => void;
  onCopy: () => void;
  onClose: () => void;
}) {
  const query = useQuery({
    queryKey: workflowsKeys.kvValue(ws, namespace, entry.key),
    queryFn: () => readKvValue(ws, namespace, entry.key),
  });
  const detail = query.data;
  const edit = canManage ? ({ ok: true } as const) : ({ ok: false, reason: MANAGE_REASON } as const);

  return (
    <Sheet open onOpenChange={(open) => (open ? undefined : onClose())}>
      <SheetContent className="max-w-xl">
        <SheetHeader>
          <SheetTitle className="break-all font-mono">{entry.key}</SheetTitle>
          <SheetDescription>
            {detail ? `Revision ${detail.revision}, written ${formatInstant(detail.updated)}.` : `A key of ${namespace}.`}
          </SheetDescription>
        </SheetHeader>
        <div className="min-h-0 flex-1 overflow-auto">
          {query.isError ? (
            <Alert variant="destructive">
              <AlertDescription className="flex flex-wrap items-center gap-3">
                <span>{kvErrorMessage(query.error, "The value could not be read")}</span>
                <Button variant="outline" size="sm" onClick={() => void query.refetch()}>
                  <RotateCw aria-hidden />
                  Retry
                </Button>
              </AlertDescription>
            </Alert>
          ) : !detail ? (
            <div className="grid gap-3" aria-hidden>
              <Skeleton className="h-6 w-24" />
              <Skeleton className="h-32 w-full" />
            </div>
          ) : (
            <dl className="grid gap-4 text-sm">
              <div className="grid gap-1">
                <dt className="text-muted-foreground">Type</dt>
                <dd>
                  <Badge variant="outline">{detail.type}</Badge>
                </dd>
              </div>
              <div className="grid gap-1">
                <dt id="kv-view-value" className="text-muted-foreground">
                  Value
                </dt>
                <dd>
                  <pre
                    aria-labelledby="kv-view-value"
                    className="max-h-96 overflow-auto whitespace-pre-wrap break-all rounded-md border bg-muted p-3 font-mono text-xs"
                  >
                    {valueForDisplay(detail.type, detail.value)}
                  </pre>
                </dd>
              </div>
              <div className="grid gap-1">
                <dt className="text-muted-foreground">Description</dt>
                <dd>{detail.description || "—"}</dd>
              </div>
              <div className="grid gap-1">
                <dt className="text-muted-foreground">Expiration</dt>
                <dd className="tabular-nums">{detail.expiresAt ? formatInstant(detail.expiresAt) : "Never"}</dd>
              </div>
              <div className="grid gap-1">
                <dt className="text-muted-foreground">Read in a flow with</dt>
                <dd className="font-mono text-xs">{kvExpression(entry.key)}</dd>
              </div>
            </dl>
          )}
        </div>
        <div className="flex shrink-0 flex-col items-end gap-1">
          <div className="flex gap-2">
            <Button type="button" size="sm" variant="outline" onClick={onCopy}>
              <Copy aria-hidden />
              Copy expression
            </Button>
            <ReasonButton enablement={edit} reasonId="kv-view-edit-reason" onClick={onEdit}>
              <Pencil aria-hidden />
              Edit
            </ReasonButton>
          </div>
          <DisabledReasons actions={[{ id: "kv-view-edit-reason", label: "Edit", enablement: edit }]} />
        </div>
      </SheetContent>
    </Sheet>
  );
}
