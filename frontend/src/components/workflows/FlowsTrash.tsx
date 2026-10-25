import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { ArchiveRestore, Trash2 } from "lucide-react";
import {
  useWorkflowListFlows,
  useWorkflowPurgeFlow,
  useWorkflowRestoreFlow,
} from "@/api/generated/workflow/workflow";
import type { FlowListItemDto } from "@/api/generated/model";
import { Button } from "@/components/ui/button";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { ListStates } from "@/components/explore/ListStates";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { SummaryStrip } from "@/components/explore/SummaryStrip";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

const errorMessage = (e: unknown, fallback: string) => (e instanceof Error && e.message ? e.message : fallback);

const nameOf = (f: Pick<FlowListItemDto, "namespace" | "flow_id">) => `${f.namespace}.${f.flow_id}`;

/**
 * The flows in the trash (KI-119): `GET …/flows?trashed=true`, newest first, each with the two
 * ways out — **Restore** puts it back in the flow list as it was, and **Delete permanently**
 * removes it with its versions and frees its namespace and id for a new flow. Its executions are
 * kept either way.
 *
 * A flat table (FE-PAGE-4): the trash has no namespace tree to group by. The actions need
 * `workflow:manage`; the server's refusal is shown as it says it (FE-ACTION-3).
 */
export function FlowsTrash({ ws }: { ws: string }) {
  const queryClient = useQueryClient();
  const params = { trashed: true };
  const query = useWorkflowListFlows(ws, params, { query: { queryKey: workflowsKeys.flows(ws, params) } });
  const flows: FlowListItemDto[] | undefined = query.data?.status === 200 ? query.data.data : undefined;
  const restore = useWorkflowRestoreFlow();
  const purge = useWorkflowPurgeFlow();
  const [pendingPurge, setPendingPurge] = useState<FlowListItemDto | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  // Both lists change: the flow list and the trash share `flows(ws)` as their prefix.
  const refresh = () => void queryClient.invalidateQueries({ queryKey: workflowsKeys.flows(ws) });

  const runRestore = async (f: FlowListItemDto) => {
    setBusy(f.id);
    try {
      await restore.mutateAsync({ workspace: ws, id: f.id });
      refresh();
      // The page stays on the trash, so a toast says where the flow went (FE-ACTION-2).
      toast.success(`Restored ${nameOf(f)} to the flow list`);
    } catch (e) {
      toast.error(errorMessage(e, "The flow could not be restored. Try again."));
    } finally {
      setBusy(null);
    }
  };

  const runPurge = async (f: FlowListItemDto) => {
    setBusy(f.id);
    try {
      await purge.mutateAsync({ workspace: ws, id: f.id });
      queryClient.removeQueries({ queryKey: workflowsKeys.flow(f.id) });
      refresh();
      toast.success(`Deleted ${nameOf(f)} permanently`);
    } catch (e) {
      toast.error(errorMessage(e, "The flow could not be deleted. Try again."));
    } finally {
      setBusy(null);
    }
  };

  return (
    <ListStates
      isLoading={query.isLoading}
      isError={query.isError}
      error={query.error}
      errorText="The trash could not be loaded. Reload the page to try again."
      hasData={!!flows && flows.length > 0}
      skeletonCols={4}
      empty={<p className="text-sm text-muted-foreground">The trash is empty. A deleted flow stays here until it is restored or deleted permanently.</p>}
    >
      <div className="mb-3" data-testid="flows-trash-summary">
        <SummaryStrip items={[{ label: "flows in the trash", singular: "flow in the trash", value: flows?.length ?? 0 }]} />
      </div>
      <Table data-testid="flows-trash">
        <TableHeader>
          <TableRow>
            <TableHead>Flow</TableHead>
            <TableHead>Version</TableHead>
            <TableHead>Deleted</TableHead>
            <TableHead>
              <span className="sr-only">Actions</span>
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {(flows ?? []).map((f) => (
            <TableRow key={f.id}>
              <TableCell className="font-mono text-xs">{nameOf(f)}</TableCell>
              <TableCell className="tabular-nums text-muted-foreground">{f.latest_version != null ? `v${f.latest_version}` : "Draft"}</TableCell>
              <TableCell className="text-muted-foreground">{f.deleted_at ? <RelativeTime iso={f.deleted_at} /> : "—"}</TableCell>
              <TableCell className="text-right">
                <span className="inline-flex gap-2">
                  <Button
                    type="button"
                    size="sm"
                    variant="outline"
                    aria-label={`Restore ${nameOf(f)}`}
                    aria-disabled={busy === f.id ? true : undefined}
                    onClick={() => {
                      if (busy !== f.id) void runRestore(f);
                    }}
                  >
                    <ArchiveRestore aria-hidden />
                    Restore
                  </Button>
                  <Button
                    type="button"
                    size="sm"
                    variant="ghost"
                    aria-label={`Delete ${nameOf(f)} permanently`}
                    aria-disabled={busy === f.id ? true : undefined}
                    onClick={() => {
                      if (busy !== f.id) setPendingPurge(f);
                    }}
                  >
                    <Trash2 aria-hidden />
                    Delete permanently
                  </Button>
                </span>
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
      <ConfirmDeleteDialog
        open={pendingPurge !== null}
        onOpenChange={(o) => {
          if (!o) setPendingPurge(null);
        }}
        title={`Delete ${pendingPurge ? nameOf(pendingPurge) : ""} permanently?`}
        description="The flow, its versions and its trigger state are deleted, and its namespace and id can be used by a new flow. Its executions are kept. This cannot be undone."
        onConfirm={async () => {
          const f = pendingPurge;
          setPendingPurge(null);
          if (f) await runPurge(f);
        }}
      />
    </ListStates>
  );
}
