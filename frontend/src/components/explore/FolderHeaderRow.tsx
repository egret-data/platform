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
      {/* `role="rowheader"` makes this cell announce as a header rather than as
          one more data cell. That is the whole of what it buys, and the claim
          is deliberately no larger than that.

          What it does NOT buy is a group association. In HTML, `<th
          scope="rowgroup">` says "this heads the remaining cells in this row
          group"; ARIA has no `scope` equivalent, so this cell is a rowheader
          for its own row — which contains nothing else — and is not associated
          with the chart rows beneath it. Nothing is re-announced on those rows,
          but that is because no association exists at all, not because one was
          scoped correctly. Against the `cell` role it replaces this is a strict
          improvement; it is not the full semantics of the HTML spelling.

          The role itself is right for the shape rather than borrowed: HTML-AAM
          maps `<th scope="rowgroup">` to `rowheader`, and ARIA has no separate
          rowgroup-header role, so `rowheader` is what this construct gets. Its
          required context is a `row`, which this cell has.

          A `<td>` with an explicit role rather than a `<th scope="rowgroup">`,
          for the reason `ConfigTable.tsx` records: a `<th>` brings its own
          implicit role to argue with. Note the tradeoff that follows from that
          choice — `<th scope="rowgroup">` WOULD carry the association this
          does not. `scope` is absent here because it is a `<th>`-only
          attribute and inert on a `<td>`; adding it would be decoration. */}
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
