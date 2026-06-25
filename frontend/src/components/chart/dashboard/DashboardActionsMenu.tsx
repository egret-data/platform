import { useNavigate } from "react-router-dom";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/components/ui/dropdown-menu";
import { RowActionsMenu } from "@/components/explore/RowActionsMenu";
import type { DashboardSummary } from "@/api/dashboards";

export function DashboardActionsMenu({ ws, dash, trashed, onDuplicate, onTrash, onRestore, onDeletePermanent }: {
  ws: string;
  dash: DashboardSummary;
  trashed: boolean;
  onDuplicate: (d: DashboardSummary) => void;
  onTrash: (d: DashboardSummary) => void;
  onRestore: (d: DashboardSummary) => void;
  onDeletePermanent: (d: DashboardSummary) => void;
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
      <DropdownMenuSeparator />
      <DropdownMenuItem onClick={() => onTrash(dash)}>Move to Trash</DropdownMenuItem>
    </RowActionsMenu>
  );
}
