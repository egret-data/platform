import { Link } from "react-router-dom";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/components/ui/dropdown-menu";
import { RowActionsMenu } from "@/components/explore/RowActionsMenu";
import type { DataApiServiceAccountDto } from "@/api/generated/model";

/**
 * Per-row actions for a Data API service account: issue it a new OAuth2
 * client, then delete the account itself.
 *
 * Same shape as `EndpointActionsMenu`/`DatasetActionsMenu`: a `RowActionsMenu`
 * wrapper with the destructive item last, behind a `DropdownMenuSeparator` and
 * marked `text-destructive`.
 *
 * "Issue client…" is a real LINK (`DropdownMenuItem asChild`, the idiom
 * `Classifications.tsx` uses) to the credential's own page. It used to be a
 * callback opening a dialog the PAGE had to own, because a `DropdownMenuItem`
 * unmounts its whole menu on click and took any dialog rendered inside it along;
 * that whole problem is gone with the dialog. A link also gets middle-click and
 * open-in-a-new-tab for free, which a button pushing the route imperatively
 * cannot. Delete stays a callback — its confirmation is still a dialog, and
 * dialogs still belong to the page.
 *
 * Deliberately absent: **Revoke**. A revoke acts on one client credential, not
 * on the account, so it stays on the client's own line inside `ClientsCell` —
 * this menu would put it on the wrong object, and an account with four clients
 * has no single "the client" to revoke.
 */
export function ServiceAccountActionsMenu({ ws, account, onDelete }: {
  /** Needed only to build the issue-client URL; the row itself carries the id. */
  ws: string;
  account: DataApiServiceAccountDto;
  onDelete: () => void;
}) {
  return (
    <RowActionsMenu label={`Actions for ${account.name}`}>
      <DropdownMenuItem asChild>
        <Link to={`/explore/${ws}/data-api/clients/${account.id}/new-client`}>Issue client…</Link>
      </DropdownMenuItem>
      <DropdownMenuSeparator />
      <DropdownMenuItem className="text-destructive focus:text-destructive" onClick={onDelete}>
        Delete
      </DropdownMenuItem>
    </RowActionsMenu>
  );
}
