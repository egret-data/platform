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
 * Unlike `EndpointActionsMenu` this menu owns no mutation of its own — both
 * items are callbacks. Both of this account's actions open a dialog (the issue
 * form, the delete confirmation), and dialogs belong to the page, not to one
 * row's menu: a `DropdownMenuItem` unmounts its whole menu on click, taking any
 * dialog rendered inside it with it.
 *
 * Deliberately absent: **Revoke**. A revoke acts on one client credential, not
 * on the account, so it stays on the client's own line inside `ClientsCell` —
 * this menu would put it on the wrong object, and an account with four clients
 * has no single "the client" to revoke.
 */
export function ServiceAccountActionsMenu({ account, onIssueClient, onDelete }: {
  account: DataApiServiceAccountDto;
  onIssueClient: () => void;
  onDelete: () => void;
}) {
  return (
    <RowActionsMenu label={`Actions for ${account.name}`}>
      <DropdownMenuItem onClick={onIssueClient}>Issue client…</DropdownMenuItem>
      <DropdownMenuSeparator />
      <DropdownMenuItem className="text-destructive focus:text-destructive" onClick={onDelete}>
        Delete
      </DropdownMenuItem>
    </RowActionsMenu>
  );
}
