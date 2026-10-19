import type { Bucket, MatchWord, NotDeterminedReason } from "@/api/generated/model";

/**
 * The words the hand-written policy views share (spec §4–§6): Data and People
 * (`HandWrittenSection`) and a classification's Hand-written tab. Every label
 * says what the server measured and no more: a match is a resource match,
 * never "grants" (a policy can be outranked, disabled or conditional), and an
 * item naming a group or a role reaches nobody on an engine (premise 4, Q1).
 */

export const BUCKET_WORD: Record<Bucket, string> = {
  allow: "Allow",
  deny: "Deny",
  allow_exception: "Allow exception",
  deny_exception: "Deny exception",
  mask: "Mask",
  row_filter: "Row filter",
};

const POLICY_TYPE_WORD: Record<string, string> = { access: "Access", masking: "Masking", rowfilter: "Row filter" };
export const policyTypeWord = (t: string) => POLICY_TYPE_WORD[t] ?? t;

/** Q2: the match level, as the Data pages word it. */
export const MATCH_WORD: Record<MatchWord, string> = {
  applies: "Applies to this object",
  part: "Applies to part of it",
  parent: "Set on a parent",
  unknown: "Could not be matched",
};

/** Q1 for an item summary (no user in view): a group or role item reaches nobody on an engine. */
export function namesNobodySentence(kind: "group" | "role", name: string): string {
  return kind === "group"
    ? `Names group ${name}. Engines resolve no groups, so on the engine it reaches nobody.`
    : `Names role ${name}. Engines resolve no roles, so on the engine it reaches nobody.`;
}

/** Q1 on a user's page (route B): the item names a group or role this user holds. */
export function namesHeldSentence(kind: "group" | "role", name: string): string {
  return kind === "group"
    ? `Names group ${name}, which this user is in. Engines resolve no groups, so on the engine it reaches nobody.`
    : `Names role ${name}, which this user holds. Engines resolve no roles, so on the engine it reaches nobody.`;
}

/** CL9: a deny-all-else policy, stated without claiming it decides. */
export const DENY_ALL_ELSE = "Denies every access its items do not allow (deny all else).";

/** Spec §4.2: why route A could not determine the policies reaching an object. */
export const NOT_DETERMINED: Record<NotDeterminedReason, string> = {
  no_engine_attached: "No engine is attached to this catalog, so no hand-written policy reaches it.",
  no_profile: "The engines attached to this catalog have no policy profile, so no hand-written policy reaches it.",
  catalog_not_in_profile:
    "No policy profile of the attached engines lists this catalog, so hand-written policies could not be matched to it.",
};

/** Without `policy:read` (no request is made). */
export const NEEDS_POLICY_READ =
  "Hand-written policies are read with the policy:read permission, which this account does not have.";
