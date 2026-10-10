import { useId } from "react";
import { useNavigate } from "react-router-dom";

import type { AppSummary } from "@/api/apps";
import type { Folder } from "@/api/folders";
import { MoveToFolderItems } from "@/components/explore/MoveToFolderSub";
import { RowActionsMenu } from "@/components/explore/RowActionsMenu";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/components/ui/dropdown-menu";

/** Why Open is greyed on an app that has never been published. Exported so the list and its
 *  tests name one string. */
export const OPEN_DISABLED_REASON = "Publish a draft before it can be opened.";

/**
 * The row menu on the Apps list.
 *
 * Gated per ROW, from the row's own `can_write` / `can_manage` — not from one page-wide flag as
 * `DashboardActionsMenu` is. App permissions are granted per app through shares, so a
 * share-only collaborator can write one row and only read the next.
 *
 * Actions the caller may not take are WITHHELD, not disabled: a greyed destructive control
 * advertises a capability the reader does not have. Open is the one exception — it is not a
 * mutation, and its disabled state says what is missing.
 */
export function AppActionsMenu({ ws, app, trashed, folders, onRename, onDuplicate, onMove, onNewFolder, onShare, onTrash, onRestore, onDeletePermanent }: {
  ws: string;
  app: AppSummary;
  trashed: boolean;
  folders: Folder[];
  onRename: (a: AppSummary) => void;
  onDuplicate: (a: AppSummary) => void;
  onMove: (a: AppSummary, folderId: string | null) => void;
  /** Absent when the caller may not create folders in this workspace: the item is withheld. */
  onNewFolder?: (a: AppSummary) => void;
  onShare: (a: AppSummary) => void;
  onTrash: (a: AppSummary) => void;
  onRestore: (a: AppSummary) => void;
  onDeletePermanent: (a: AppSummary) => void;
}) {
  const nav = useNavigate();
  const reasonId = useId();
  const { can_write: canWrite, can_manage: canManage } = app;

  if (trashed) {
    // Restore needs write and Delete permanently needs manage. A caller with neither would get
    // an empty menu — render nothing at all rather than a trigger that opens onto nothing.
    if (!canWrite && !canManage) return null;
    return (
      <RowActionsMenu>
        {canWrite && <DropdownMenuItem onClick={() => onRestore(app)}>Restore</DropdownMenuItem>}
        {canWrite && canManage && <DropdownMenuSeparator />}
        {canManage && (
          <DropdownMenuItem className="text-destructive focus:text-destructive" onClick={() => onDeletePermanent(app)}>
            Delete permanently
          </DropdownMenuItem>
        )}
      </RowActionsMenu>
    );
  }

  const published = app.latest_version_no != null;
  return (
    <RowActionsMenu>
      {/* Open is not a mutation, so it stays for everyone: a reader keeps the way in, and loses
          only the actions the server would refuse. */}
      {published ? (
        <DropdownMenuItem onClick={() => nav(`/explore/${ws}/apps/${app.id}`)}>Open</DropdownMenuItem>
      ) : (
        <>
          <DropdownMenuItem disabled aria-describedby={reasonId}>Open</DropdownMenuItem>
          {/* A sibling, not a child of the item: a disabled item is drawn at half opacity and
              takes no pointer events, so a reason inside it would be the hardest text in the
              menu to read and could not carry a hover tooltip either. */}
          <p id={reasonId} className="max-w-48 px-2 pb-1.5 text-xs text-muted-foreground">{OPEN_DISABLED_REASON}</p>
        </>
      )}
      {canWrite && (
        <>
          <DropdownMenuItem onClick={() => nav(`/explore/${ws}/apps/${app.id}/edit`)}>Edit</DropdownMenuItem>
          <DropdownMenuItem onClick={() => onRename(app)}>Rename…</DropdownMenuItem>
          <DropdownMenuItem onClick={() => onDuplicate(app)}>Duplicate</DropdownMenuItem>
          <MoveToFolderItems
            folders={folders}
            currentFolderId={app.folder_id ?? null}
            onMove={(fid) => onMove(app, fid)}
            onNewFolder={onNewFolder ? () => onNewFolder(app) : undefined}
          />
        </>
      )}
      {/* Sharing hands out grants, so it follows manage, not write. */}
      {canManage && <DropdownMenuItem onClick={() => onShare(app)}>Share…</DropdownMenuItem>}
      {canWrite && (
        <>
          <DropdownMenuSeparator />
          <DropdownMenuItem onClick={() => onTrash(app)}>Move to Trash</DropdownMenuItem>
        </>
      )}
    </RowActionsMenu>
  );
}
