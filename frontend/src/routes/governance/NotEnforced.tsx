import { AlertTriangle } from "lucide-react";
import type { NotEnforced, RuleEnforcement } from "@/api/generated/model";
import { usePermRulesEnforcement } from "@/api/generated/perm/perm";
import { usePolicyPermissions } from "@/routes/governance/policyWrites";
import { notEnforcedReason } from "@/routes/governance/ruleEffect";

export { invalidateRuleEnforcement } from "@/routes/governance/ruleEnforcementCache";

/** The lead words of every "not enforced" statement about a stored rule. */
export const NOT_ENFORCED_YET = "Not enforced yet";

/** At most this many rules are asked about in one request (`RULE_ENFORCEMENT_CAP` on the server). */
export const RULE_ENFORCEMENT_CAP = 100;

/**
 * Whether each of `ids` (stored rules) is enforced, from
 * `GET /perm/rules/enforcement` — the same computation as the rule preview's
 * `effect` (`crates/egret-perm-compile/src/effect.rs`). The distinct ids are
 * sorted and the first {@link RULE_ENFORCEMENT_CAP} asked about; a rule past
 * the cap gets no answer, so no badge (never a guess).
 *
 * `null` while nothing is known: no `policy:read`, loading, or failed. A
 * caller then says nothing about enforcement (the spec's rule: a status is
 * shown only when the data says so). Always refetched on mount: an engine
 * attached elsewhere changes the answer without any write on this page.
 */
export function useRuleEnforcement(ids: string[]): ReadonlyMap<string, RuleEnforcement> | null {
  const canRead = usePolicyPermissions()("policy:read");
  const asked = [...new Set(ids)].sort().slice(0, RULE_ENFORCEMENT_CAP);
  const q = usePermRulesEnforcement(
    { ids: asked.join(",") },
    { query: { enabled: canRead && asked.length > 0, retry: false, staleTime: 0 } },
  );
  if (!canRead || asked.length === 0) return asked.length === 0 && canRead ? new Map() : null;
  if (q?.data?.status !== 200) return null;
  return new Map(q.data.data.map((e) => [e.ruleId, e]));
}

/** Whether a rule is enforced, as {@link NotEnforcedBadge} reads it. */
export type EnforcementAnswer = { enforcedBy: number; notEnforced?: NotEnforced | null };

/**
 * A rule's enforcement IN ONE CATALOG, for a page about that catalog (the
 * Data pages): the server's entry for `catalogId` in `catalogs`, since a rule
 * enforced in catalog A may be enforced by nothing in catalog B. When the
 * server lists no entry for this catalog, only a rule-wide "not enforced"
 * holds here too; a rule-wide "enforced" says nothing about this catalog, so
 * the answer is unknown (`undefined`: no badge, never a guess).
 */
export function enforcementIn(e: RuleEnforcement | undefined, catalogId: string): EnforcementAnswer | undefined {
  if (!e) return undefined;
  const here = e.catalogs.find((c) => c.catalogId === catalogId);
  if (here) return { enforcedBy: here.engines.length, notEnforced: here.notEnforced ?? null };
  return e.enforcedBy === 0 ? { enforcedBy: 0, notEnforced: e.notEnforced ?? null } : undefined;
}

/**
 * {@link useRuleEnforcement}, answered for catalog `catalogId`
 * ({@link enforcementIn}): what a Data page badges. `null` while unknown.
 */
export function useRuleEnforcementIn(ids: string[], catalogId: string): ReadonlyMap<string, EnforcementAnswer> | null {
  const all = useRuleEnforcement(ids);
  if (all === null) return null;
  const out = new Map<string, EnforcementAnswer>();
  for (const [id, e] of all) {
    const here = enforcementIn(e, catalogId);
    if (here) out.set(id, here);
  }
  return out;
}

/**
 * "Not enforced yet — no engine attached": a stored rule that no engine
 * enforces, said in words beside the rule (never by colour alone). Renders
 * nothing for an enforced rule and for an unknown answer.
 */
export function NotEnforcedBadge({
  enforcement,
  appliesTo,
  className = "",
}: {
  enforcement: EnforcementAnswer | null | undefined;
  /** What the rule is written on: a reach of nothing reads differently for each. */
  appliesTo: "tag" | "path";
  className?: string;
}) {
  if (!enforcement || enforcement.enforcedBy > 0) return null;
  return (
    <span className={`inline-flex items-start gap-1 text-xs text-foreground ${className}`}>
      <AlertTriangle aria-hidden className="mt-0.5 size-3 shrink-0" />
      <span>
        <span className="font-medium">{NOT_ENFORCED_YET}</span>
        {` — ${notEnforcedReason(enforcement.notEnforced, appliesTo)}`}
      </span>
    </span>
  );
}

/**
 * One line for several stored rules shown together (the Effect strip's rules
 * fact): nothing while unknown or when every rule is enforced; the one rule's
 * reason; "2 of 3 rules: <reason>" when some are not; and, when their reasons
 * differ, the count with a pointer to each rule's page.
 */
export function NotEnforcedSummary({
  rules,
  enforcement,
  className = "",
}: {
  rules: { id: string; appliesTo: "tag" | "path" }[];
  enforcement: ReadonlyMap<string, RuleEnforcement> | null;
  className?: string;
}) {
  if (enforcement === null) return null;
  const off = rules.flatMap((r) => {
    const e = enforcement.get(r.id);
    return e && e.enforcedBy === 0 ? [{ e, appliesTo: r.appliesTo }] : [];
  });
  if (off.length === 0) return null;
  if (rules.length === 1) return <NotEnforcedBadge enforcement={off[0].e} appliesTo={off[0].appliesTo} className={className} />;
  const reasons = [...new Set(off.map((o) => notEnforcedReason(o.e.notEnforced, o.appliesTo)))];
  const count = off.length === rules.length ? "" : `${off.length} of ${rules.length} rules: `;
  const why = reasons.length === 1 ? reasons[0] : `${off.length} of ${rules.length} rules; each rule's page says why`;
  return (
    <span className={`inline-flex items-start gap-1 text-xs text-foreground ${className}`}>
      <AlertTriangle aria-hidden className="mt-0.5 size-3 shrink-0" />
      <span>
        <span className="font-medium">{NOT_ENFORCED_YET}</span>
        {` — ${reasons.length === 1 ? count : ""}${why}`}
      </span>
    </span>
  );
}
