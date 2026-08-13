import { useNavigate } from "react-router-dom";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/components/ui/dropdown-menu";
import { RowActionsMenu } from "@/components/explore/RowActionsMenu";
import { MoveToFolderItems } from "@/components/explore/MoveToFolderSub";
import type { DashboardSummary } from "@/api/dashboards";
import type { Folder } from "@/api/folders";

export function DashboardActionsMenu({ ws, dash, trashed, folders, canManage = true, onDuplicate, onMove, onNewFolder, onTrash, onRestore, onDeletePermanent, onShare }: {
  ws: string;
  dash: DashboardSummary;
  trashed: boolean;
  folders: Folder[];
  /** Whether to offer anything beyond Open. Defaults to `true` so existing callers — and any
   *  response predating the server field — keep the menu they have always had. */
  canManage?: boolean;
  onDuplicate: (d: DashboardSummary) => void;
  onMove: (d: DashboardSummary, folderId: string | null) => void;
  onNewFolder: (d: DashboardSummary) => void;
  onTrash: (d: DashboardSummary) => void;
  onRestore: (d: DashboardSummary) => void;
  onDeletePermanent: (d: DashboardSummary) => void;
  onShare?: (d: DashboardSummary) => void;
}) {
  const nav = useNavigate();
  if (trashed) {
    // Restore and Delete permanently are BOTH manage actions, so a reader browsing Trash gets an
    // empty menu — render nothing at all rather than a trigger that opens onto nothing.
    if (!canManage) return null;
    return (
      <RowActionsMenu>
        <DropdownMenuItem onClick={() => onRestore(dash)}>Restore</DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem className="text-destructive focus:text-destructive" onClick={() => onDeletePermanent(dash)}>Delete permanently</DropdownMenuItem>
      </RowActionsMenu>
    );
  }
  if (!canManage) {
    // Open is not a mutation, so it stays: a reader keeps the way in, and loses only the
    // actions the server would refuse.
    return (
      <RowActionsMenu>
        <DropdownMenuItem onClick={() => nav(`/explore/${ws}/dashboards/${dash.id}`)}>Open</DropdownMenuItem>
      </RowActionsMenu>
    );
  }
  return (
    <RowActionsMenu>
      <DropdownMenuItem onClick={() => nav(`/explore/${ws}/dashboards/${dash.id}`)}>Open</DropdownMenuItem>
      <DropdownMenuItem onClick={() => onDuplicate(dash)}>Duplicate</DropdownMenuItem>
      <MoveToFolderItems
        folders={folders}
        currentFolderId={dash.folder_id ?? null}
        onMove={(fid) => onMove(dash, fid)}
        onNewFolder={() => onNewFolder(dash)}
      />
      {onShare && <DropdownMenuItem onClick={() => onShare(dash)}>Share…</DropdownMenuItem>}
      {/* Discovery: both reference products create a subscription FROM the
          object, not from a separate page you must already know about. */}
      <DropdownMenuItem
        onClick={() =>
          nav(
            `/explore/${ws}/alerts/new?type=report&kind=dashboard&object=${dash.id}`,
          )
        }
      >
        Subscribe…
      </DropdownMenuItem>

      <DropdownMenuSeparator />
      <DropdownMenuItem onClick={() => onTrash(dash)}>Move to Trash</DropdownMenuItem>
    </RowActionsMenu>
  );
}
