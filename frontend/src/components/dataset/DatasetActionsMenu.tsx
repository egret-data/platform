import { useNavigate } from "react-router-dom";
import { toast } from "sonner";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/components/ui/dropdown-menu";
import { RowActionsMenu } from "@/components/explore/RowActionsMenu";
import { MoveToFolderItems } from "@/components/explore/MoveToFolderSub";
import { useRefreshDataset, type DatasetSummary } from "@/api/datasets";
import type { Folder } from "@/api/folders";

/**
 * Per-row actions. Live: Edit, Refresh, Duplicate (virtual), Move to folder,
 * Move to Trash. Trashed: Restore, Delete permanently.
 */
export function DatasetActionsMenu({ ws, ds, trashed, folders, onDuplicate, onMove, onNewFolder, onTrash, onRestore, onDeletePermanent }: {
  ws: string;
  ds: DatasetSummary;
  trashed: boolean;
  folders: Folder[];
  onDuplicate: (ds: DatasetSummary) => void;
  onMove: (ds: DatasetSummary, folderId: string | null) => void;
  onNewFolder: (ds: DatasetSummary) => void;
  onTrash: (ds: DatasetSummary) => void;
  onRestore: (ds: DatasetSummary) => void;
  onDeletePermanent: (ds: DatasetSummary) => void;
}) {
  const nav = useNavigate();
  const refresh = useRefreshDataset(ws, ds.id);

  if (trashed) {
    return (
      <RowActionsMenu>
        <DropdownMenuItem onClick={() => onRestore(ds)}>Restore</DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem className="text-destructive focus:text-destructive" onClick={() => onDeletePermanent(ds)}>Delete permanently</DropdownMenuItem>
      </RowActionsMenu>
    );
  }

  return (
    <RowActionsMenu>
      <DropdownMenuItem onClick={() => nav(`/explore/${ws}/datasets/${ds.id}?edit=1`)}>Edit</DropdownMenuItem>
      <DropdownMenuItem
        disabled={refresh.isPending}
        onClick={() => refresh.mutateAsync().then(() => toast.success("Refreshed")).catch((e) => toast.error(String(e)))}>
        {refresh.isPending ? "Refreshing…" : "Refresh"}
      </DropdownMenuItem>
      {ds.kind === "virtual" && <DropdownMenuItem onClick={() => onDuplicate(ds)}>Duplicate</DropdownMenuItem>}
      <MoveToFolderItems
        folders={folders}
        currentFolderId={ds.folder_id ?? null}
        onMove={(fid) => onMove(ds, fid)}
        onNewFolder={() => onNewFolder(ds)}
      />
      <DropdownMenuSeparator />
      <DropdownMenuItem onClick={() => onTrash(ds)}>Move to Trash</DropdownMenuItem>
    </RowActionsMenu>
  );
}
