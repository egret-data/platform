import { useNavigate } from "react-router-dom";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/components/ui/dropdown-menu";
import { RowActionsMenu } from "@/components/explore/RowActionsMenu";
import { MoveToFolderItems } from "@/components/explore/MoveToFolderSub";
import type { Chart } from "@/api/charts";
import type { Folder } from "@/api/folders";

export function ChartActionsMenu({ ws, chart, folders, onDuplicate, onMove, onNewFolder, onDelete }: {
  ws: string; chart: Chart; folders: Folder[];
  onDuplicate: (c: Chart) => void;
  onMove: (c: Chart, folderId: string | null) => void;
  onNewFolder: (c: Chart) => void;
  onDelete: (c: Chart) => void;
}) {
  const nav = useNavigate();
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
      <DropdownMenuSeparator />
      <DropdownMenuItem className="text-destructive focus:text-destructive" onClick={() => onDelete(chart)}>Delete</DropdownMenuItem>
    </RowActionsMenu>
  );
}
