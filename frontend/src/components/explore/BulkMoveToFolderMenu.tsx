import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { Folder } from "@/api/folders";

/**
 * The bulk toolbar's "Move n to folder…": a PICKER of the folders that exist, not a name to type.
 *
 * It used to open a name box on every list, and any name that box did not recognise created a
 * folder — so a typo of an existing folder's name made a near-duplicate, and someone allowed to
 * move rows but not to create folders was refused. Now:
 *  - each existing folder, then Uncategorized (`onMove(null)`: out of any folder);
 *  - "New folder…" only when `onNewFolder` is given — pass it only to a caller who may create
 *    one; it is the single way to the name box;
 *  - nothing at all when there is no folder and none can be made: there is nowhere to move to.
 */
export function BulkMoveToFolderMenu({ count, folders, onMove, onNewFolder }: {
  /** How many rows the move will be applied to; shown in the trigger. */
  count: number;
  folders: Pick<Folder, "id" | "name">[];
  onMove: (folderId: string | null) => void;
  onNewFolder?: () => void;
}) {
  if (folders.length === 0 && !onNewFolder) return null;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="outline" size="sm">Move {count} to folder&hellip;</Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start">
        {folders.map((f) => (
          <DropdownMenuItem key={f.id} onClick={() => onMove(f.id)}>{f.name}</DropdownMenuItem>
        ))}
        {folders.length > 0 && <DropdownMenuItem onClick={() => onMove(null)}>Uncategorized</DropdownMenuItem>}
        {onNewFolder && (
          <>
            {folders.length > 0 && <DropdownMenuSeparator />}
            <DropdownMenuItem onClick={onNewFolder}>New folder&hellip;</DropdownMenuItem>
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/**
 * Apply `move` to each id in turn, one request and one verdict per row. A refusal goes to
 * `onError` and the rest still move; the return value counts only what moved, so a success
 * message cannot claim a row the server refused.
 */
export async function moveEach(
  ids: readonly string[],
  move: (id: string) => Promise<unknown>,
  onError: (e: unknown) => void,
): Promise<number> {
  let moved = 0;
  for (const id of ids) {
    try { await move(id); moved += 1; } catch (e) { onError(e); }
  }
  return moved;
}
