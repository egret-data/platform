import { DropdownMenuItem, DropdownMenuSeparator } from "@/components/ui/dropdown-menu";
import { RowActionsMenu } from "@/components/explore/RowActionsMenu";
import { MoveToFolderItems } from "@/components/explore/MoveToFolderSub";
import type { ScheduleSummary } from "@/api/alerts";
import type { Folder } from "@/api/folders";

/**
 * The row's actions, in one `…` menu — the same shape as `ChartActionsMenu`,
 * including its trashed branch.
 *
 * Four inline text buttons per row (what this replaced) cost ~380px of every
 * row and put Delete one mis-click from Run now. Collapsing them also lets the
 * columns that answer "is this thing healthy?" keep the width.
 */
export function ScheduleActionsMenu({
  schedule,
  canManage,
  trashed,
  running,
  folders,
  onRunNow,
  onRunLog,
  onEdit,
  onTrash,
  onRestore,
  onDeletePermanent,
  onMove,
  onNewFolder,
}: {
  schedule: ScheduleSummary;
  canManage: boolean;
  trashed: boolean;
  running: boolean;
  folders: Folder[];
  onRunNow: (s: ScheduleSummary) => void;
  onRunLog: (s: ScheduleSummary) => void;
  onEdit: (s: ScheduleSummary) => void;
  onTrash: (s: ScheduleSummary) => void;
  onRestore: (s: ScheduleSummary) => void;
  onDeletePermanent: (s: ScheduleSummary) => void;
  onMove: (s: ScheduleSummary, folderId: string | null) => void;
  onNewFolder: (s: ScheduleSummary) => void;
}) {
  if (trashed) {
    // A trashed row offers only the two ways out of the bin. Run log is
    // withheld too: it reads a schedule the user has already discarded. A
    // reader gets no menu at all rather than an empty one that opens onto
    // nothing.
    if (!canManage) return null;
    return (
      <RowActionsMenu label={`Actions for ${schedule.name}`}>
        <DropdownMenuItem onClick={() => onRestore(schedule)}>Restore</DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          className="text-destructive focus:text-destructive"
          onClick={() => onDeletePermanent(schedule)}
        >
          Delete permanently
        </DropdownMenuItem>
      </RowActionsMenu>
    );
  }
  return (
    <RowActionsMenu label={`Actions for ${schedule.name}`}>
      <DropdownMenuItem onClick={() => onRunLog(schedule)}>Run log</DropdownMenuItem>
      {canManage && (
        <>
          <DropdownMenuItem disabled={running} onClick={() => onRunNow(schedule)}>
            Run now
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => onEdit(schedule)}>Edit…</DropdownMenuItem>
          <MoveToFolderItems
            folders={folders}
            currentFolderId={schedule.folder_id ?? null}
            onMove={(fid) => onMove(schedule, fid)}
            onNewFolder={() => onNewFolder(schedule)}
          />
          <DropdownMenuSeparator />
          <DropdownMenuItem onClick={() => onTrash(schedule)}>Move to Trash</DropdownMenuItem>
        </>
      )}
    </RowActionsMenu>
  );
}
