import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import type { BulkResultsDto } from "@/api/generated/model";
import {
  useWorkflowDeleteTriggerBackfill,
  useWorkflowDeleteTriggersBackfill,
  useWorkflowSetTriggerBackfillPaused,
  useWorkflowSetTriggersBackfillPaused,
} from "@/api/generated/workflow/workflow";
import { adoptEntries, switchError, type BulkRefusal } from "@/components/workflows/triggers/useTriggerSwitch";
import { backfillBulkMessage, type TriggerRow } from "@/components/workflows/triggers/triggersModel";

const triggerCount = (n: number) => (n === 1 ? "1 backfill" : `${n} backfills`);

/**
 * The backfill actions of the Triggers views (W7f spec §4.1, §4.4): pause,
 * resume and delete one backfill, or the same over a selection.
 *
 * As the switch does (FE-ACTION-2): nothing moves on a click. The server's
 * answer is written into the flow's cached triggers, and both trigger lists
 * refresh, so the strip changes with the answer. A failure is the server's
 * sentence in a toast and leaves the row as it was. A bulk answer lists its
 * refusals by name (`refusals`), which the caller shows and clears.
 */
export function useBackfill(ws: string, { onBulkAnswered }: { onBulkAnswered?: () => void } = {}) {
  const queryClient = useQueryClient();
  const pauseOne = useWorkflowSetTriggerBackfillPaused();
  const deleteOne = useWorkflowDeleteTriggerBackfill();
  const pauseMany = useWorkflowSetTriggersBackfillPaused();
  const deleteMany = useWorkflowDeleteTriggersBackfill();
  const [busy, setBusy] = useState<ReadonlySet<string>>(new Set());
  const [refusals, setRefusals] = useState<BulkRefusal[]>([]);

  const key = (r: Pick<TriggerRow, "flow_uuid" | "trigger_id">) => `${r.flow_uuid}/${r.trigger_id}`;
  const mark = (rows: TriggerRow[], on: boolean) =>
    setBusy((prev) => {
      const next = new Set(prev);
      for (const r of rows) {
        if (on) next.add(key(r));
        else next.delete(key(r));
      }
      return next;
    });

  const pauseSelf = async (row: TriggerRow, paused: boolean) => {
    mark([row], true);
    try {
      const res = await pauseOne.mutateAsync({ workspace: ws, id: row.flow_uuid, triggerId: row.trigger_id, data: { paused } });
      if (res.status !== 200) throw new Error("unexpected answer");
      adoptEntries(queryClient, ws, [{ flow_uuid: row.flow_uuid, entry: res.data }]);
      toast.success(`${paused ? "Paused" : "Resumed"} the backfill of ${row.trigger_id}.`);
    } catch (e) {
      toast.error(`The backfill of ${row.trigger_id} was not ${paused ? "paused" : "resumed"}: ${switchError(e)}`);
    } finally {
      mark([row], false);
    }
  };

  const deleteSelf = async (row: TriggerRow) => {
    mark([row], true);
    try {
      const res = await deleteOne.mutateAsync({ workspace: ws, id: row.flow_uuid, triggerId: row.trigger_id });
      if (res.status !== 200) throw new Error("unexpected answer");
      adoptEntries(queryClient, ws, [{ flow_uuid: row.flow_uuid, entry: res.data }]);
      toast.success(`Deleted the backfill of ${row.trigger_id}. The regular runs resume.`);
    } catch (e) {
      toast.error(`The backfill of ${row.trigger_id} was not deleted: ${switchError(e)}`);
    } finally {
      mark([row], false);
    }
  };

  /** One bulk request, one transaction per entry on the server; its refusals are kept for the list. */
  const sendMany = async (rows: TriggerRow[], send: () => Promise<{ status: number; data: unknown }>, verb: string) => {
    mark(rows, true);
    setRefusals([]);
    try {
      const res = await send();
      if (res.status !== 200) throw new Error("unexpected answer");
      const results = (res.data as BulkResultsDto).results;
      adoptEntries(
        queryClient,
        ws,
        results.flatMap((r) => (r.ok && r.trigger ? [{ flow_uuid: r.flow_id, entry: r.trigger }] : [])),
      );
      const done = results.filter((r) => r.ok).length;
      setRefusals(
        results
          .filter((r) => !r.ok)
          .map((r) => ({ flow_uuid: r.flow_id, trigger_id: r.trigger_id, message: backfillBulkMessage(r.code, r.message) })),
      );
      if (done > 0) toast.success(`${verb} ${triggerCount(done)}.`);
      onBulkAnswered?.();
    } catch (e) {
      toast.error(`The backfills were not changed: ${switchError(e)}`);
    } finally {
      mark(rows, false);
    }
  };

  return {
    isBusy: (row: Pick<TriggerRow, "flow_uuid" | "trigger_id">) => busy.has(key(row)),
    anyBusy: busy.size > 0,
    pause: (row: TriggerRow, paused: boolean) => void pauseSelf(row, paused),
    remove: (row: TriggerRow) => void deleteSelf(row),
    pauseMany: (rows: TriggerRow[], paused: boolean) =>
      void sendMany(
        rows,
        () =>
          pauseMany.mutateAsync({
            workspace: ws,
            data: { triggers: rows.map((r) => ({ flow_id: r.flow_uuid, trigger_id: r.trigger_id })), paused },
          }),
        paused ? "Paused" : "Resumed",
      ),
    removeMany: (rows: TriggerRow[]) =>
      void sendMany(
        rows,
        () =>
          deleteMany.mutateAsync({
            workspace: ws,
            data: { triggers: rows.map((r) => ({ flow_id: r.flow_uuid, trigger_id: r.trigger_id })) },
          }),
        "Deleted",
      ),
    refusals,
    clearRefusals: () => setRefusals([]),
  };
}
