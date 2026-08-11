import { useScheduleRuns, type ScheduleSummary } from "@/api/alerts";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { StateBadge } from "@/routes/explore/Alerts";

/**
 * Recent runs for one schedule.
 *
 * This is the first thing anyone opens when an alert "didn't work", which is
 * why it ships with the model rather than after the engine: without it, a
 * schedule that is silently erroring is indistinguishable from one whose
 * condition is simply never true.
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
  const { data, isLoading } = useScheduleRuns(ws, schedule.id);
  const runs = data ?? [];

  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="max-h-[80vh] max-w-2xl overflow-y-auto">
        <DialogHeader>
          <DialogTitle>Run log — {schedule.name}</DialogTitle>
        </DialogHeader>

        {isLoading ? (
          <ListSkeleton />
        ) : runs.length === 0 ? (
          <p className="py-8 text-center text-sm text-muted-foreground">
            This schedule has not run yet. The scheduler that executes it is not deployed
            in this version.
          </p>
        ) : (
          <table className="w-full text-sm">
            <thead className="border-b text-left text-muted-foreground">
              <tr>
                <th className="py-2 font-medium">State</th>
                <th className="py-2 font-medium">Started</th>
                <th className="py-2 font-medium">Value</th>
                <th className="py-2 font-medium">Detail</th>
              </tr>
            </thead>
            <tbody>
              {runs.map((r) => (
                <tr key={r.id} className="border-b last:border-0 align-top">
                  <td className="py-2">
                    <StateBadge state={r.state} />
                  </td>
                  <td className="py-2 text-muted-foreground">
                    <RelativeTime iso={r.started_at} />
                  </td>
                  <td className="py-2">{r.value ?? "—"}</td>
                  <td className="py-2 text-muted-foreground">
                    {r.error ? (
                      <span className="text-destructive">{r.error}</span>
                    ) : r.notification_sent_at ? (
                      "Notification sent"
                    ) : (
                      "—"
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </DialogContent>
    </Dialog>
  );
}
