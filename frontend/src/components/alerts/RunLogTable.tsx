import { useScheduleRuns, type ScheduleType } from "@/api/alerts";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
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
 *
 * A report's run measures nothing, so its log has no Value column (KI-171):
 * the column was always "—" and squeezed its heading into Detail's.
 */
export function RunLogTable({
  ws,
  scheduleId,
  type,
  limit,
}: {
  ws: string;
  scheduleId: string;
  type: ScheduleType;
  limit?: number;
}) {
  const hasValue = type !== "report";
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
    <Table>
      <TableHeader>
        <TableRow className="hover:bg-transparent">
          <TableHead className="w-28 font-medium text-muted-foreground">State</TableHead>
          <TableHead className="w-32 font-medium text-muted-foreground">Started</TableHead>
          {hasValue && (
            <TableHead className="w-24 font-medium text-muted-foreground">Value</TableHead>
          )}
          <TableHead className="font-medium text-muted-foreground">Detail</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {runs.map((r) => (
          <TableRow key={r.id} className="align-top">
            <TableCell className="py-2">
              <StateBadge state={r.state} />
            </TableCell>
            <TableCell className="py-2 text-muted-foreground">
              <RelativeTime iso={r.started_at} />
            </TableCell>
            {hasValue && <TableCell className="py-2 tabular-nums">{r.value ?? "—"}</TableCell>}
            {/* Detail is the one column that wraps: it carries the server's
                error message verbatim, which can be long. */}
            <TableCell className="whitespace-normal break-words py-2 text-muted-foreground">
              {r.error ? (
                <span className="text-destructive">{r.error}</span>
              ) : r.notification_sent_at ? (
                "Notification sent"
              ) : (
                "—"
              )}
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
