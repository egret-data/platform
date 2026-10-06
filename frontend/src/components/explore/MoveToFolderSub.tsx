import { DropdownMenuItem, DropdownMenuSeparator } from "@/components/ui/dropdown-menu";
import type { Folder } from "@/api/folders";

/** Flat "Move to folder" choices for a row kebab: each folder, Uncategorized, then New folder…
 *  (the last only when `onNewFolder` is given: creating a folder is a workspace permission the
 *  caller may lack while still being able to move this one row).
 *  Rendered inline (not a Radix sub-menu) to keep it simple + testable. */
export function MoveToFolderItems({ folders, currentFolderId, onMove, onNewFolder }: {
  folders: Folder[]; currentFolderId: string | null;
  onMove: (folderId: string | null) => void; onNewFolder?: () => void;
}) {
  return (
    <>
      <DropdownMenuSeparator />
      {folders.map((f) => (
        <DropdownMenuItem key={f.id} disabled={f.id === currentFolderId} onClick={() => onMove(f.id)}>
          Move to &quot;{f.name}&quot;
        </DropdownMenuItem>
      ))}
      {currentFolderId !== null && <DropdownMenuItem onClick={() => onMove(null)}>Remove from folder</DropdownMenuItem>}
      {onNewFolder && <DropdownMenuItem onClick={onNewFolder}>Move to new folder&hellip;</DropdownMenuItem>}
    </>
  );
}
