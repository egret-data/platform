import { useId, useState } from "react";
import { Pause, Play, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Progress } from "@/components/ui/progress";
import { TableCell, TableRow } from "@/components/ui/table";
import { DisabledReasons, ReasonButton } from "@/components/workflows/ReasonButton";
import type { Enablement } from "@/components/workflows/draftState";
import {
  backfillFrozen,
  backfillRefusalText,
  backfillStatus,
  backfillWindowText,
  MANAGE_REASON,
  ticksText,
  type TriggerRow,
} from "@/components/workflows/triggers/triggersModel";
import { StatusBadge } from "@/components/ui/status-badge";

/**
 * Whether the Backfill button of a trigger without one can be used, and why not
 * (FE-ACTION-4). The caller's `workflow:manage` is checked first, as the server
 * does at its gate; then the server's own refusal.
 */
export function backfillEnablement(row: TriggerRow, manage: boolean): Enablement {
  if (!manage) return { ok: false, reason: MANAGE_REASON };
  if (row.can_backfill) return { ok: true };
  return { ok: false, reason: row.backfill_refusal ? backfillRefusalText(row.backfill_refusal, row.schedule?.last_error) : "This trigger cannot be backfilled" };
}

/**
 * The Backfill column's cell (spec §4.1): "Backfill" with its reason when the
 * trigger can take none, "Running" or "Paused" with an icon when one exists;
 * empty for a trigger that is not a schedule. A row whose cell has no
 * `onBackfill` (the create form is not open here) shows a dash for "none".
 */
export function BackfillCell({
  row,
  manage,
  onBackfill,
}: {
  row: TriggerRow;
  manage: boolean;
  onBackfill?: (row: TriggerRow) => void;
}) {
  const reasonId = useId();
  if (row.kind !== "schedule") return null;
  const b = row.backfill;
  if (b) {
    const Icon = b.paused ? Pause : Play;
    return (
      <span className="inline-flex items-center gap-1.5 text-sm">
        <Icon aria-hidden className="h-4 w-4 text-muted-foreground" />
        {b.paused ? "Paused" : "Running"}
      </span>
    );
  }
  if (!onBackfill) return <span className="text-muted-foreground">—</span>;
  const enablement = backfillEnablement(row, manage);
  return (
    <span className="inline-flex flex-wrap items-center gap-2">
      <Button
        type="button"
        size="sm"
        variant="outline"
        disabled={!enablement.ok}
        title={enablement.ok ? undefined : enablement.reason}
        aria-describedby={enablement.ok ? undefined : reasonId}
        onClick={() => onBackfill(row)}
      >
        Backfill
      </Button>
      {enablement.ok ? null : (
        <span id={reasonId} className="text-xs text-muted-foreground">
          {enablement.reason}
        </span>
      )}
    </span>
  );
}

/**
 * The sub-row under a trigger that has a backfill (spec §4.1, §4.2): the window,
 * the progress bar, the status in words, Pause or Resume, and Delete, which
 * asks first (FE-ACTION-1). The strip shows the server's state; it moves when the
 * answer arrives.
 */
export function BackfillStrip({
  row,
  colSpan,
  name,
  manage,
  busy,
  onPause,
  onDelete,
}: {
  row: TriggerRow;
  /** The table's column count, so the strip spans the row. */
  colSpan: number;
  /** How the row is named to a screen reader: the trigger's id, with its flow on the global page. */
  name: string;
  manage: boolean;
  busy: boolean;
  onPause: (paused: boolean) => void;
  onDelete: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const reasonsId = useId();
  const b = row.backfill;
  if (!b) return null;
  const frozen = backfillFrozen(row);
  const manageEnablement: Enablement = manage ? { ok: true } : { ok: false, reason: MANAGE_REASON };
  const remaining = b.total_ticks - b.ticks_done;

  return (
    <TableRow data-testid="backfill-strip" className="bg-muted/30 hover:bg-muted/30">
      <TableCell colSpan={colSpan}>
        <div className="grid gap-2 py-1 md:grid-cols-[minmax(0,1fr)_minmax(0,14rem)_auto] md:items-center md:gap-4">
          <div className="grid gap-0.5 text-sm">
            <span className="font-medium">{backfillWindowText(b)}</span>
            <span className="text-muted-foreground tabular-nums">
              {b.progress}% · {ticksText(b)}
            </span>
          </div>
          <div className="grid gap-1">
            <Progress value={b.progress} aria-label={`Backfill progress of ${name}`} className="h-2" />
            <span className="text-xs text-muted-foreground">
              {b.paused ? <StatusBadge tone="highlight">Paused</StatusBadge> : backfillStatus(b)}
              {frozen ? <span className="ml-2">Frozen while the trigger is off</span> : null}
            </span>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <ReasonButton
              enablement={manageEnablement}
              reasonId={`${reasonsId}-pause`}
              variant="outline"
              busy={busy}
              onClick={() => onPause(!b.paused)}
            >
              {b.paused ? <Play aria-hidden /> : <Pause aria-hidden />}
              {b.paused ? "Resume" : "Pause"}
            </ReasonButton>
            <ReasonButton
              enablement={manageEnablement}
              reasonId={`${reasonsId}-delete`}
              variant="outline"
              busy={busy}
              onClick={() => setConfirming(true)}
            >
              <Trash2 aria-hidden />
              Delete
            </ReasonButton>
          </div>
        </div>
        <DisabledReasons
          actions={[
            { id: `${reasonsId}-pause`, label: b.paused ? "Resume" : "Pause", enablement: manageEnablement },
            { id: `${reasonsId}-delete`, label: "Delete", enablement: manageEnablement },
          ]}
        />
        <AlertDialog open={confirming} onOpenChange={setConfirming}>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>Delete this backfill?</AlertDialogTitle>
              <AlertDialogDescription>
                Its remaining {remaining} {remaining === 1 ? "tick" : "ticks"} will not run. Executions already started stay.
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>Cancel</AlertDialogCancel>
              <AlertDialogAction
                onClick={() => {
                  setConfirming(false);
                  onDelete();
                }}
              >
                Delete backfill
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </TableCell>
    </TableRow>
  );
}
