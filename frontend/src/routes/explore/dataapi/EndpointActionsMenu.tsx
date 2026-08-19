import { useNavigate } from "react-router-dom";
import { toast } from "sonner";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/components/ui/dropdown-menu";
import { RowActionsMenu } from "@/components/explore/RowActionsMenu";
import { MoveToFolderItems } from "@/components/explore/MoveToFolderSub";
// Mutations come from the hand-written wrapper, never from the generated hooks:
// the wrapper is what owns the cache invalidation (see `src/api/dataApi.ts`).
// Wired to the generated hook directly, "Endpoint disabled" toasts over a row
// whose badge still reads Live for the next 30 seconds.
import { useDisableEndpoint, useEnableEndpoint, useRestoreEndpoint, useTrashEndpoint, useUpdateEndpoint } from "@/api/dataApi";
import type { Folder } from "@/api/folders";
import type { DataApiEndpointDto } from "@/api/generated/model";

/**
 * Per-row actions for a Data API endpoint.
 *
 * Two menus, chosen by `trashed`. In the TRASH the row can only be brought
 * back or destroyed — Restore, then Delete permanently — because every other
 * action here 409s on a trashed endpoint server-side (task 3 made publish,
 * enable, disable, rollback and PATCH all refuse it, each naming restore as
 * the way forward), so offering them would be offering guaranteed failures.
 * Otherwise: Open, the state-dependent Disable/Enable, the move-to-folder
 * choices, Move to Trash, then the permanent Delete.
 *
 * Same shape as `DatasetActionsMenu`/`ChartActionsMenu`: a `RowActionsMenu`
 * wrapper, the destructive item last behind a `DropdownMenuSeparator` and
 * marked `text-destructive`, and the row's own mutations owned here rather than
 * threaded down as callbacks. Disable and Enable read their `isPending` to
 * disable themselves in flight; the move items do not, so a move is not
 * currently guarded against a second click. Deletion stays a callback because
 * the confirmation dialog belongs to the page, not to one row's menu.
 *
 * The trigger is NAMED after its row, like `ServiceAccountActionsMenu`'s. The
 * default bare "Actions" leaves a 30-row list with 30 identically-named
 * controls, which a screen-reader user navigating by button cannot tell apart.
 */
export function EndpointActionsMenu({ ws, endpoint, trashed = false, folders, onNewFolder, onDelete }: {
  ws: string;
  endpoint: DataApiEndpointDto;
  /** Whether this row is being rendered in the Trash view. */
  trashed?: boolean;
  /** The workspace's endpoint folders, owned by the page (it already queries
   *  them to group the table) so the menu cannot hold a second, disagreeing copy. */
  folders: Folder[];
  /** Open the page's folder dialog in "move to a new folder" mode for THIS row. */
  onNewFolder: () => void;
  onDelete: () => void;
}) {
  const nav = useNavigate();
  const disableMutation = useDisableEndpoint();
  const enableMutation = useEnableEndpoint();
  const updateMutation = useUpdateEndpoint();
  const trashMutation = useTrashEndpoint();
  const restoreMutation = useRestoreEndpoint();

  /**
   * A `live` endpoint cannot be trashed: `POST …/trash` refuses it with a 409
   * reading "endpoint is live and cannot be moved to the trash; disable it
   * first", and the schema's own CHECK constraint (migration 0126) makes the
   * combination unrepresentable.
   *
   * The item is therefore rendered DISABLED with the reason beside it, rather
   * than hidden or left live. Hidden would make the capability undiscoverable
   * for exactly the endpoints whose authors most need to find it — you cannot
   * learn "disable it first" from a menu that never mentions trashing. Left
   * live it would spend a round trip for the server to say what this component
   * already knows. Disabled-with-a-reason is the only one of the three that
   * teaches the rule.
   *
   * None of that weakens the server: the 409 is the enforcement, and it stands
   * whether or not this menu ever renders.
   */
  const liveBlocksTrash = endpoint.state === "live";

  /**
   * Move this endpoint into `folderId`, or out of every folder when it is null.
   *
   * `folder_id` is `Option<Option<Uuid>>` on the wire (see
   * `UpdateEndpointInput`): an ABSENT key means "leave the folder alone" and an
   * explicit `null` means "move to Uncategorized". So the body is built with
   * the key always PRESENT and its value `null` for the Uncategorized case —
   * `{ folder_id: null }`, never `{}`. Conversely this is the only field sent:
   * a PATCH that also carried the row's name/sql would re-submit stale values
   * from whenever the list was last fetched.
   */
  async function move(folderId: string | null) {
    try {
      await updateMutation.mutateAsync({ workspace: ws, id: endpoint.id, data: { folder_id: folderId } });
      toast.success(folderId === null ? "Removed from folder" : "Moved");
    } catch (err) {
      toast.error(String(err));
    }
  }

  if (trashed) {
    return (
      <RowActionsMenu label={`Actions for ${endpoint.name}`}>
        <DropdownMenuItem
          disabled={restoreMutation.isPending}
          onClick={() => {
            if (restoreMutation.isPending) return;
            // Restore always brings the endpoint back as `disabled`, never as
            // whatever it was before — say so, because an author who trashed a
            // once-live endpoint will otherwise assume its URL is serving again.
            restoreMutation.mutateAsync({ workspace: ws, id: endpoint.id })
              .then(() => toast.success("Endpoint restored as disabled"))
              .catch((err) => toast.error(String(err)));
          }}
        >
          Restore
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem className="text-destructive focus:text-destructive" onClick={onDelete}>
          Delete permanently
        </DropdownMenuItem>
      </RowActionsMenu>
    );
  }

  return (
    <RowActionsMenu label={`Actions for ${endpoint.name}`}>
      <DropdownMenuItem onClick={() => nav(`/explore/${ws}/data-api/endpoints/${endpoint.id}`)}>Open</DropdownMenuItem>
      {endpoint.state === "live" && (
        // `disabled` while in flight, like every other mutation trigger in
        // this feature: the menu closes on click but the row's menu can be
        // reopened before the request settles, and a second Disable on an
        // already-disabled endpoint is a 409 the author never asked for.
        <DropdownMenuItem
          disabled={disableMutation.isPending}
          onClick={() =>
            disableMutation.mutateAsync({ workspace: ws, id: endpoint.id })
              .then(() => toast.success("Endpoint disabled"))
              .catch((err) => toast.error(String(err)))
          }
        >
          Disable
        </DropdownMenuItem>
      )}
      {endpoint.state === "disabled" && (
        // `useDataApiEnableEndpoint` 409s when `live_version_id` is null
        // (EndpointStore::enable, lifecycle.rs) — there is nothing to
        // re-enable. `disabled` + `title` communicate this up front rather
        // than let the user discover it through a failed-mutation toast;
        // the `live_version_id` check INSIDE the handler too, since
        // `data-[disabled]:pointer-events-none` is CSS — real browsers
        // honour it, but nothing stops a click from reaching `onClick`
        // in an environment that doesn't apply CSS at all (jsdom).
        <DropdownMenuItem
          disabled={endpoint.live_version_id == null || enableMutation.isPending}
          title={endpoint.live_version_id == null ? "This endpoint has never been published; publish it instead." : undefined}
          onClick={() => {
            if (endpoint.live_version_id == null || enableMutation.isPending) return;
            enableMutation.mutateAsync({ workspace: ws, id: endpoint.id })
              .then(() => toast.success("Endpoint enabled"))
              .catch((err) => toast.error(String(err)));
          }}
        >
          Enable
        </DropdownMenuItem>
      )}
      <MoveToFolderItems
        folders={folders}
        // `?? null`: the DTO's `folder_id` is optional AND nullable, and
        // `undefined` would make the row's own folder look like "not the
        // current one", so its entry would be offered as a live move to where
        // the row already is.
        currentFolderId={endpoint.folder_id ?? null}
        onMove={(folderId) => void move(folderId)}
        onNewFolder={onNewFolder}
      />
      <DropdownMenuSeparator />
      <DropdownMenuItem
        disabled={liveBlocksTrash || trashMutation.isPending}
        onClick={() => {
          // The guard is repeated inside the handler for the same reason the
          // Enable item repeats its own: `data-[disabled]:pointer-events-none`
          // is CSS, which jsdom does not apply, so `disabled` alone stops the
          // click in a browser and nowhere else.
          if (liveBlocksTrash || trashMutation.isPending) return;
          trashMutation.mutateAsync({ workspace: ws, id: endpoint.id })
            .then(() => toast.success("Moved to Trash"))
            .catch((err) => toast.error(String(err)));
        }}
      >
        <span className="flex flex-col items-start">
          <span>Move to Trash</span>
          {liveBlocksTrash && <span className="text-xs text-muted-foreground">Disable it first</span>}
        </span>
      </DropdownMenuItem>
      <DropdownMenuItem className="text-destructive focus:text-destructive" onClick={onDelete}>
        Delete
      </DropdownMenuItem>
    </RowActionsMenu>
  );
}
