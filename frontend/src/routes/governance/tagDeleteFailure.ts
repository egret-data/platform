import { toast } from "sonner";
import { ApiError } from "@/api/client";
import { TERM } from "@/routes/governance/glossary";

/**
 * A refused classification delete, in a toast (FE-ACTION-3). `DELETE /tags/{id}`
 * answers 409 while a rule uses the classification: the rules table's tag key
 * references `tags(key)` (migration 0154), so the classification cannot go
 * before its rules. The toast keeps the server's sentence and says where
 * those rules are listed: the classification page's "Rules" tab
 * (`TagRulesSection`, shown to a `policy:read` holder). `where` names that
 * page from the page the delete was asked on; `showRules`, when given, opens
 * the tab. (Task 11 rewrites this on the `tag_in_use` code.)
 */
export function toastTagDeleteFailure(
  e: unknown,
  { where, canReadRules, showRules }: { where: "this page" | "the classification's page"; canReadRules: boolean; showRules?: () => void },
) {
  const message = e instanceof Error && e.message ? e.message : "The classification could not be deleted. Try again.";
  if (!(e instanceof ApiError) || e.status !== 409) {
    toast.error(message);
    return;
  }
  toast.error(message, {
    description: canReadRules
      ? `The "${TERM.rules}" tab on ${where} lists the rules that use this classification.`
      : `The "${TERM.rules}" tab on ${where} lists the rules that use this classification to a holder of policy:read.`,
    action: showRules ? { label: "Show the rules", onClick: showRules } : undefined,
  });
}
