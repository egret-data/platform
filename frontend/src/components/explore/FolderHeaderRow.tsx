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
      <td colSpan={colSpan} className="py-1">
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
