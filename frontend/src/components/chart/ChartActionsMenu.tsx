import { useNavigate } from "react-router-dom";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/components/ui/dropdown-menu";
import { RowActionsMenu } from "@/components/explore/RowActionsMenu";
import type { Chart } from "@/api/charts";

export function ChartActionsMenu({ ws, chart, onDelete }: {
  ws: string; chart: Chart; onDelete: (c: Chart) => void;
}) {
  const nav = useNavigate();
  return (
    <RowActionsMenu>
      <DropdownMenuItem onClick={() => nav(`/explore/${ws}/charts/${chart.id}`)}>Open</DropdownMenuItem>
      <DropdownMenuSeparator />
      <DropdownMenuItem className="text-destructive focus:text-destructive" onClick={() => onDelete(chart)}>Delete</DropdownMenuItem>
    </RowActionsMenu>
  );
}
