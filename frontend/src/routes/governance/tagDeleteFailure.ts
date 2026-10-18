import { toast } from "sonner";
import { ApiError } from "@/api/client";

/**
 * A refused tag delete, in a toast (FE-ACTION-3). `DELETE /tags/{id}` answers
 * 409 while a permission rule selects the tag: `perm.rules.tag_key` references
 * `tags(key)` (migration 0154), so the tag cannot go before its rules. The
 * toast keeps the server's sentence and says where those rules are listed:
 * the tag page's "Mask and filter rules" section (`TagRulesSection`, shown to
 * a `policy:read` holder). `where` names that section from the page the
 * delete was asked on; `showRules`, when given, opens it.
 */
export function toastTagDeleteFailure(
  e: unknown,
  { where, canReadRules, showRules }: { where: "this page" | "the tag's page"; canReadRules: boolean; showRules?: () => void },
) {
  const message = e instanceof Error && e.message ? e.message : "The tag could not be deleted. Try again.";
  if (!(e instanceof ApiError) || e.status !== 409) {
    toast.error(message);
    return;
  }
  toast.error(message, {
    description: canReadRules
      ? `The "Mask and filter rules" section on ${where} lists the rules that select this tag.`
      : `The "Mask and filter rules" section on ${where} lists the rules that select this tag to a holder of policy:read.`,
    action: showRules ? { label: "Show the rules", onClick: showRules } : undefined,
  });
}
