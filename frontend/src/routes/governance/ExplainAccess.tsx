import { useState } from "react";
import { Link } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import {
  AlertTriangle,
  Ban,
  HelpCircle,
  KeyRound,
  Loader2,
  Search,
  Server,
  ShieldCheck,
} from "lucide-react";

import { usePermExplain } from "@/api/generated/perm/perm";
import { policyAdminListPrincipals } from "@/api/generated/policy-admin/policy-admin";
import type {
  Action,
  ExplainAccess as AccessVerdict,
  ExplainAttribution,
  ExplainProfile,
  ExplainResponse,
  ExplainTagDecline,
  ExplainTargetLevel,
  Scope,
  WireDecision,
  WireEngineType,
  WireGraphProblem,
  WireItemBucket,
  WirePluginVersion,
  WirePolicyType,
  WireServiceDefIssue,
  WireTagAttachmentProblem,
  WireTagNormalisationProblem,
  WireTagResourceProblem,
  WireUndecidableCause,
  WireUnmodelled,
} from "@/api/generated/model";
import { ApiError } from "@/api/client";
import { refFor } from "@/components/perm/PrincipalPicker";
import { ScopePicker } from "@/components/perm/ScopePicker";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge, type badgeVariants } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { VariantProps } from "class-variance-authority";

const EMPTY_SCOPE: Scope = { catalog_id: "" };

type BadgeVariant = NonNullable<VariantProps<typeof badgeVariants>["variant"]>;

/** A decision that names a deciding policy — everything but `Undecidable`, which has none. */
type DecidedDecision = Exclude<WireDecision, { kind: "undecidable" }>;

/** One spelling of each engine per API surface (SP4d-2a Task 5 review, I3): the
 *  wire says `trino` / `starrocks`, and only the DISPLAY differs. */
const ENGINE_LABEL: Record<WireEngineType, string> = {
  trino: "Trino",
  starrocks: "StarRocks",
};

const PLUGIN_LABEL: Record<WirePluginVersion, string> = {
  ranger_2_5_0: "Ranger 2.5.0",
  ranger_2_8_0: "Ranger 2.8.0",
  ranger_2_9_0: "Ranger 2.9.0",
};

/** Ranger's five item buckets, named the way an operator reading the policy would. */
const BUCKET_LABEL: Record<WireItemBucket, string> = {
  policy_items: "allow item",
  deny_policy_items: "deny item",
  allow_exceptions: "allow exception",
  deny_exceptions: "deny exception",
  data_mask_policy_items: "masking item",
  row_filter_policy_items: "row-filter item",
};

const POLICY_TYPE_LABEL: Record<WirePolicyType, string> = {
  access: "access",
  data_mask: "data-mask",
  row_filter: "row-filter",
};

/** `“name” (policy id 7)`, or `(policy id unknown)` — a decision can name a
 *  policy Ranger reported without an id at all (its `superuser`
 *  short-circuit reports `-1`), and inventing one would be worse. */
function policyRef(policyName: string, policyId?: number | null): string {
  return `“${policyName}” (policy id ${policyId ?? "unknown"})`;
}

/**
 * The construct a policy or envelope carries that `egret-ranger-eval` detected
 * but does not model, as a PHRASE — the wire mirrors every variant
 * exhaustively (`WireUnmodelled`) precisely so a page can say what it is
 * instead of printing a Debug string. `TagPolicies` carries its count as a JSON
 * number, so the count renders as a number inside a sentence.
 */
function unmodelledPhrase(u: WireUnmodelled): string {
  switch (u.kind) {
    case "validity_schedules":
      return "a validity schedule — a time window egret does not evaluate";
    case "policy_conditions":
      return "a policy-level condition egret does not evaluate";
    case "item_conditions":
      return `a condition on ${BUCKET_LABEL[u.bucket]} ${u.index}`;
    case "zone_name":
      return `the security zone “${u.zone_name}”`;
    case "security_zones":
      return "security zones";
    case "tag_policies":
      // ⚠ The `policies` arm is SUPERSEDED COPY kept alive only by the
      // exhaustive mirror, and it said "which egret does not evaluate yet"
      // until SP4d-2b Task 7 swept it. The evaluator evaluates the block as of
      // Task 3 and both of its envelope sweeps now SKIP
      // `TagPolicies(Policies(_))` by name, so this arm cannot be reached from
      // the backend at all; the arm stays because `explain_wire.rs` mirrors the
      // model exhaustively and a wildcard here is what that exhaustiveness
      // exists to forbid. The wording is therefore what it would mean if a new
      // code path ever did report it — a block that was NOT evaluated — and no
      // longer a promise about a feature that has shipped.
      return u.block.kind === "policies"
        ? `${u.block.count} tag ${u.block.count === 1 ? "policy" : "policies"} in a block that was not evaluated`
        : "a tag-policy block egret could not read";
    case "policy_deltas":
      return "incremental policy deltas";
    case "service_config":
      return "a service-config map, which can carry a super-user list";
    case "mask_expression_marker":
      return `a mask expression with a Ranger macro in it (${BUCKET_LABEL[u.bucket]} ${u.item_index}: ${u.expr})`;
    case "mask_condition_expr":
      return `a masking condition (${BUCKET_LABEL[u.bucket]} ${u.item_index})`;
    case "row_filter_expression_marker":
      return `a row filter with a Ranger macro in it (${BUCKET_LABEL[u.bucket]} ${u.item_index}: ${u.expr})`;
    case "resource_value_expression_marker":
      return `a Ranger macro in the ${u.resource_type} resource value “${u.value}”`;
    case "user_expression_marker":
      return `a Ranger macro in the user “${u.user}” (${BUCKET_LABEL[u.bucket]} ${u.index})`;
    case "explicit_deny":
      return `an explicit deny of ${u.access_type} (${BUCKET_LABEL[u.bucket]} ${u.index})`;
    case "delegate_admin":
      return `a delegated-admin item (${BUCKET_LABEL[u.bucket]} ${u.index})`;
  }
  // **No wildcard, and no string fallback either** (whole-branch review,
  // I-5). `explain_wire.rs`'s module doc spends 25 lines arguing that a
  // catch-all here "would convert a future fail-open defect in the evaluator
  // into a green build", and a whole pushback round was spent enforcing
  // exhaustive mirroring BY BUILD BREAK. A trailing `return "…"` undoes
  // exactly that: TypeScript already proves the switch above exhaustive
  // (removing the fallback produces no `tsc` diagnostic), so the string was
  // unreachable and its only effect was to let a new variant compile. SP4d-2b
  // adds variants to these very enums.
  const _never: never = u;
  return _never;
}

function graphProblemPhrase(p: WireGraphProblem): string {
  switch (p.kind) {
    case "no_sources_or_sinks":
      return "it has no root and no leaf";
    case "sink_is_not_a_valid_leaf":
      return `“${p.resource}” is a leaf the definition does not accept as one`;
    case "broken_parent_chain":
      return `“${p.resource}” has a broken parent chain`;
  }
  const _never: never = p;
  return _never;
}

function serviceDefIssuePhrase(i: WireServiceDefIssue): string {
  switch (i.kind) {
    case "unmodelled_matcher_class":
      return `resource “${i.resource}” uses the matcher class ${i.matcher}, which egret does not model`;
    case "quoted_case_sensitive":
      return `resource “${i.resource}” is quoted and case-sensitive`;
    case "chained_implied_grants":
      return `access type ${i.implying} implies ${i.implied}, which implies another in turn`;
    case "invalid_resource_graph":
      return `the ${POLICY_TYPE_LABEL[i.policy_type]} resource graph is invalid: ${graphProblemPhrase(i.reason)}`;
  }
  const _never: never = i;
  return _never;
}

/**
 * How far a cause reaches, when that is wider than the request being explained.
 *
 * **SP4d-2b Task 7 rewrote this note, and the rewrite is the deliverable rather
 * than a tidy-up.** SP4d-2a's browser gate (2026-09-29) measured that `explain`
 * passed `resolved_tags: None` unconditionally, so ONE tag-addressed rule or
 * clearance anywhere in the installation turned EVERY verdict on every catalog a
 * profile serves into `Undecidable` — all seven Trino answers in that fixture,
 * from two tag policies. **That sentence is now false.** `explain` resolves the
 * tags, and a profile whose tag block carries policies answers.
 *
 * What is left is narrower and comes in two shapes a steward has to be able to
 * tell apart, so they get two different notes — and the per-policy tag causes
 * get none, because a scope note on them would overstate them:
 *
 * | cause | reaches | and the thing to act on |
 * |---|---|---|
 * | `tags_unresolved` | every target in THIS CATALOG for `stored_fold_divergent`, and THIS REQUEST ALONE for `target_fold_divergent` — which is why `tagDeclineScope` is two notes and not one | whichever of the two `tag_resolution_declined` names, each with its own remedy (⚠ this row said "one stored tag assignment names no schema or no table" while FOUR states produced the cause — the fix-round-3 defect, surviving in a doc comment. There are two states now: migration 0177 made `stored_level_empty` and `stored_wildcard` unstorable) |
 * | `envelope_construct` / `policy_construct` with `tag_policies` | every target on THIS PROFILE | the tag download itself, which did not read |
 * | `tag_attachment_not_reproducible`, `tag_resource_not_reproducible`, `tag_policy_not_normalisable`, both arbitration causes | the tag or the policy the sentence already NAMES | nothing to add: the subject is in the sentence |
 */
/**
 * Why a tag policy's candidacy cannot be reproduced. The four shapes have four
 * different remedies, which is why the wire carries them apart rather than as
 * one sentence — see `UndecidableCause::TagResourceNotReproducible`.
 */
function tagResourceProblemPhrase(p: WireTagResourceProblem): string {
  switch (p.kind) {
    case "not_only_the_tag_resource":
      return "names a resource level other than `tag`";
    case "excludes":
      return "sets `isExcludes` on its tag resource";
    case "value_not_trie_prefix_exact":
      return `matches tags with the wildcard value “${p.value}”`;
    case "recursive":
      return "sets `isRecursive` on its tag resource";
  }
  const _never: never = p;
  return _never;
}

/**
 * What one resolved tag ATTACHMENT carries that egret cannot reproduce. Three
 * shapes, three different things for a steward to do — see
 * `UndecidableCause::TagAttachmentNotReproducible`. The first two say the tag
 * download carries a validity construct egret neither writes nor evaluates; the
 * third says the download's service resource is not a shape egret's own writer
 * can produce, which is a question about where the payload came from and not
 * something to edit in egret.
 */
function tagAttachmentProblemPhrase(p: WireTagAttachmentProblem): string {
  switch (p.kind) {
    case "options":
      return `carries the tag option${p.keys.length === 1 ? "" : "s"} ${p.keys.join(", ")}, which can restrict when the tag applies`;
    case "validity_periods":
      return `carries ${p.count} validity period${p.count === 1 ? "" : "s"}, which restrict when the tag applies`;
    case "resource_levels":
      // A comma list, not a slash path: the wire carries the `resourceElements`
      // KEY SET (sorted), and a slash would imply a hierarchy order the set does
      // not carry — `catalog/table` would read as a path that does not exist.
      return `is attached at the resource level${p.levels.length === 1 ? "" : "s"} ${p.levels.length === 0 ? "— none at all" : p.levels.join(", ")}, which is not a shape egret's own tag download writes`;
  }
  const _never: never = p;
  return _never;
}

/**
 * What a tag policy carries that the engine's TAG policy repository would
 * rewrite or reject on the way in — see
 * `UndecidableCause::TagPolicyNotNormalisable`.
 *
 * The third shape is not a variation on the first two, and the wording says so:
 * a namespaced access or mask type is something a steward EDITS, while an
 * absent mask type makes the plugin fail to build a policy engine from the
 * download at all. Reporting the second as "egret cannot decide this access"
 * would understate it — nothing the engine serves is being decided.
 */
function tagNormalisationProblemPhrase(p: WireTagNormalisationProblem): string {
  switch (p.kind) {
    case "namespaced_access_type":
      return `names the access type “${p.access_type}”, which the engine's plugin reads as a component-qualified name rather than a literal`;
    case "namespaced_mask_type":
      return `names the mask type “${p.mask_type}”, which the engine's plugin reads as a component-qualified name rather than a literal`;
    case "mask_type_absent":
      return "has a masking item with no mask type at all, which makes the engine's plugin fail while loading this policy download";
  }
  const _never: never = p;
  return _never;
}

/** The stored coordinate a decline is about, as a steward reads it: `sales.orders.email`,
 *  or `sales / orders / (the table itself)` for a table-level attachment.
 *
 *  ⚠ The `(none)` rendering for an empty SCHEMA or TABLE is gone with the states that
 *  produced it: migration 0177 makes an empty schema or table unstorable, so the only
 *  empty level that can reach here is `column`, which is 0081's table-level sentinel
 *  and has always had its own wording. */
function storedCoordinate(d: { schema: string; table: string; column: string }): string {
  return `${d.schema} / ${d.table} / ${d.column === "" ? "(the table itself)" : d.column}`;
}

const TARGET_LEVEL_LABEL: Record<ExplainTargetLevel, string> = {
  schema: "schema",
  table: "table",
  column: "column",
};

/**
 * Why egret declined to resolve this target's tags, as a sentence naming the thing
 * to fix.
 *
 * **⚠ Fix round 3 — this function is the fix for a measured defect, not a
 * refinement.** Round 1 wrote ONE sentence for this cause, "a tag assignment in
 * this catalog is stored with no schema or no table … re-store that assignment on a
 * table or a column", when `EmptyLevel` was the only state that produced it. Rounds
 * 1 and 2 added three more states and left the sentence alone, so a steward whose
 * catalog held `('sales','*','')` was told about a row with no table — a row that
 * does not exist — and offered a remedy already satisfied. The states have different
 * remedies, which is exactly why `ExplainTagDecline` carries them apart and why the
 * handler stopped discarding the variant.
 *
 * ⚠ **Two of the four arms are gone, and not because they were wrong.** Migration
 * 0177 adds a `CHECK` to `catalog_tag_assignments` that makes an empty schema or
 * table, and a `*` or `?` in any level, unstorable — so `stored_level_empty` and
 * `stored_wildcard` are states the backend can no longer report and the wire enum no
 * longer carries. A `case` for a `kind` that cannot arrive is the same defect as the
 * stale sentence above, one level removed.
 */
function tagDeclineSentence(d: ExplainTagDecline): string {
  switch (d.kind) {
    case "stored_fold_divergent":
      return `a tag assignment in this catalog is stored at ${storedCoordinate(d)}, which carries a character whose upper- and lower-case forms egret's database and the engine's matcher need not agree on. Rename that resource, or re-store the assignment without it`;
    case "target_fold_divergent":
      return `the ${TARGET_LEVEL_LABEL[d.level]} you asked about, “${d.value}”, carries a character whose upper- and lower-case forms egret's database and the engine's matcher need not agree on. Nothing is stored wrongly — re-ask for the resource's exact name`;
  }
  // No wildcard and no string fallback, for the reason spelled out on
  // `unmodelledPhrase`: an arm added to the wire enum and not here must break the
  // build, not render an empty sentence.
  const _never: never = d;
  return _never;
}

/**
 * How far one decline reaches — and the two answers are genuinely different, which
 * is why this is not one note.
 *
 * ⚠ Round 1's note said "every target in this catalog answers the same way" for ALL
 * of them. Measured false for `target_fold_divergent`: that state is a property of
 * the REQUEST, so every other target in the catalog answers normally, and telling a
 * steward otherwise sends them looking at stored data that is fine.
 */
function tagDeclineScope(d: ExplainTagDecline): string {
  switch (d.kind) {
    case "stored_fold_divergent":
      return "This is not specific to your target: egret declines to resolve tags for the whole catalog while that assignment is stored this way, so every target in this catalog answers the same way until it changes.";
    case "target_fold_divergent":
      return "This one IS specific to what you asked: nothing is stored wrongly, and every other target in this catalog answers normally.";
  }
  const _never: never = d;
  return _never;
}

function causeScopeNote(c: WireUndecidableCause, declined?: ExplainTagDecline): string | null {
  if (c.kind === "tags_unresolved") {
    // ⚠ No note at all when the reason is absent, rather than round 1's
    // per-catalog claim: without `tag_resolution_declined` the REACH is unknown,
    // and one of the two states is per-request. The backend sends the two
    // together — `tag_resolution_declined` is `Some` exactly when the handler
    // passed `resolved_tags: None`, which is the only production state raising
    // this cause — so the absent case is a payload egret does not produce.
    return declined ? tagDeclineScope(declined) : null;
  }
  const construct =
    c.kind === "envelope_construct" || c.kind === "policy_construct" ? c.construct : null;
  if (construct?.kind !== "tag_policies") return null;
  // ⚠ Worded for BOTH blocks it fires for (fix round 1, Low 4). It ended "until
  // that download can be read", which is true of `unreadable` and false of
  // `policies` — a block that WAS read. One note, two blocks: say the part that
  // holds for both (it is a property of the download, not of the target) and
  // leave the specific remedy to the sentence above, which names the block.
  return "This is not specific to your target: it is a property of the tag-policy download this profile received, so every access on every catalog this profile serves answers the same way while that download is in this state.";
}

/**
 * One named cause, as a sentence a steward can act on.
 *
 * **Every cause is rendered, never just the first** — `why` is a list because
 * one request can be blocked from a defensible answer by several independent
 * things, and collapsing it discards exactly what the three-valued contract
 * exists to say.
 */
function causeSentence(c: WireUndecidableCause, declined?: ExplainTagDecline): string {
  switch (c.kind) {
    case "envelope_construct":
      return `The downloaded policy set carries ${unmodelledPhrase(c.construct)} — so this access cannot be decided.`;
    case "policy_construct":
      return `Ranger policy ${policyRef(c.policy_name, c.policy_id)} carries ${unmodelledPhrase(c.construct)} — so this access cannot be decided.`;
    case "service_def":
      return `This profile's service definition cannot be evaluated around: ${serviceDefIssuePhrase(c.issue)}.`;
    case "resource_owner_unknown":
      return `Ranger policy ${policyRef(c.policy_name, c.policy_id)} decides by the resource's owner, and egret does not record a table owner.`;
    case "super_users_unknown":
      return "The engine's super-user list is unknown, and Ranger checks super users before any policy at all, so no verdict here could be relied on.";
    case "pseudo_access_type":
      return `“${c.access_type}” is a pseudo access type Ranger expands internally, not one the downloaded policy set decides directly.`;
    case "access_type_case_mismatch":
      return `Ranger policy ${policyRef(c.policy_name, c.policy_id)} names the access type “${c.access_type}” in a different case than the service definition does.`;
    case "mask_expression":
      return `Ranger policy ${policyRef(c.policy_name, c.policy_id)} masks with the expression ${c.expr} (item ${c.item_index}), which egret does not evaluate.`;
    case "row_filter_expression":
      return `Ranger policy ${policyRef(c.policy_name, c.policy_id)} filters rows with the expression ${c.expr} (item ${c.item_index}), which egret does not evaluate.`;
    case "row_filter_double_brackets":
      return `Ranger policy ${policyRef(c.policy_name, c.policy_id)} has a row filter with doubled brackets (item ${c.item_index}: ${c.expr}), which Ranger itself reads ambiguously.`;
    case "eval_order_ambiguous":
      return `Several policies could decide this and their evaluation order is ambiguous: ${c.policy_names.join(", ")} (policy ids ${c.policy_ids.map((id) => id ?? "unknown").join(", ")}).`;
    case "engine_mismatch":
      return `This profile declares ${ENGINE_LABEL[c.declared]} but carries the service definition “${c.service_def_name}”, whose resource levels are a different vocabulary.`;
    case "plugin_version_mismatch":
      // `declared` is the version egret reads the bound engine at: for an
      // external engine, the one its operator declared; for a provisioned one,
      // the one its exact image ships. The profile itself declares nothing.
      return `The ${ENGINE_LABEL[c.engine]} engine bound to this profile runs ${PLUGIN_LABEL[c.declared]} (the version its image ships, or for an external engine the version its operator declared), and egret's evaluator has not been verified against that version, so no verdict here could be relied on.`;
    case "tags_unresolved": {
      // ⚠ Rewritten TWICE. Task 7 changed the cause's meaning (it used to be
      // unconditional, because `explain` never resolved tags); fix round 3 moved
      // the REASON out of this sentence and into `tag_resolution_declined`,
      // because by then four states produced this cause and this sentence named
      // only the first of them (two states since migration 0177). The distinction
      // the cause has always carried — "declined to resolve" is not "there are
      // none", since an empty set would have been an ANSWER — stays here, where it
      // is true of every one of them.
      const head =
        "This profile's tag service carries policies that would be evaluated against this resource's tags, and egret declined to resolve them";
      const tail =
        "“Declined to resolve” is not the same as there being none, so no verdict here could be relied on.";
      return declined ? `${head}: ${tagDeclineSentence(declined)}. ${tail}` : `${head}. ${tail}`;
    }
    case "tag_resource_not_reproducible":
      return `Tag policy ${policyRef(c.policy_name, c.policy_id)} ${tagResourceProblemPhrase(c.problem)}, so egret cannot tell which tags the engine would run it for.`;
    // ⚠ **This cause and `tag_sub_result_split_by_resource_policy` below are raised
    // ONLY from the evaluator's `decide_sub` / `evaluate_sub` — the MASK and
    // ROW-FILTER entry points — and `explain` calls neither** (whole-branch review,
    // L3; `evaluate_sub` has no caller in `egret-perm-compile`). So these two
    // sentences, and the tests asserting them, describe states this page cannot
    // currently be shown. They stay for the reason `explain_wire.rs` mirrors every
    // variant: an arm added to the wire enum and not here must break the build, and
    // the day `explain` reports a mask verdict these are the two sentences it will
    // need. The concrete form of the scope finding at the top of the plan — the
    // whole tag mask / row-filter half of this package is reachable from no product
    // surface.
    case "tag_sub_policy_is_deny_capable":
      return `Tag policy ${policyRef(c.policy_name, c.policy_id)} produced a mask or row filter and also carries deny items, and Ranger can then report this policy's id with a later policy's mask.`;
    case "resource_candidacy_not_reproducible":
      return `Whether Ranger's policy trie returns ${policyRef(c.policy_name, c.policy_id)} for this resource decides the answer, and egret cannot reproduce that filter for this policy's resource shape.`;
    case "tag_attachment_not_reproducible":
      return `The tag “${c.tag}” on this resource ${tagAttachmentProblemPhrase(c.problem)}, so egret cannot tell whether the engine would evaluate its tag policies here.`;
    case "tag_sub_result_split_by_resource_policy":
      return `Tag policy ${policyRef(c.tag_policy_name, c.tag_policy_id)} produced a mask or row filter and the higher-priority Ranger policy ${policyRef(c.resource_policy_name, c.resource_policy_id)} then produced one too, so Ranger reports the tag policy's id with the other policy's mask.`;
    case "tag_deny_reopened_differs_by_plugin_version": {
      // Two states, two different remedies — and the sentence used to give the
      // second one in both cases (fix round 1, IMPORTANT 4). `explain` now always
      // declares the plugin version, so "record which plugin version its engine is
      // running" was advice a steward had already taken and could not take again.
      const shape = `Tag policy ${policyRef(c.tag_policy_name, c.tag_policy_id)} denies this and the higher-priority Ranger policy ${policyRef(c.resource_policy_name, c.resource_policy_id)} allows it, and Ranger 2.5.0 and 2.8.0 resolve that combination differently`;
      return c.plugin_version
        ? `${shape}. This engine runs ${PLUGIN_LABEL[c.plugin_version]}, which keeps the tag deny here where 2.8.0 would reopen it — egret does not yet reproduce 2.5.0's side of that split, so it will not guess. Levelling the two priorities, or removing one of the two policies, makes this decidable.`
        : `${shape}, so it cannot be answered until this profile records which plugin version its engine is running.`;
    }
    // ⚠ **"tag policies" is correct again, and the round trip is the lesson.** The
    // concurrent Critical first rode this wire tag as a documented ALIAS, which made
    // this noun false for half the states the sentence was shown for — so it was
    // briefly widened to "Several policies". The cause has its own variant and its
    // own sentence now (below), so this one is once more raised ONLY where both
    // policies are tag policies, and the specific word is the honest one. Widening
    // a sentence to cover a state that should have been split is a different defect
    // from leaving it narrow: it costs every OTHER reader precision.
    case "tag_priority_arbitration_not_modelled":
      return `Several tag policies at different priorities each decide this, and which one the engine keeps depends on an arbitration egret does not reproduce: ${c.policy_names
        .map((name, i) => `${policyRef(name, c.policy_ids[i] ?? null)} at priority ${c.priorities[i]}`)
        .join(", ")}. Giving them the same priority makes this decidable.`;
    // ⚠ **Its own sentence, and the facts in it are PROVED rather than sampled.**
    // The refusal fires when a tag ALLOW is in force and a RESOURCE policy at a
    // strictly higher priority also allows. That the tag side is the allow is not an
    // assumption: `allowed` becomes true at exactly one site, which also sets
    // `priority`, and evaluation is priority-DESCENDING, so any later candidate has
    // `found.priority <= priority`. So the sentence can name which policy is which,
    // which `tag_priority_arbitration_not_modelled`'s positional lists cannot.
    //
    // It says "rarely" because that is measured: 0 of 1148 composed-random cases and
    // 10 of 4000 overall. A sentence that implied this was common would send a
    // steward looking for a widespread misconfiguration.
    case "access_priority_composite_not_modelled":
      return `Two policies allow this — the tag policy ${policyRef(c.tag_policy_name, c.tag_policy_id)} at priority ${c.tag_priority}, and the resource policy ${policyRef(c.resource_policy_name, c.resource_policy_id)} at the higher priority ${c.resource_priority} — and Ranger composes the two through a priority rule egret does not reproduce, so egret will not tell you which one the engine reports as the decider. Rarely reached. Giving the two the same priority makes this decidable.`;
    case "tag_policy_not_normalisable":
      return c.problem.kind === "mask_type_absent"
        ? `Tag policy ${policyRef(c.policy_name, c.policy_id)} ${tagNormalisationProblemPhrase(c.problem)} — so the engine is serving no decisions from this download at all, and this one cannot be answered either.`
        : `Tag policy ${policyRef(c.policy_name, c.policy_id)} ${tagNormalisationProblemPhrase(c.problem)}, so egret cannot tell what the engine would actually evaluate.`;
  }
  const _never: never = c;
  return _never;
}

/**
 * One decision's rendering. Three-valued, and the three are not shades of one
 * another: `Undecidable` is neither an allow nor a refusal, and gets its own
 * colour for that reason. `no_policy_matched` and the two denies stay apart
 * the way `no_grant_matched` and `explicit_deny` did — nobody said yes is a
 * different administrative problem from somebody saying no, and Ranger's own
 * Access audit collapses both into a single `-`.
 */
function decisionMeta(d: WireDecision): {
  label: string;
  hint: string;
  variant: BadgeVariant;
  icon: typeof ShieldCheck;
} {
  if (d.kind === "allowed") {
    return {
      label: "Allowed",
      hint: "The policy set this engine downloads allows this access.",
      variant: "success",
      icon: ShieldCheck,
    };
  }
  if (d.kind === "undecidable") {
    return {
      label: "Undecidable",
      hint: "Neither allowed nor refused: something stands between the evaluator and a defensible answer, and it is named below rather than guessed at in either direction.",
      variant: "processing",
      icon: AlertTriangle,
    };
  }
  switch (d.reason) {
    case "deny_policy_item":
      return {
        label: "Refused by a deny item",
        hint: "Somebody forbade this: a deny item in the downloaded policy set matches this subject and resource.",
        variant: "destructive",
        icon: Ban,
      };
    case "deny_all_else":
      return {
        label: "Refused by deny-all-else",
        hint: "The deciding policy refuses everything it does not explicitly allow, and nothing in it allowed this.",
        variant: "destructive",
        icon: Ban,
      };
    case "no_policy_matched":
      return {
        label: "No policy matched",
        hint: "Nobody granted this — not the same as somebody forbidding it. Ranger’s own audit shows “-” for exactly this case. There is no deciding policy to open: the evaluator names none, because none matched.",
        variant: "warning",
        icon: HelpCircle,
      };
  }
  return {
    label: "Refused",
    hint: "The downloaded policy set refuses this access.",
    variant: "destructive",
    icon: Ban,
  };
}

/**
 * A deciding policy egret's compiler does not own — and it is reported, never
 * dropped. Four live categories reach here: the `egret-admin`/`all`
 * admin-all-access hierarchies egret seeds onto every generated profile,
 * `egret-engine-mgmt`'s delegator plumbing, operator-written raw JSON, and
 * Ranger's own `superuser` short-circuit. Dropping any of them would recreate,
 * inside the new mechanism, the blindness this package removes: the old page
 * could not see a seeded admin policy at all and answered “no grant matched”
 * while the engine allowed.
 *
 * There is no link, because there is no governance page for a policy the
 * compiler did not mint.
 */
function RawPolicy({
  policyName,
  policyId,
}: {
  policyName: string;
  policyId?: number | null;
}) {
  // `egret_policy::default_policies` mints exactly these two prefixes
  // (`seed_admin_all_access` → `egret-admin - …`, `seed_default_trino_policies`
  // → `all - …`). A seeded admin policy is the EXPECTED explanation for an
  // administrator, so it must not be dressed as a finding.
  //
  // **The two families are seeded by different paths and the copy below must
  // not claim one origin for both** (whole-branch review, Minor). Auto-bind's
  // thirteen `egret-admin` hierarchies go onto every GENERATED profile;
  // `seed_default_trino_policies`' `all - …` family only ever runs on a
  // NON-generated one, so naming auto-bind for it would say the opposite of
  // the truth.
  const seeded = policyName.startsWith("egret-admin") || policyName.startsWith("all - ");
  const seededByAutoBind = policyName.startsWith("egret-admin");
  return (
    <div
      data-testid="raw-policy"
      data-kind={seeded ? "seeded-admin" : "unowned"}
      className="space-y-1"
    >
      <Badge variant={seeded ? "secondary" : "outline"} className="gap-1">
        {seeded ? <ShieldCheck className="h-3 w-3" /> : <Server className="h-3 w-3" />}
        {seeded ? "Seeded admin policy" : "Outside the permission model"}
      </Badge>
      <p className="text-xs text-muted-foreground">
        {seeded
          ? seededByAutoBind
            ? "egret seeds this policy itself, outside the permission model: auto-bind puts thirteen admin-all-access hierarchies onto every generated profile, granting its platform admins all access. For an administrator this is the expected explanation."
            : "egret seeds this policy itself, outside the permission model: it belongs to the default Trino policy set, which is seeded onto a profile egret did not generate. For an administrator this is the expected explanation."
          : "egret’s compiler did not mint this policy, so it is outside the permission model and has no governance page to open. It is reported by its Ranger name and id, because it decided this access."}
      </p>
      <p className="text-xs text-muted-foreground">
        Ranger policy <span className="font-mono">{policyName}</span>, id{" "}
        <span className="font-mono">{policyId ?? "unknown"}</span>
      </p>
    </div>
  );
}

/**
 * The `perm` object the deciding policy's Ranger NAME maps back to — a Grant, a
 * Rule, or a classification Clearance — or, when the name carries no model
 * attribution at all, the raw policy itself (see [`RawPolicy`]).
 *
 * The `?grant=` / `?rule=` deep links live here, per decision, because under
 * the per-profile shape there is no page-level list of reasons to hang them on.
 * SP4b-1's PATCH keeps a grant/rule id stable across an edit; these links would
 * rot on a delete-and-recreate.
 */
function DecidedBy({
  policyName,
  policyId,
  attribution,
}: {
  policyName: string;
  policyId?: number | null;
  attribution?: ExplainAttribution | null;
}) {
  const ranger = (
    <span className="text-muted-foreground">
      Ranger policy <span className="font-mono">{policyName}</span>, id{" "}
      <span className="font-mono">{policyId ?? "unknown"}</span>
    </span>
  );

  if (!attribution) {
    return <RawPolicy policyName={policyName} policyId={policyId} />;
  }

  if (attribution.kind === "grant") {
    return (
      <div className="flex flex-wrap gap-x-3 gap-y-1 text-xs">
        <Link
          to={`/governance/data-permissions?grant=${encodeURIComponent(attribution.grant_id)}`}
          className="text-primary hover:underline"
        >
          View grant {attribution.grant_id}
        </Link>
        {ranger}
      </div>
    );
  }

  if (attribution.kind === "rule") {
    return (
      <div className="flex flex-wrap gap-x-3 gap-y-1 text-xs">
        <Link
          to={`/governance/mask-filter-rules?rule=${encodeURIComponent(attribution.rule_id)}`}
          className="text-primary hover:underline"
        >
          View rule {attribution.rule_id}
        </Link>
        {/* A per-column StarRocks rule expands ONE `perm.rules` row into N
            policies, so the locator is the only thing that says which of the N
            decided this resource. */}
        {attribution.locator && (
          <span data-testid="rule-locator" className="text-muted-foreground">
            expansion <span className="font-mono">{attribution.locator}</span>
          </span>
        )}
        {ranger}
      </div>
    );
  }

  // The engine-wide `create materialized view` deny (view governance A3). It
  // belongs to no single tag, so there is no tag or role to name.
  if (attribution.kind === "materialized_view_deny") {
    return (
      <div className="space-y-1 text-xs">
        <p className="text-muted-foreground">
          The engine-wide materialized-view deny: only users cleared for every classification
          with a column on this engine may create a materialized view on it.
        </p>
        {ranger}
      </div>
    );
  }

  // A classification gate. D10's pre-existing defect was here: `clearance` had
  // no entry in the reason map at all, so it rendered the raw wire string with
  // "Unrecognized reason class." and dropped the gating tag and the clearing
  // roles — the two facts a steward most needs. Both are on the wire, so both
  // are rendered.
  //
  // `null` and a missing key both mean "no gate was looked up": the server
  // always serialises the key, but the generated type is optional because the
  // field is nullable, and a reader of this block should not have to know
  // which. They collapse here, once, so the three sentences below each test
  // one thing.
  const clearingRoles = attribution.required_roles ?? null;
  return (
    <div data-testid="clearance-attribution" className="space-y-1">
      <Badge variant="warning" className="gap-1">
        <KeyRound className="h-3 w-3" />
        Clearance required
      </Badge>
      <div className="flex flex-wrap gap-x-3 gap-y-1 text-xs">
        <span className="text-muted-foreground">
          Gating tag{" "}
          {attribution.tag ? (
            <span className="font-mono">{attribution.tag}</span>
          ) : (
            <span className="italic">unknown — the tag row is gone until the next recompile</span>
          )}
        </span>
        {attribution.locator && (
          <span className="text-muted-foreground">
            expansion <span className="font-mono">{attribution.locator}</span>
          </span>
        )}
        {ranger}
      </div>
      {/* **Browser-gate Finding 2, now answered by the wire.** This used to
          branch on `attribution.tag === null` because an empty
          `required_roles` meant either "the gate names no role" or "no gate
          could be looked up", and only the OTHER field could tell them apart.
          With the tag unresolved the page asserted "it declares only groups" —
          contradicting the sentence directly above it, and false for the gate
          the browser gate measured, which declared `sp4d2a_cleared`. The field
          is now `Option`: `attribution_for` in
          `crates/egret-perm-compile/src/routes.rs` sends `null` on the one
          path that cannot look a gate up, and a list on the one that did. So
          all three sentences read one field, and "unknown" can no longer be
          inferred from a second. */}
      {clearingRoles === null ? (
        <p className="text-xs text-muted-foreground">
          The clearing roles are unknown: no gate could be looked up for this clearance, so
          this is unknown rather than none — the engine is still enforcing the compilation
          as it was.
        </p>
      ) : clearingRoles.length > 0 ? (
        <p className="text-xs text-muted-foreground">
          Cleared by holding any one of{" "}
          {clearingRoles.map((r, i) => (
            <span key={r}>
              {i > 0 && ", "}
              <span className="font-mono">{r}</span>
            </span>
          ))}
          .
        </p>
      ) : (
        <p className="text-xs text-muted-foreground">
          No clearing role is recorded for this tag: a gate that declares only groups looks
          like this — no engine egret provisions can see a group — and so does a gate row
          that has already been deleted.
        </p>
      )}
    </div>
  );
}

/**
 * Does this decision name a policy at all?
 *
 * **`no_policy_matched` does not, by construction** — `egret-ranger-eval`
 * returns `policy_name: String::new()` and `policy_id: None` for it
 * (`eval.rs:759-763`), because nothing matching is what the verdict MEANS.
 * Routing it through the decider block was browser-gate Finding 1: it printed
 * "Outside the permission model … because it decided this access. / Ranger
 * policy , id unknown" — asserting that a policy decided when none did,
 * calling that non-existent policy hand-written, and leaving a hole where the
 * name goes, which sends a steward into raw Ranger hunting for it. The same
 * CLASS as D10's `clearance_required` defect: a decision variant rendered by a
 * branch written for a different variant.
 */
function namesADecidingPolicy(d: DecidedDecision): boolean {
  return d.kind === "allowed" || d.reason !== "no_policy_matched";
}

/** One access type's verdict inside one profile. A `write` explain is several
 *  of these and they are never collapsed: Ranger has no `write`, and an
 *  operator who can insert but not delete has to be able to see that. */
function AccessRow({
  verdict,
  declined,
}: {
  verdict: AccessVerdict;
  /** `ExplainResponse::tag_resolution_declined`, which is per RESPONSE because it
   *  is a property of the target and the catalog rather than of this access. */
  declined?: ExplainTagDecline;
}) {
  const meta = decisionMeta(verdict.decision);
  const Icon = meta.icon;
  return (
    <li
      data-testid="explain-access"
      className="flex flex-col gap-1.5 rounded-md border border-border p-3 sm:flex-row sm:items-start sm:gap-3"
    >
      <div className="flex w-fit shrink-0 items-center gap-2">
        <span className="font-mono text-xs text-muted-foreground">{verdict.access_type}</span>
        <Badge data-testid="decision-badge" variant={meta.variant} className="gap-1">
          <Icon className="h-3 w-3" />
          {meta.label}
        </Badge>
      </div>
      <div className="min-w-0 flex-1 space-y-1.5">
        <p className="text-sm text-muted-foreground">{meta.hint}</p>
        {verdict.decision.kind === "undecidable" ? (
          <ul data-testid="undecidable-causes" className="space-y-1">
            {verdict.decision.why.map((cause, i) => (
              <li
                key={`${cause.kind}-${i}`}
                data-testid="undecidable-cause"
                className="space-y-1 text-xs text-muted-foreground"
              >
                {/* Two elements, two test ids, because they are two claims: the
                    SENTENCE says what could not be decided and the NOTE says how
                    far that reaches. A mutation that emptied one of them survived
                    a suite asserting both through the <li> (SP4d-2b Task 7), which
                    is the "harness can satisfy your assertion" shape — the
                    containing node holds the other string. */}
                <p data-testid="undecidable-cause-sentence">
                  {causeSentence(cause, declined)}
                </p>
                {causeScopeNote(cause, declined) && (
                  <p data-testid="undecidable-cause-scope" className="italic">
                    {causeScopeNote(cause, declined)}
                  </p>
                )}
              </li>
            ))}
            {verdict.decision.why.length === 0 && (
              <li className="text-xs text-muted-foreground">
                No cause was named, which should not happen: an undecidable verdict exists to
                say what it could not decide.
              </li>
            )}
          </ul>
        ) : (
          namesADecidingPolicy(verdict.decision) && (
            <DecidedBy
              policyName={verdict.decision.policy_name}
              policyId={verdict.decision.policy_id}
              attribution={verdict.attribution}
            />
          )
        )}
      </div>
    </li>
  );
}

/**
 * One profile's block. Per profile rather than once overall because a single
 * answer cannot be right for two: the same `perm` grant compiles into two
 * different policy sets, the engines do not share the resource vocabulary the
 * request is built from (`schema` vs `database`), and their access-type
 * vocabularies only partly overlap.
 */
function ProfileBlock({
  profile,
  declined,
}: {
  profile: ExplainProfile;
  declined?: ExplainTagDecline;
}) {
  return (
    <section
      data-testid="explain-profile"
      aria-label={`Profile ${profile.service_name}`}
      className="space-y-2 rounded-lg border border-border p-3 md:p-4"
    >
      <div className="flex flex-wrap items-center gap-2">
        <Server className="h-4 w-4 text-muted-foreground" />
        <h3 className="text-sm font-semibold">{profile.service_name}</h3>
        <Badge variant="outline">{ENGINE_LABEL[profile.engine]}</Badge>
        <span className="text-xs text-muted-foreground">service id {profile.service_id}</span>
      </div>
      <p className="text-xs text-muted-foreground">
        Evaluated over the policy set this profile’s plugin downloads.
      </p>
      <ul className="space-y-2">
        {profile.accesses.map((verdict) => (
          <AccessRow key={verdict.access_type} verdict={verdict} declined={declined} />
        ))}
        {/* Final review V-1: StarRocks asks no access decision for a column
            whose deciding mask is MASK_NULL, from whichever policy, so there is
            no verdict to show, only the mask below. Explain does not know the
            query's other columns, so the query-level consequence is stated. */}
        {profile.masked_read_skips_access && (
          <li
            data-testid="explain-access-not-checked"
            className="rounded-md border border-border p-3 text-sm text-muted-foreground"
          >
            This column is not access-checked on StarRocks: its mask is MASK_NULL, so the read
            returns NULL for it, whoever denies it. The query as a whole is still refused unless the
            reader may select another column it reads; when it selects only such columns, StarRocks
            checks the table’s first unmasked column (measured: id, on a partly classified Iceberg table) or{" "}
            <span className="font-mono text-xs">___count___</span> for{" "}
            <span className="font-mono text-xs">count(*)</span>; when no column is unmasked, on an
            Iceberg table it checks the hidden <span className="font-mono text-xs">_file</span> /{" "}
            <span className="font-mono text-xs">_pos</span> columns. Measured on StarRocks 4.1.6, for
            the clearance’s mask and for a hand-written MASK_NULL; Trino checks access on a masked
            column.
          </li>
        )}
        {profile.accesses.length === 0 && !profile.masked_read_skips_access && (
          <li className="text-sm text-muted-foreground">No access type was evaluated.</li>
        )}
      </ul>
      {profile.mask && <MaskLine mask={profile.mask} />}
      {/* Fix round 1 (m-5), V-1: StarRocks checks no access on a MASK_NULL
          column, so with the mask unknown the access verdict above may not apply. */}
      {profile.mask?.decision.kind === "undecidable" && (
        <p data-testid="explain-mask-provisional" className="text-xs text-muted-foreground">
          The access verdict above is provisional: if StarRocks applies a MASK_NULL mask here, it
          does not check this access.
        </p>
      )}
      {/* Task 12, M-4: the materialized-view deny is engine-wide, so the page
          states the rule instead of a verdict for one database. */}
      {profile.engine === "starrocks" && (
        <p data-testid="explain-mv-rule" className="text-xs text-muted-foreground">
          Materialized views: when this profile holds a classified column, StarRocks refuses{" "}
          <span className="font-mono">create materialized view</span> in every database to anyone
          not cleared for every classification on the profile. Explain does not answer it per
          database.
        </p>
      )}
      <ProfileTags profile={profile} declined={declined} />
    </section>
  );
}

/**
 * The masked-read verdict for the target column (StarRocks, view governance
 * A2). Plain on purpose: the DG frontend is redesigned later as its own spec.
 */
function MaskLine({ mask }: { mask: NonNullable<ExplainProfile["mask"]> }) {
  const d = mask.decision;
  return (
    <div className="space-y-1">
      <MaskSentence mask={mask} />
      {/* Final review V-1 / Task 10 M4: any policy's MASK_NULL can decide the
          skip, so the steward sees which perm object (or raw policy) it was. */}
      {d.kind === "masked" && (
        <div data-testid="explain-mask-decided-by">
          <DecidedBy
            policyName={d.result.policy_name}
            policyId={d.result.policy_id}
            attribution={mask.attribution}
          />
        </div>
      )}
    </div>
  );
}

function MaskSentence({ mask }: { mask: NonNullable<ExplainProfile["mask"]> }) {
  const d = mask.decision;
  return (
    <p data-testid="explain-mask" className="text-xs text-muted-foreground">
      Mask on {mask.access_type}:{" "}
      {d.kind === "masked" && d.result.mask_type === "MASK_NONE" ? (
        <>
          not transformed (<span className="font-mono">MASK_NONE</span>, Ranger policy{" "}
          <span className="font-mono">{d.result.policy_name}</span>)
        </>
      ) : d.kind === "masked" && d.result.mask_type === "MASK_NULL" ? (
        <>
          returns NULL (<span className="font-mono">MASK_NULL</span>, Ranger policy{" "}
          <span className="font-mono">{d.result.policy_name}</span>)
        </>
      ) : d.kind === "masked" ? (
        <>
          masked to <span className="font-mono">{d.result.mask_type}</span> by Ranger policy{" "}
          <span className="font-mono">{d.result.policy_name}</span>
        </>
      ) : d.kind === "no_mask" ? (
        "no mask applies"
      ) : (
        `undecidable (${d.why.map((c) => c.kind).join(", ")})`
      )}
    </p>
  );
}

/**
 * The tag set THIS profile's verdicts could have been decided by.
 *
 * **⚠ Whole-branch review H1 / H2 / H3 — this block used to render ONCE, below
 * every profile, from a response-level field.** All three Importants were that
 * one arrangement, and each was measured:
 *
 * - **H3**, reachable with no StarRocks, no second profile and no tag service: a
 *   catalog nothing serves rendered "Nothing is enforcing" and "Tags that decide
 *   this request — pii" on one screen, with no verdicts in existence at all. A
 *   sibling catalog's assignment on the same warehouse resolves for the target
 *   because `catalog_tag_assignments` has no catalog column.
 * - **H1**: a catalog served by one profile with a tag service and one without
 *   made the page claim, once, that these tags decide this request — false for
 *   the second.
 * - **H2**: a StarRocks profile given a `tagService` would list tags as deciding
 *   while the engine's enricher discards all of them.
 *
 * Rendering it per profile is the LEVEL fix: the server has no response-level tag
 * set any more, so `no_profile_serves_the_catalog` cannot carry one and a fourth
 * instance of the class has nowhere to appear. Three sentence edits would have
 * left that open.
 *
 * `tag_policies_in_download === 0` is why a set can be empty while the column is
 * tagged — the question H1's steward is otherwise left holding — and it is the
 * server's own narrowing predicate, not a second rule invented here.
 */
function ProfileTags({
  profile,
  declined,
}: {
  profile: ExplainProfile;
  declined?: ExplainTagDecline;
}) {
  const noTagPolicies = profile.tag_policies_in_download === 0;
  return (
    <div className="space-y-1.5 border-t border-border pt-2">
      {/* A heading, not a form label: nothing here is a control. */}
      <p className="text-xs font-medium leading-none">Tags that could decide here</p>
      {declined ? (
        <p data-testid="profile-tags-declined" className="text-xs text-muted-foreground">
          Not resolved — see the reason above. This is not the same as there being none.
        </p>
      ) : noTagPolicies ? (
        <p data-testid="profile-tags-no-policies" className="text-xs text-muted-foreground">
          None: this profile’s download carries no tag policies, so no tag decided anything
          here — whatever is stored on the resource.
        </p>
      ) : profile.tags.length > 0 ? (
        <div className="flex flex-wrap gap-1">
          {profile.tags.map((t) => (
            <Badge
              key={`${t.name}-${String(t.attached_at_column)}`}
              data-testid="profile-tag"
              variant="outline"
            >
              {t.name}
              <span className="ml-1 font-normal text-muted-foreground">
                {t.attached_at_column ? "on this column" : "on the table"}
              </span>
            </Badge>
          ))}
        </div>
      ) : (
        <p data-testid="profile-tags-none" className="text-xs text-muted-foreground">
          None.
        </p>
      )}
    </div>
  );
}

/**
 * The subject's single `user` field — a search over the same principal
 * source `PrincipalPicker` uses (`policyAdminListPrincipals`, kind `user`).
 * Stores the allocated `username`, never the email `list_principals` also
 * returns and never a client-side guess — via `PrincipalPicker`'s own `refFor`,
 * imported rather than re-implemented (design spec: "a user's `ref` is the bare
 * username, never an email"). Typing a value directly is also accepted (not every operator
 * knows a name to search for), so a principal with no allocated username
 * is shown but not selectable, exactly as `PrincipalPicker` shows it.
 */
function SubjectUserField({
  value,
  onChange,
}: {
  value: string;
  onChange: (username: string) => void;
}) {
  const [open, setOpen] = useState(false);

  const suggestions = useQuery({
    queryKey: ["perm-principals", "user", value],
    queryFn: async () => {
      const res = await policyAdminListPrincipals({ q: value, kind: "user" });
      return res.status === 200 ? res.data : [];
    },
    enabled: open,
    retry: false,
  });

  return (
    <div className="relative space-y-1.5">
      <Label htmlFor="explain-user">User</Label>
      <Input
        id="explain-user"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onFocus={() => setOpen(true)}
        onBlur={() => setTimeout(() => setOpen(false), 150)}
        placeholder="Search users…"
        autoComplete="off"
      />
      {open && (
        <div className="absolute z-10 mt-1 max-h-56 w-full overflow-y-auto rounded-md border border-border bg-popover p-1 shadow-md">
          {(suggestions.data ?? []).map((p) => {
            // Fact 5/5's ONE implementation, imported rather than re-derived
            // (review M-2): the engine sees the allocated bare `username`,
            // never the email `list_principals` also returns.
            const ref = refFor("user", p);
            if (!ref) {
              return (
                <div
                  key={p.name}
                  className="flex w-full items-center justify-between gap-2 rounded px-2 py-1.5 text-left text-sm text-muted-foreground"
                >
                  <span>{p.label}</span>
                  <span className="shrink-0 text-xs italic">no allocated username yet</span>
                </div>
              );
            }
            return (
              <button
                key={p.name}
                type="button"
                className="flex w-full items-center justify-between gap-2 rounded px-2 py-1.5 text-left text-sm hover:bg-accent"
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => {
                  onChange(ref);
                  setOpen(false);
                }}
              >
                <span>{p.label}</span>
                <span className="shrink-0 text-xs text-muted-foreground">stored as {ref}</span>
              </button>
            );
          })}
          {(suggestions.data ?? []).length === 0 && (
            <div className="px-2 py-1.5 text-xs text-muted-foreground">
              {suggestions.isFetching ? "Searching…" : "No matches."}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/**
 * Governance → Explain access (D9; rerouted onto the served artifact by
 * SP4d-2a).
 *
 * Admin-facing: subject + action + target in; one verdict per POLICY PROFILE
 * out, three-valued, over the policy set each engine actually downloads. There
 * is deliberately no aggregate badge: no single word is honest over
 * `{allowed, refused, undecidable}` × N profiles, and a summary reading
 * "Allowed" above a profile that refuses would be the most misleading thing on
 * the page.
 *
 * The subject is a user and nothing else. `workspaces` (SP4a), `roles` (SP4c)
 * and `groups` (SP4d-2a) each left `ExplainSubject` for one version of one
 * reason — a caller-supplied value cannot stand in for a fact only egret can
 * verify — and roles and groups reach the evaluator EMPTY because no engine
 * egret provisions can match either.
 *
 * Gated `policy:read` at the route (`App.tsx`), not `policy:update` — this is
 * the one route on this surface a read-only user can call, because `explain`
 * opens no transaction and writes nothing.
 */
export default function ExplainAccess() {
  const [user, setUser] = useState("");
  const [action, setAction] = useState<Action>("read");
  const [target, setTarget] = useState<Scope>(EMPTY_SCOPE);

  const [result, setResult] = useState<ExplainResponse | null>(null);
  const [explainError, setExplainError] = useState<ApiError | null>(null);

  const explainMutation = usePermExplain<ApiError>();

  const canSubmit = user.trim().length > 0 && target.catalog_id.trim().length > 0;
  /**
   * Why `Explain` is disabled, in words (SP4b-2 finding 2). A disabled button
   * with no explanation is the failure this finding is about: with
   * `policy:read` alone the target picker holds zero options — it says so
   * itself now, and needing `catalog_registry:read` for an admin-tier page's
   * picker is acceptable (D4) — but the button must not simply sit there inert
   * either way.
   */
  const missing = [
    user.trim().length === 0 ? "a user" : null,
    target.catalog_id.trim().length === 0 ? "a target catalog" : null,
  ]
    .filter(Boolean)
    .join(" and ");

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!canSubmit) return;
    setExplainError(null);
    try {
      const res = await explainMutation.mutateAsync({
        data: {
          // A user and nothing else — see the component doc.
          subject: { user: user.trim() },
          action,
          target,
        },
      });
      setResult(res.status === 200 ? res.data : null);
    } catch (err) {
      setResult(null);
      setExplainError(err as ApiError);
    }
  }

  const forbidden = explainError?.status === 403;

  return (
    <div className="space-y-5">
      <div>
        <h1 className="text-2xl font-semibold leading-tight">Explain access</h1>
        <p className="text-sm text-muted-foreground">
          Why a subject can or cannot reach a target — answered per policy profile, over
          the policy set each engine actually downloads.
        </p>
      </div>

      <form
        onSubmit={handleSubmit}
        className="grid gap-6 rounded-lg border border-border p-4 md:grid-cols-2 md:p-6"
      >
        <div className="space-y-4">
          <h2 className="text-sm font-semibold">Subject</h2>
          <SubjectUserField value={user} onChange={setUser} />

          <div className="space-y-1.5">
            <Label htmlFor="explain-action">Action</Label>
            <Select value={action} onValueChange={(v) => setAction(v as Action)}>
              <SelectTrigger id="explain-action" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="read">read</SelectItem>
                <SelectItem value="write">write</SelectItem>
              </SelectContent>
            </Select>
            <p className="text-xs text-muted-foreground">
              Ranger has no <span className="font-mono">write</span>: it is evaluated as
              insert and delete on Trino, and insert, update and delete on StarRocks, each
              answered separately below.
            </p>
          </div>
        </div>

        <div className="space-y-4">
          <h2 className="text-sm font-semibold">Target</h2>
          <ScopePicker value={target} onChange={setTarget} />
          <Button type="submit" disabled={!canSubmit || explainMutation.isPending}>
            {explainMutation.isPending ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : (
              <Search className="h-4 w-4" />
            )}
            Explain
          </Button>
          {!canSubmit && (
            <p className="text-xs text-muted-foreground">Explain needs {missing}.</p>
          )}
        </div>
      </form>

      {explainError && (
        <Alert variant="destructive">
          <AlertDescription>
            {forbidden ? (
              <>
                You need the <code>policy:read</code> permission.
              </>
            ) : (
              explainError.message || "Failed to explain access."
            )}
          </AlertDescription>
        </Alert>
      )}

      {result && (
        <div
          data-testid="explain-result"
          className="space-y-4 rounded-lg border border-border p-4 md:p-6"
        >
          {result.outcome === "no_profile_serves_the_catalog" ? (
            /* Its own state, and deliberately not a refusal's vocabulary or
               colour: nothing is enforcing on this target, which is a
               different fact from something refusing the access. Collapsing
               the two would tell a steward the model is withholding an access
               when in truth no policy set egret compiles covers the resource.

               **It does not say "no plugin is ever consulted"** (whole-branch
               review, Minor). `ExplainOutcome::NoProfileServesTheCatalog` has
               three routes in, and in the second — a profile egret's compiler
               does not target, a tag service say — a plugin IS downloading.
               What is true of all three is that no profile egret compiles for
               covers this catalog, so that is what the copy claims. */
            <Alert data-testid="no-profile-state">
              <AlertDescription className="space-y-1">
                <Badge variant="outline" className="gap-1">
                  <Server className="h-3 w-3" />
                  Nothing is enforcing
                </Badge>
                <p className="text-sm">
                  No policy profile serves this catalog, so no policy set egret compiles
                  covers it. This is not a denial — nothing is enforcing here, which is a
                  different fact from a policy refusing the access. Attach the catalog to a
                  profile if it is meant to be governed.
                </p>
              </AlertDescription>
            </Alert>
          ) : (
            <div className="space-y-3">
              {result.profiles.map((profile) => (
                <ProfileBlock
                  key={profile.service_id}
                  profile={profile}
                  declined={result.tag_resolution_declined ?? undefined}
                />
              ))}
              {result.profiles.length === 0 && (
                <p className="text-sm text-muted-foreground">
                  The answer names no profile, which should not happen for this outcome.
                </p>
              )}
            </div>
          )}

          {/* The honest limits, stated where the answer is read — not in a help
              page. A tool that looks like a complete oracle while being partial
              is worse than one that says what it does not know.

              **The paragraph that used to lead this block is GONE, not
              reworded.** It said this tool cannot see hand-written Ranger
              policy, which was true of the retired model evaluator and is false
              now: `explain` reads the artifact each plugin downloads, seeded
              `egret-admin` hierarchies and operator-written JSON included. A
              limit that no longer holds is worse than no limit, because it
              teaches a reader to discount the ones that still do. */}
          <Alert data-testid="honest-limits">
            <AlertDescription className="space-y-1 text-xs">
              {/* ⚠ **Rewritten by Task 8's browser gate (F4).** This said the tag
                  list "decides nothing here" and is "not the set the engine would
                  resolve", which was true of the exact-match query it used to
                  render and is false of what the server sends now: that list IS the
                  evaluator's input, rolled up the hierarchy, and every verdict above
                  was computed from it. The limit that survives is the one that still
                  holds — a target naming no table can carry none. */}
              {/* ⚠ **Gated on there BEING a profile — found in the browser, and it is
                  the same class one notch further down.** These two paragraphs talk
                  about "each profile's tag list" and "those sets"; under
                  `no_profile_serves_the_catalog` there are no profiles and no lists,
                  so they described nothing on screen. Prose about an absent thing is
                  how a reader learns to discount the limits that do hold — the
                  argument this whole block is introduced with. */}
              {result.profiles.length > 0 && (
                <>
              {/* ⚠ Narrowed with the block itself (review H1/H2/H3). This said
                      "the set every verdict HERE was computed from" under a single
                      list below every profile — which asserted per verdict what was
                      true per request, and said it even when there were no verdicts.
                      Each profile now carries its own set, so this says what is true
                      of all of them and claims nothing about any one. */}
                  <p>
                    Each profile’s tag list is the set egret resolved for this target, rolled up
                    the resource hierarchy the way the engine’s own enricher rolls it: a tag
                    marked <span className="italic">on the table</span> reaches this column
                    because the assignment names the table. A profile whose download carries no
                    tag policies shows none, because nothing there could consult them. Tags
                    populate only when the target names both a schema and a table — a
                    catalog-only target carries none, because assignments attach to tables and
                    columns.
                  </p>
                  {/* ⚠ The asymmetry a steward auditing a table would otherwise learn
                      the hard way, and it is MEASURED rather than reasoned: asking
                      about `sp4d1.t2` with no column answers an empty list while
                      `sp4d1.t2.item` answers `pii`, because the enricher admits SELF and
                      ANCESTOR attachments and not DESCENDANT ones. The engine behaves
                      the same way — the live gate measured a tag-addressed row filter
                      NOT firing at table granularity for a column-level tag — so this
                      is the engine's rule showing through, not egret's gap. Said here
                      because the list cannot say it: it is a true statement about what
                      is ABSENT. */}
                  <p>
                    Those sets are for the resource you asked about, and that cuts downwards:
                    a tag attached to a column does <span className="font-medium">not</span>{" "}
                    appear when you ask about its table, because the engine’s enricher reaches
                    up the hierarchy and never down. Auditing a table is therefore not the same
                    as auditing its columns — ask about the column to see a tag stored on it.
                  </p>
                </>
              )}
              <p>
                A policy that grants through a role, or through any group except{" "}
                <span className="font-mono">public</span>, matches nobody on these engines:
                egret serves no roles download and runs no usersync, so explain sends
                neither. That is why egret’s own compiler names a role’s holders and a
                group’s members instead, by username — so only a hand-written policy can
                still name one.{" "}
                <span className="font-mono">public</span> is the exception and it does
                decide — Ranger matches it by name whatever the request carries, and egret’s
                own compiler grants and denies through it, so a verdict here can come from a{" "}
                <span className="font-mono">public</span> policy. Otherwise any verdict is
                what the engine reaches for this username alone.
              </p>
              <p>
                There is no field here to assert a role, a group or a workspace the subject
                does not hold, so this answers why the subject is allowed or refused as they
                are — not what would happen if they held something else.
              </p>
              {/* ⚠ **Task 8's browser gate, F3.** This paragraph used to end "mask
                  and row-filter verdicts arrive with SP4d-2b" — rendered, on the
                  SP4d-2b branch, where they do not arrive: `explain` calls neither
                  `evaluate_data_mask` nor `evaluate_row_filter`, though the
                  evaluator implements both and Task 8's live gate was built out of
                  them. A promise dated to the version the reader is holding is the
                  limit-that-no-longer-holds failure in its worst form, and the
                  comment above this block already argued against it.

                  So this states the limit and dates nothing. Replacing it with a
                  new promise would reintroduce exactly the same defect the first
                  time a package slipped. */}
              {/* Task 12, M-5: this used to say "ACCESS only" under a StarRocks
                  mask line, which contradicted it. It now names what is shown. */}
              <p>
                On a StarRocks profile, a read of a column also shows the mask the engine
                applies to it. Apart from that this answers about access only: row filters,
                and masks on Trino, are not reported, so a target this page calls{" "}
                <span className="font-medium">Allowed</span> can still return masked values,
                filtered rows, or an error from a mask the engine cannot apply. Asking which
                takes the engine.
              </p>
              <p>
                Each plugin polls for its policy set on an interval, so an engine can still
                be enforcing a slightly older download than the artifact evaluated here.
              </p>
            </AlertDescription>
          </Alert>
        </div>
      )}
    </div>
  );
}
