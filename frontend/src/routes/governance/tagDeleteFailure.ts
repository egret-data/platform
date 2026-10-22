import { toast } from "sonner";
import { tagInUse } from "@/routes/governance/apiErrors";
import { TERM } from "@/routes/governance/glossary";

/**
 * A refused classification delete, in a toast (FE-ACTION-3, spec C11). The
 * delete dialog already withholds Delete while the rules it can read use the
 * classification; this is what is left: a rule written meanwhile, or a
 * deleter without `policy:read`. On `tag_in_use` the toast says how many
 * rules use the classification and names them, never the server's table
 * (`perm.rules`); `showRules`, when given, opens the classification's Rules
 * tab. Any other refusal keeps the server's message.
 */
export function toastTagDeleteFailure(e: unknown, { tagName, showRules }: { tagName: string; showRules?: () => void }) {
  const inUse = tagInUse(e);
  if (inUse) {
    const n = inUse.rules.length;
    const rules = TERM.rules.toLowerCase();
    const count = n === 0 ? rules : `${n} ${n === 1 ? TERM.rule.toLowerCase() : rules}`;
    toast.error(`${tagName} is still used by ${count}.`, {
      description: n > 0 ? `${TERM.delete} or change these ${rules} first: ${inUse.rules.map((r) => r.name).join(", ")}.` : undefined,
      action: showRules ? { label: `Show ${rules}`, onClick: showRules } : undefined,
    });
    return;
  }
  toast.error(e instanceof Error && e.message ? e.message : "The classification could not be deleted. Try again.");
}
