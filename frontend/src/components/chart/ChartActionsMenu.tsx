import { useNavigate } from "react-router-dom";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/components/ui/dropdown-menu";
import { RowActionsMenu } from "@/components/explore/RowActionsMenu";
import type { Chart } from "@/api/charts";

export function ChartActionsMenu({ ws, chart, onDuplicate, onDelete }: {
  ws: string; chart: Chart; onDuplicate: (c: Chart) => void; onDelete: (c: Chart) => void;
}) {
  const nav = useNavigate();
  return (
    <RowActionsMenu>
      <DropdownMenuItem onClick={() => nav(`/explore/${ws}/charts/${chart.id}`)}>Open</DropdownMenuItem>
      <DropdownMenuItem onClick={() => onDuplicate(chart)}>Duplicate</DropdownMenuItem>
      <DropdownMenuSeparator />
      <DropdownMenuItem className="text-destructive focus:text-destructive" onClick={() => onDelete(chart)}>Delete</DropdownMenuItem>
    </RowActionsMenu>
  );
}
