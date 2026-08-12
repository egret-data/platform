import { DropdownMenuItem, DropdownMenuSeparator } from "@/components/ui/dropdown-menu";
import { RowActionsMenu } from "@/components/explore/RowActionsMenu";
import type { ScheduleSummary } from "@/api/alerts";

/**
 * The row's actions, in one `…` menu — the same shape as `ChartActionsMenu`.
 *
 * Four inline text buttons per row (what this replaces) cost ~380px of every
 * row and put Delete one mis-click from Run now. Collapsing them also lets the
 * columns that answer "is this thing healthy?" keep the width.
 */
export function ScheduleActionsMenu({
  schedule,
  canManage,
  running,
  onRunNow,
  onRunLog,
  onEdit,
  onDelete,
}: {
  schedule: ScheduleSummary;
  canManage: boolean;
  running: boolean;
  onRunNow: (s: ScheduleSummary) => void;
  onRunLog: (s: ScheduleSummary) => void;
  onEdit: (s: ScheduleSummary) => void;
  onDelete: (s: ScheduleSummary) => void;
}) {
  return (
    <RowActionsMenu label={`Actions for ${schedule.name}`}>
      <DropdownMenuItem onClick={() => onRunLog(schedule)}>Run log</DropdownMenuItem>
      {canManage && (
        <>
          <DropdownMenuItem disabled={running} onClick={() => onRunNow(schedule)}>
            Run now
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => onEdit(schedule)}>Edit…</DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem
            className="text-destructive focus:text-destructive"
            onClick={() => onDelete(schedule)}
          >
            Delete
          </DropdownMenuItem>
        </>
      )}
    </RowActionsMenu>
  );
}
