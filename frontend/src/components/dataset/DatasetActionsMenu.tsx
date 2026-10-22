import { useNavigate } from "react-router-dom";
import { toast } from "sonner";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/components/ui/dropdown-menu";
import { RowActionsMenu } from "@/components/explore/RowActionsMenu";
import { MoveToFolderItems } from "@/components/explore/MoveToFolderSub";
import { useRefreshDataset, type DatasetSummary } from "@/api/datasets";
import type { Folder } from "@/api/folders";
import { refreshSummary } from "@/components/dataset/refreshSummary";

/**
 * Per-row actions. Live: Edit, Refresh, Duplicate, Move to folder,
 * Move to Trash. Trashed: Restore, Delete permanently.
 */
export function DatasetActionsMenu({ ws, ds, trashed, folders, onDuplicate, onMove, onNewFolder, onTrash, onRestore, onDeletePermanent, onShare, onTags }: {
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
  onShare?: (ds: DatasetSummary) => void;
  onTags?: (ds: DatasetSummary) => void;
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
        onClick={() => refresh.mutateAsync().then((r) => toast.success(refreshSummary(r))).catch((e) => toast.error(String(e)))}>
        {refresh.isPending ? "Refreshing…" : "Refresh"}
      </DropdownMenuItem>
      {/* Both kinds (KI-110): the copy keeps the column settings and metrics, which is the part
          of a physical dataset that cannot be recreated by picking the table again. */}
      <DropdownMenuItem onClick={() => onDuplicate(ds)}>Duplicate</DropdownMenuItem>
      <MoveToFolderItems
        folders={folders}
        currentFolderId={ds.folder_id ?? null}
        onMove={(fid) => onMove(ds, fid)}
        onNewFolder={() => onNewFolder(ds)}
      />
      {onShare && <DropdownMenuItem onClick={() => onShare(ds)}>Share…</DropdownMenuItem>}
      {onTags && <DropdownMenuItem onClick={() => onTags(ds)}>Tags…</DropdownMenuItem>}
      <DropdownMenuSeparator />
      <DropdownMenuItem onClick={() => onTrash(ds)}>Move to Trash</DropdownMenuItem>
    </RowActionsMenu>
  );
}
