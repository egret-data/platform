import { useId, useState } from "react";
import { Loader2 } from "lucide-react";
import type { TriggerEntryDto } from "@/api/generated/model";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { missedLabel, recoverDescription, type EnableChoice } from "@/components/workflows/triggers/triggersModel";
import { fullTimestamp } from "@/lib/relativeTime";

/**
 * The missed ticks of the schedules asked about, summed. Each count is capped
 * at 1,000 by the server, so the sum is a floor whenever one of them is.
 */
function missedOf(schedules: TriggerEntryDto[]): string {
  if (schedules.length === 1) return missedLabel(schedules[0].schedule?.missed_ticks);
  const sum = schedules.reduce((n, s) => n + (s.schedule?.missed_ticks?.count ?? 0), 0);
  return schedules.some((s) => s.schedule?.missed_ticks?.more) ? `${sum}+` : String(sum);
}

/**
 * What "Recover missed runs" does, per the triggers' `recoverMissedSchedules`:
 * one sentence for one schedule or for several that read it the same way,
 * else one line per trigger.
 */
function recoverLines(schedules: TriggerEntryDto[]): string[] {
  const descriptions = schedules.map((s) => recoverDescription(s.schedule?.recover_missed_schedules));
  if (schedules.length === 1) return [`${schedules[0].trigger_id} ${descriptions[0]}.`];
  if (new Set(descriptions).size === 1) return [`Each ${descriptions[0]}.`];
  return schedules.map((s, i) => `${s.trigger_id} ${descriptions[i]}.`);
}

/**
 * Switching on a schedule whose next tick has passed (spec §4.3): skip the
 * missed ticks (Kestra's default) or recover them as the trigger's
 * `recoverMissedSchedules` says. For one trigger, or for a selection, whose
 * other triggers switch on with the same request.
 *
 * "Skip" names no time: egret has no cron library in the browser, and the
 * spec's rule is no new dependency (§8), so the next tick is not computed here.
 */
export function EnableTriggerDialog({
  open,
  count,
  bulk,
  schedules,
  busy,
  onCancel,
  onEnable,
}: {
  open: boolean;
  /** How many triggers the Enable switches on. */
  count: number;
  bulk: boolean;
  /** The overdue schedules the question is about. */
  schedules: TriggerEntryDto[];
  busy: boolean;
  onCancel: () => void;
  onEnable: (choice: EnableChoice) => void;
}) {
  const [choice, setChoice] = useState<EnableChoice>("skip");
  const id = useId();
  const lines = recoverLines(schedules);
  const title = bulk ? (count === 1 ? "Enable 1 trigger" : `Enable ${count} triggers`) : "Enable trigger";
  const subject = bulk
    ? schedules.length === 1
      ? `${schedules[0].trigger_id} has missed runs while it was switched off.`
      : `${schedules.length} schedules have missed runs while they were switched off.`
    : `${schedules[0]?.trigger_id ?? "This trigger"} has missed runs while it was switched off.`;

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (!o && !busy) {
          setChoice("skip");
          onCancel();
        }
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>{subject}</DialogDescription>
        </DialogHeader>
        <RadioGroup value={choice} onValueChange={(v) => setChoice(v as EnableChoice)} aria-label="Missed runs">
          <div className="flex items-start gap-2">
            <RadioGroupItem id={`${id}-skip`} value="skip" className="mt-0.5" />
            <Label htmlFor={`${id}-skip`} className="font-normal">
              Skip missed runs; start at the next tick
            </Label>
          </div>
          <div className="flex items-start gap-2">
            <RadioGroupItem id={`${id}-recover`} value="recover" className="mt-0.5" aria-describedby={`${id}-recover-desc`} />
            <div className="grid gap-1">
              <Label htmlFor={`${id}-recover`} className="font-normal">
                {/* One inline run: ui/label is `flex gap-2`, so loose text and a count span would each get a gap. */}
                <span>
                  Recover missed runs (<span className="tabular-nums">{missedOf(schedules)}</span> missed)
                </span>
              </Label>
              <div id={`${id}-recover-desc`} className="text-sm text-muted-foreground">
                {lines.length === 1 ? (
                  <p>{lines[0]}</p>
                ) : (
                  <ul className="list-disc pl-4" role="list">
                    {lines.map((l) => (
                      <li key={l}>{l}</li>
                    ))}
                  </ul>
                )}
              </div>
            </div>
          </div>
        </RadioGroup>
        {schedules.flatMap((s) =>
          s.backfill ? (
            <p key={s.trigger_id} className="text-sm text-muted-foreground">
              The backfill of {s.trigger_id} resumes at {fullTimestamp(s.backfill.current_tick)}. This choice is about the regular runs that
              fell due.
            </p>
          ) : [],
        )}
        <DialogFooter>
          <Button
            type="button"
            variant="outline"
            disabled={busy}
            onClick={() => {
              setChoice("skip");
              onCancel();
            }}
          >
            Cancel
          </Button>
          <Button type="button" disabled={busy} onClick={() => onEnable(choice)}>
            {busy && <Loader2 aria-hidden className="animate-spin" />}
            Enable
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
