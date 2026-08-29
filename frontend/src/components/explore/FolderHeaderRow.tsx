import { ChevronDown, ChevronRight, MoreHorizontal } from "lucide-react";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";

/** A collapsible section header rendered as a full-width table row. */
export function FolderHeaderRow({ name, count, colSpan, collapsed, onToggle, onShare, onRename, onDelete, onDropItem }: {
  name: string; count: number; colSpan: number; collapsed: boolean; onToggle: () => void;
  onShare?: () => void; onRename?: () => void; onDelete?: () => void; onDropItem?: (id: string) => void;
}) {
  const Chevron = collapsed ? ChevronRight : ChevronDown;
  return (
    <tr
      className="border-t bg-muted/40"
      onDragOver={onDropItem ? (e) => e.preventDefault() : undefined}
      onDrop={onDropItem ? (e) => { const id = e.dataTransfer.getData("text/plain"); if (id) onDropItem(id); } : undefined}
    >
      {/* `role="rowheader"` is not a stretch of the ARIA table model and not
          merely the idiom `ConfigTableGroup` and `dataapi/Clients.tsx` already
          use: it is what HTML-AAM computes for `<th scope="rowgroup">`, and a
          full-span header cell heading the rows of one `<tbody>` is exactly
          what that spelling is for. ARIA has no separate rowgroup-header role,
          so this IS the role for this construct.

          Its required context is a `row`, which this cell has. The association
          is within a row, so nothing here is announced again on the data rows
          beneath — the whole effect is that the folder reads as a heading
          rather than as one more data cell, which is the confusion a spanning
          group row exists to remove.

          A `<td>` with an explicit role rather than a `<th scope="rowgroup">`,
          for the reason `ConfigTable.tsx` records: a `<th>` brings its own
          implicit role to argue with. `scope` is deliberately absent — it is a
          `<th>`-only attribute and inert on a `<td>`. */}
      <td colSpan={colSpan} role="rowheader" className="py-1">
        <div className="flex items-center gap-1">
          <button className="inline-flex items-center gap-1 font-medium hover:underline" onClick={onToggle} aria-label={`Toggle ${name}`}>
            <Chevron className="h-4 w-4 text-muted-foreground" />
            {name}
            <span className="text-muted-foreground">({count})</span>
          </button>
          {(onShare || onRename || onDelete) && (
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button variant="ghost" size="sm" className="ml-1 h-6 w-6 p-0" aria-label={`Folder ${name} actions`}>
                  <MoreHorizontal className="h-4 w-4" />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="start">
                {onShare && <DropdownMenuItem onClick={onShare}>Share…</DropdownMenuItem>}
                {onRename && <DropdownMenuItem onClick={onRename}>Rename</DropdownMenuItem>}
                {onDelete && <DropdownMenuItem className="text-destructive focus:text-destructive" onClick={onDelete}>Delete</DropdownMenuItem>}
              </DropdownMenuContent>
            </DropdownMenu>
          )}
        </div>
      </td>
    </tr>
  );
}
