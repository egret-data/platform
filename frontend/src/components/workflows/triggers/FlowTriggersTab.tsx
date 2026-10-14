import { useId, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { Power, PowerOff, RotateCw, X } from "lucide-react";
import { ApiError } from "@/api/client";
import { useWorkflowListFlowTriggers } from "@/api/generated/workflow/workflow";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import type { Enablement } from "@/components/workflows/draftState";
import { DisabledReasons, ReasonButton } from "@/components/workflows/ReasonButton";
import { EnableTriggerDialog } from "@/components/workflows/triggers/EnableTriggerDialog";
import { TriggerBanners } from "@/components/workflows/triggers/TriggerBanners";
import { TriggerDetailsDrawer, triggerBlock } from "@/components/workflows/triggers/TriggerDetailsDrawer";
import { selectionKey, TriggersTable } from "@/components/workflows/triggers/TriggersTable";
import { bannersFor, MANAGE_REASON, type TriggerRow } from "@/components/workflows/triggers/triggersModel";
import { useTriggerSwitch } from "@/components/workflows/triggers/useTriggerSwitch";

import { workflowsKeys } from "@/routes/workflows/queryKeys";

/** A failed read, in words (FE-COPY-2): the server's sentence, ending once, or what to do. */
const readError = (e: unknown) =>
  e instanceof ApiError && e.message ? `${e.message.replace(/\.+$/, "")}.` : "the request failed. Check your connection and try again.";

/** How often the tab reads the triggers again while it is shown (spec §4.1). */
export const TRIGGERS_POLL_MS = 30_000;

/**
 * The flow's Triggers tab (W7e spec §4.1): every declared trigger, its state
 * in words, its runtime switch, the bulk Enable and Disable over a selection,
 * and the details drawer (`?trigger=<id>`, §4.2).
 *
 * The tab's content is unmounted while another tab is shown (`ui/tabs`), so
 * the 30-second refresh runs only while it is visible, and not while the
 * browser tab is hidden (`refetchIntervalInBackground: false`).
 */
export default function FlowTriggersTab({
  ws,
  flowId,
  source,
  manage,
  onEditTrigger,
}: {
  ws: string;
  /** The flow's uuid. */
  flowId: string;
  /** The saved draft, which the triggers were read from: the drawer cuts each trigger's YAML from it. */
  source: string;
  /** `workflow:manage` (the flow's `can_manage`). */
  manage: boolean;
  /** Open the editor at this trigger's YAML. */
  onEditTrigger?: (triggerId: string) => void;
}) {
  const base = `/explore/${ws}/workflows`;
  const query = useWorkflowListFlowTriggers(ws, flowId, {
    query: {
      queryKey: workflowsKeys.flowTriggers(flowId),
      refetchInterval: TRIGGERS_POLL_MS,
      refetchIntervalInBackground: false,
    },
  });
  const page = query.data?.status === 200 ? query.data.data : undefined;
  const [selection, setSelection] = useState<Set<string>>(() => new Set());
  // A bulk action that the server answered clears the selection; its refusals are listed by name.
  const sw = useTriggerSwitch(ws, { onBulkAnswered: () => setSelection(new Set()) });
  const [params, setParams] = useSearchParams();
  const reasonsId = useId();
  const openId = params.get("trigger");

  // Error first (FE-STATE-1), while nothing has loaded; a failed refresh keeps the rows below.
  if (query.isError && !page) {
    return (
      <Alert variant="destructive">
        <AlertDescription className="flex flex-wrap items-center gap-3">
          <span>The triggers could not be loaded: {readError(query.error)}</span>
          <Button variant="outline" size="sm" onClick={() => void query.refetch()}>
            <RotateCw aria-hidden />
            Retry
          </Button>
        </AlertDescription>
      </Alert>
    );
  }
  if (!page) return <ListSkeleton cols={9} />;

  if (page.triggers.length === 0) {
    return (
      <EmptyState
        variant="dashed"
        title="This flow has no triggers"
        description="Add a trigger under triggers: in the flow's YAML, then save."
        action={
          <Button asChild variant="outline" size="sm">
            <Link to={`${base}/flows/${encodeURIComponent(flowId)}/edit`}>Open the editor</Link>
          </Button>
        }
      />
    );
  }

  const rows: TriggerRow[] = page.triggers.map((t) => ({ ...t, flow_uuid: flowId }));
  const ctx = { flowDisabled: page.flow_disabled, flowDeleted: page.flow_deleted };
  // A trigger id may repeat (`duplicate_id`); the switch is per id, so it is selected once.
  const selected = rows.filter((r, i) => selection.has(selectionKey(r)) && rows.findIndex((o) => o.trigger_id === r.trigger_id) === i);
  const bulk: Enablement = !manage
    ? { ok: false, reason: MANAGE_REASON }
    : selected.length === 0
      ? { ok: false, reason: "Select triggers first" }
      : { ok: true };

  const detailsHref = (row: TriggerRow) => {
    const next = new URLSearchParams(params);
    next.set("trigger", row.trigger_id);
    return `?${next.toString()}`;
  };
  const closeDrawer = () =>
    setParams((prev) => {
      const next = new URLSearchParams(prev);
      next.delete("trigger");
      return next;
    });
  const openEntry = openId === null ? undefined : page.triggers.find((t) => t.trigger_id === openId);

  return (
    <div className="grid gap-3">
      <TriggerBanners banners={bannersFor(page)} />
      {query.isError ? (
        <Alert variant="destructive">
          <AlertDescription>The triggers could not be refreshed: {readError(query.error)} What is shown is the last answer.</AlertDescription>
        </Alert>
      ) : null}
      <div className="flex flex-col items-start gap-1">
        <div className="flex flex-wrap items-center gap-2">
          <ReasonButton enablement={bulk} reasonId={`${reasonsId}-enable`} variant="outline" busy={sw.anyBusy} onClick={() => sw.switchMany(selected, true)}>
            <Power aria-hidden />
            Enable
          </ReasonButton>
          <ReasonButton enablement={bulk} reasonId={`${reasonsId}-disable`} variant="outline" busy={sw.anyBusy} onClick={() => sw.switchMany(selected, false)}>
            <PowerOff aria-hidden />
            Disable
          </ReasonButton>
          {selected.length > 0 ? (
            <span className="text-sm text-muted-foreground tabular-nums">
              {selected.length === 1 ? "1 trigger selected" : `${selected.length} triggers selected`}
            </span>
          ) : null}
        </div>
        <DisabledReasons
          actions={[
            { id: `${reasonsId}-enable`, label: "Enable", enablement: bulk },
            { id: `${reasonsId}-disable`, label: "Disable", enablement: bulk },
          ]}
        />
      </div>
      {sw.refusals.length > 0 ? (
        <Alert variant="destructive" data-testid="bulk-refusals">
          <AlertDescription className="grid gap-1">
            <div className="flex items-start justify-between gap-2">
              <p>
                {sw.refusals.length === 1 ? "1 trigger was not switched:" : `${sw.refusals.length} triggers were not switched:`}
              </p>
              <Button type="button" size="iconSm" variant="ghost" aria-label="Dismiss" onClick={sw.clearRefusals}>
                <X aria-hidden />
              </Button>
            </div>
            <ul className="list-disc pl-5" role="list">
              {sw.refusals.map((r) => (
                <li key={`${r.flow_uuid}/${r.trigger_id}`}>
                  <span className="font-mono">{r.trigger_id}</span>: {r.message}
                </li>
              ))}
            </ul>
          </AlertDescription>
        </Alert>
      ) : null}
      <TriggersTable
        rows={rows}
        variant="flow"
        base={base}
        manage={manage}
        selection={selection}
        onSelectionChange={setSelection}
        onToggle={sw.toggle}
        isBusy={sw.isBusy}
        contextOf={() => ctx}
        detailsHref={detailsHref}
      />
      <TriggerDetailsDrawer
        key={openId ?? ""}
        open={openId !== null}
        onClose={closeDrawer}
        triggerId={openId ?? ""}
        entry={openEntry}
        ctx={ctx}
        webhooksEnabled={page.webhooks_enabled}
        block={openId === null ? null : triggerBlock(source, openId)}
        base={base}
        onEditInEditor={onEditTrigger && openId !== null ? () => onEditTrigger(openId) : undefined}
      />
      <EnableTriggerDialog
        key={sw.question ? sw.question.rows.map(selectionKey).join(",") : "closed"}
        open={sw.question !== null}
        count={sw.question?.rows.length ?? 0}
        bulk={sw.question?.bulk ?? false}
        schedules={sw.question?.schedules ?? []}
        busy={sw.answering}
        onCancel={sw.dismiss}
        onEnable={(choice) => void sw.answer(choice)}
      />
    </div>
  );
}
