import { useNavigate } from "react-router-dom";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/components/ui/dropdown-menu";
import { RowActionsMenu } from "@/components/explore/RowActionsMenu";
import { MoveToFolderItems } from "@/components/explore/MoveToFolderSub";
import type { Chart } from "@/api/charts";
import type { Folder } from "@/api/folders";

export function ChartActionsMenu({ ws, chart, trashed, folders, onDuplicate, onMove, onNewFolder, onTrash, onRestore, onDeletePermanent, onShare }: {
  ws: string; chart: Chart; trashed: boolean; folders: Folder[];
  onDuplicate: (c: Chart) => void;
  onMove: (c: Chart, folderId: string | null) => void;
  onNewFolder: (c: Chart) => void;
  onTrash: (c: Chart) => void;
  onRestore: (c: Chart) => void;
  onDeletePermanent: (c: Chart) => void;
  onShare?: (c: Chart) => void;
}) {
  const nav = useNavigate();
  if (trashed) {
    return (
      <RowActionsMenu>
        <DropdownMenuItem onClick={() => onRestore(chart)}>Restore</DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem className="text-destructive focus:text-destructive" onClick={() => onDeletePermanent(chart)}>Delete permanently</DropdownMenuItem>
      </RowActionsMenu>
    );
  }
  return (
    <RowActionsMenu>
      <DropdownMenuItem onClick={() => nav(`/explore/${ws}/charts/${chart.id}`)}>Open</DropdownMenuItem>
      <DropdownMenuItem onClick={() => onDuplicate(chart)}>Duplicate</DropdownMenuItem>
      <MoveToFolderItems
        folders={folders}
        currentFolderId={chart.folder_id ?? null}
        onMove={(fid) => onMove(chart, fid)}
        onNewFolder={() => onNewFolder(chart)}
      />
      {onShare && <DropdownMenuItem onClick={() => onShare(chart)}>Share…</DropdownMenuItem>}
      {/* Discovery: both reference products create a subscription FROM the
          object, not from a separate page you must already know about. */}
      <DropdownMenuItem
        onClick={() =>
          nav(
            `/explore/${ws}/alerts?new=report&kind=chart&object=${chart.id}`,
          )
        }
      >
        Subscribe…
      </DropdownMenuItem>

      <DropdownMenuSeparator />
      <DropdownMenuItem onClick={() => onTrash(chart)}>Move to Trash</DropdownMenuItem>
    </RowActionsMenu>
  );
}
