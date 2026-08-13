import { useScheduleRuns } from "@/api/alerts";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { StateBadge } from "@/routes/explore/Alerts";

/**
 * Recent runs for one schedule, as a table.
 *
 * Extracted from `RunLogDrawer` so the detail PAGE can show the same rows
 * without a dialog. The page needs them for a reason the list does not: its
 * header carries a state badge, and an "Error" badge with no way to see the
 * error on the same screen asks a question it cannot answer.
 *
 * `limit` exists because the page shows a recent slice while the drawer shows
 * the log — the same rows, a different amount of them.
 */
export function RunLogTable({
  ws,
  scheduleId,
  limit,
}: {
  ws: string;
  scheduleId: string;
  limit?: number;
}) {
  const { data, isLoading } = useScheduleRuns(ws, scheduleId);
  const all = data ?? [];
  const runs = limit ? all.slice(0, limit) : all;

  if (isLoading) return <ListSkeleton />;
  if (runs.length === 0) {
    return (
      <p className="py-8 text-center text-sm text-muted-foreground">
        This schedule has not run yet.
      </p>
    );
  }

  return (
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
          <tr key={r.id} className="border-b align-top last:border-0">
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
  );
}
