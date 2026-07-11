import { useNavigate } from "react-router-dom";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/components/ui/dropdown-menu";
import { RowActionsMenu } from "@/components/explore/RowActionsMenu";
import { MoveToFolderItems } from "@/components/explore/MoveToFolderSub";
import type { DashboardSummary } from "@/api/dashboards";
import type { Folder } from "@/api/folders";

export function DashboardActionsMenu({ ws, dash, trashed, folders, onDuplicate, onMove, onNewFolder, onTrash, onRestore, onDeletePermanent, onShare }: {
  ws: string;
  dash: DashboardSummary;
  trashed: boolean;
  folders: Folder[];
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
    return (
      <RowActionsMenu>
        <DropdownMenuItem onClick={() => onRestore(dash)}>Restore</DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem className="text-destructive focus:text-destructive" onClick={() => onDeletePermanent(dash)}>Delete permanently</DropdownMenuItem>
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
      <DropdownMenuSeparator />
      <DropdownMenuItem onClick={() => onTrash(dash)}>Move to Trash</DropdownMenuItem>
    </RowActionsMenu>
  );
}
