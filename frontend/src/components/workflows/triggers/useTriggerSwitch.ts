import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { ApiError } from "@/api/client";
import type { SetTriggerDisabledRequest, TriggerEntryDto } from "@/api/generated/model";
import {
  useWorkflowSetTriggerDisabled,
  useWorkflowSetTriggersDisabled,
  type workflowListFlowTriggersResponse,
} from "@/api/generated/workflow/workflow";
import { enableBody, needsEnableDialog, type EnableChoice, type TriggerRow } from "@/components/workflows/triggers/triggersModel";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

/** The enable dialog's question, while it is open: the triggers to switch on and the schedules it asks about. */
export type EnableQuestion = { rows: TriggerRow[]; schedules: TriggerRow[]; bulk: boolean };

/** A bulk entry the server refused, named with its sentence (spec §3.7). */
export type BulkRefusal = { flow_uuid: string; trigger_id: string; message: string };

const rowKey = (r: Pick<TriggerRow, "flow_uuid" | "trigger_id">) => `${r.flow_uuid}/${r.trigger_id}`;

/** What a failed switch says (FE-ACTION-3, FE-COPY-2): the server's sentence, or what to do. */
export function switchError(e: unknown): string {
  if (e instanceof ApiError && e.status === 404) return "This trigger no longer exists in the flow; reload.";
  if (e instanceof ApiError && e.message) return e.message;
  return "the request failed. Check your connection and try again.";
}

const triggerCount = (n: number) => (n === 1 ? "1 trigger" : `${n} triggers`);

/**
 * The runtime switch, one trigger or a selection (spec §4.1, §4.3).
 *
 * The switches show the server's state only: nothing here changes a row until
 * the server answers, and then the answer is written into the flow's cached
 * triggers (so the switch moves with the answer, not with the refetch) and
 * both trigger queries are invalidated (FE-ACTION-2). A refusal is the
 * server's sentence in a toast, and the switch stays where it was.
 *
 * Switching on a schedule that `needsEnableDialog` opens the question first;
 * `answer` sends the choice, `dismiss` sends nothing.
 */
export function useTriggerSwitch(ws: string) {
  const queryClient = useQueryClient();
  const one = useWorkflowSetTriggerDisabled();
  const many = useWorkflowSetTriggersDisabled();
  const [busy, setBusy] = useState<ReadonlySet<string>>(new Set());
  const [question, setQuestion] = useState<EnableQuestion | null>(null);
  const [answering, setAnswering] = useState(false);
  const [refusals, setRefusals] = useState<BulkRefusal[]>([]);

  const mark = (rows: TriggerRow[], on: boolean) =>
    setBusy((prev) => {
      const next = new Set(prev);
      for (const r of rows) {
        if (on) next.add(rowKey(r));
        else next.delete(rowKey(r));
      }
      return next;
    });

  /** Write the server's entries into the flow's cached triggers, then refresh both lists. */
  const adopt = (entries: { flow_uuid: string; entry: TriggerEntryDto }[]) => {
    const flows = new Set(entries.map((e) => e.flow_uuid));
    for (const flowUuid of flows) {
      queryClient.setQueryData<workflowListFlowTriggersResponse>(workflowsKeys.flowTriggers(flowUuid), (old) => {
        if (!old || old.status !== 200) return old;
        const fresh = new Map(entries.filter((e) => e.flow_uuid === flowUuid).map((e) => [e.entry.trigger_id, e.entry]));
        return { ...old, data: { ...old.data, triggers: old.data.triggers.map((t) => fresh.get(t.trigger_id) ?? t) } };
      });
      void queryClient.invalidateQueries({ queryKey: workflowsKeys.flowTriggers(flowUuid) });
    }
    void queryClient.invalidateQueries({ queryKey: workflowsKeys.triggers(ws) });
  };

  const sendOne = async (row: TriggerRow, body: SetTriggerDisabledRequest): Promise<boolean> => {
    mark([row], true);
    try {
      const res = await one.mutateAsync({ workspace: ws, id: row.flow_uuid, triggerId: row.trigger_id, data: body });
      if (res.status !== 200) throw new Error("unexpected answer");
      adopt([{ flow_uuid: row.flow_uuid, entry: res.data }]);
      toast.success(`Switched ${row.trigger_id} ${body.disabled ? "off" : "on"}.`);
      return true;
    } catch (e) {
      toast.error(`${row.trigger_id} was not switched ${body.disabled ? "off" : "on"}: ${switchError(e)}`);
      return false;
    } finally {
      mark([row], false);
    }
  };

  const sendMany = async (rows: TriggerRow[], body: SetTriggerDisabledRequest): Promise<boolean> => {
    mark(rows, true);
    setRefusals([]);
    try {
      const res = await many.mutateAsync({
        workspace: ws,
        data: {
          triggers: rows.map((r) => ({ flow_id: r.flow_uuid, trigger_id: r.trigger_id })),
          disabled: body.disabled,
          recover_missed: body.recover_missed ?? false,
        },
      });
      if (res.status !== 200) throw new Error("unexpected answer");
      const results = res.data.results;
      adopt(results.flatMap((r) => (r.ok && r.trigger ? [{ flow_uuid: r.flow_id, entry: r.trigger }] : [])));
      const switched = results.filter((r) => r.ok).length;
      setRefusals(
        results
          .filter((r) => !r.ok)
          .map((r) => ({ flow_uuid: r.flow_id, trigger_id: r.trigger_id, message: r.message ?? r.code ?? "refused" })),
      );
      if (switched > 0) toast.success(`Switched ${triggerCount(switched)} ${body.disabled ? "off" : "on"}.`);
      return true;
    } catch (e) {
      toast.error(`The triggers were not switched ${body.disabled ? "off" : "on"}: ${switchError(e)}`);
      return false;
    } finally {
      mark(rows, false);
    }
  };

  /** The switch of one row: off at once; on at once, or after the question for an overdue schedule. */
  const toggle = (row: TriggerRow, on: boolean) => {
    if (!on) void sendOne(row, { disabled: true, recover_missed: false });
    else if (needsEnableDialog(row, Date.now())) setQuestion({ rows: [row], schedules: [row], bulk: false });
    else void sendOne(row, enableBody("skip"));
  };

  /** Enable or Disable over a selection, through the bulk route. */
  const switchMany = (rows: TriggerRow[], on: boolean) => {
    if (rows.length === 0) return;
    if (!on) {
      void sendMany(rows, { disabled: true, recover_missed: false });
      return;
    }
    const now = Date.now();
    const schedules = rows.filter((r) => needsEnableDialog(r, now));
    if (schedules.length > 0) setQuestion({ rows, schedules, bulk: true });
    else void sendMany(rows, enableBody("skip"));
  };

  /** The dialog's Enable: sends the choice; the question closes once the server has answered yes. */
  const answer = async (choice: EnableChoice) => {
    if (!question) return;
    setAnswering(true);
    try {
      const ok = question.bulk ? await sendMany(question.rows, enableBody(choice)) : await sendOne(question.rows[0], enableBody(choice));
      if (ok) setQuestion(null);
    } finally {
      setAnswering(false);
    }
  };

  return {
    isBusy: (row: Pick<TriggerRow, "flow_uuid" | "trigger_id">) => busy.has(rowKey(row)),
    anyBusy: busy.size > 0,
    toggle,
    switchMany,
    question,
    answering,
    answer,
    dismiss: () => setQuestion(null),
    refusals,
    clearRefusals: () => setRefusals([]),
  };
}
