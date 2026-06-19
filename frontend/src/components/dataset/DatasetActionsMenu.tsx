import { useNavigate } from "react-router-dom";
import { MoreHorizontal } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useRefreshDataset, type DatasetSummary } from "@/api/datasets";

/** Per-row actions: Edit (→ detail edit), Refresh, Duplicate (virtual), Delete. */
export function DatasetActionsMenu({ ws, ds, onDuplicate, onDelete }: {
  ws: string;
  ds: DatasetSummary;
  onDuplicate: (ds: DatasetSummary) => void;
  onDelete: (ds: DatasetSummary) => void;
}) {
  const nav = useNavigate();
  const refresh = useRefreshDataset(ws, ds.id);

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="sm" className="h-7 w-7 p-0" title="Actions">
          <MoreHorizontal className="h-4 w-4" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem onClick={() => nav(`/explore/${ws}/datasets/${ds.id}?edit=1`)}>Edit</DropdownMenuItem>
        <DropdownMenuItem
          disabled={refresh.isPending}
          onClick={() =>
            refresh.mutateAsync().then(() => toast.success("Refreshed")).catch((e) => toast.error(String(e)))
          }>
          {refresh.isPending ? "Refreshing…" : "Refresh"}
        </DropdownMenuItem>
        {ds.kind === "virtual" && (
          <DropdownMenuItem onClick={() => onDuplicate(ds)}>Duplicate</DropdownMenuItem>
        )}
        <DropdownMenuSeparator />
        <DropdownMenuItem className="text-destructive focus:text-destructive" onClick={() => onDelete(ds)}>
          Delete
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
