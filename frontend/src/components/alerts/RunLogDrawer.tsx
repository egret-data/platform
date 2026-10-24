import { type ScheduleSummary, type ScheduleType } from "@/api/alerts";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { RunLogTable } from "./RunLogTable";

/**
 * Recent runs for one schedule, from the LIST.
 *
 * This is the first thing anyone opens when an alert "didn't work", which is
 * why it ships with the model rather than after the engine: without it, a
 * schedule that is silently erroring is indistinguishable from one whose
 * condition is simply never true.
 *
 * The rows themselves live in `RunLogTable`, which the detail page also
 * renders — the list opens them over the table it came from, the page shows a
 * recent slice under the form.
 */
export function RunLogDrawer({
  ws,
  schedule,
  onClose,
}: {
  ws: string;
  schedule: ScheduleSummary;
  onClose: () => void;
}) {
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="max-h-[80vh] max-w-2xl overflow-y-auto">
        <DialogHeader>
          <DialogTitle>Run log — {schedule.name}</DialogTitle>
        </DialogHeader>
        <RunLogTable ws={ws} scheduleId={schedule.id} type={schedule.type as ScheduleType} />
      </DialogContent>
    </Dialog>
  );
}
