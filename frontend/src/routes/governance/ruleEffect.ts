import type { EffectCatalog, ExistingMask, NotEnforced, PermPrincipal, PrincipalSet, RuleEffect } from "@/api/generated/model";
import { MASK_TYPE_LABEL } from "@/routes/governance/glossary";

/*
 * The rule preview in effect terms (spec 2026-10-21-classification-ux-design
 * C6, C7): pure functions over the server's `RuleEffect`, so the dialog only
 * lays the words out. Plurals are written out (FE-COPY-4), and a count the
 * server leaves `null` (a pattern, or "every column") is never printed as a
 * number.
 */

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** "a", "a and b", "a, b and c". */
function listOf(names: string[]): string {
  if (names.length <= 1) return names.join("");
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

function reachOf(e: RuleEffect): string {
  const tables = e.tableCount ?? null;
  const columns = e.columnCount ?? null;
  if (e.kind === "row_filter") {
    if (tables === 0) return "Filters no table yet";
    return tables === null ? "Filters rows of the tables matching the path" : `Filters rows of ${plural(tables, "table", "tables")}`;
  }
  if (tables === 0) return "Masks no column yet";
  if (tables === null) {
    return columns === null ? "Masks the columns matching the path" : `Masks ${plural(columns, "column", "columns")} on the tables matching the path`;
  }
  return columns === null
    ? `Masks every column of ${plural(tables, "table", "tables")}`
    : `Masks ${plural(columns, "column", "columns")} on ${plural(tables, "table", "tables")}`;
}

function exemptOf(e: RuleEffect): string {
  if (e.exemptEveryone) return "everyone exempt";
  if (e.exemptMembers === 0) return "nobody exempt";
  return `${plural(e.exemptMembers, "person", "people")} exempt`;
}

/**
 * One line: what the rule reaches, how many engines enforce it, who is
 * exempt (spec C7.1). The engine count is left out at 0: the "Not enforced
 * yet" warning ({@link notEnforcedWarning}) says that instead.
 */
export function effectSentence(e: RuleEffect): string {
  const parts = [reachOf(e)];
  if (e.enforcedBy > 0) parts.push(`enforced by ${plural(e.enforcedBy, "engine", "engines")}`);
  parts.push(exemptOf(e));
  return parts.join(" · ");
}

/**
 * The sentences that follow "Not enforced yet." when no engine enforces the
 * rule (spec C7.2), one per reason, naming the catalogs it applies to; `null`
 * while an engine enforces it. A warning, not a refusal: the rule can still
 * be created, and is enforced once an engine serves it. `appliesTo` says what
 * the rule is written on: a reach of nothing means an unattached
 * classification only for a classification rule.
 */
export function notEnforcedWarning(e: RuleEffect, appliesTo: "tag" | "path" = "tag"): string[] | null {
  if (e.enforcedBy > 0) return null;
  const verb = e.kind === "row_filter" ? "filtered" : "masked";
  const stored = `so this rule is stored but no query is ${verb}.`;
  const named = (reason: NotEnforced) =>
    e.catalogs.filter((c: EffectCatalog) => c.notEnforced === reason).map((c) => c.catalogName);

  if (e.notEnforced === "no_assignment" || e.catalogs.length === 0) {
    return appliesTo === "tag"
      ? [`This classification is not attached to any table or column a catalog serves yet, ${stored} Attach it in Governance > Data.`]
      : [`No catalog egret knows serves this path yet, ${stored}`];
  }
  const lines: string[] = [];
  const noProfile = named("no_profile");
  if (noProfile.length > 0) {
    const one = noProfile.length === 1;
    lines.push(
      `${one ? `${noProfile[0]}'s engines` : `The engines of ${listOf(noProfile)}`} are not in a policy profile, ${stored} ` +
        `${one ? "Its" : "Their"} engines have no policy profile; bind one in Governance > Engine & policy.`,
    );
  }
  const noEngine = named("no_engine");
  if (noEngine.length > 0) {
    lines.push(
      `${listOf(noEngine)} ${noEngine.length === 1 ? "has" : "have"} no engine attached, ${stored} Attach an engine in Control Plane > Engines.`,
    );
  }
  return lines.length > 0 ? lines : [`No engine enforces it yet, ${stored}`];
}

/** One principal's result where several masks reach one column (spec C6). */
export type PrincipalLine = { principal: string; verdict: "real" | "masked" | "undecided"; text: string };

const KIND_LABEL: Record<string, string> = { group: "group", user: "user", role: "role", workspace_members: "workspace" };

/** "group data-team", "user alice", "workspace sales". */
export function principalLabel(p: PermPrincipal): string {
  return `${KIND_LABEL[p.kind] ?? p.kind} ${p.ref}`;
}

/** A mask type's glossary label, or the stored value when it has none. */
export const maskTypeLabel = (t: string) => MASK_TYPE_LABEL[t] ?? t;

/**
 * What each principal sees when this rule joins the masks already on a
 * column (spec C6): one line per principal any of them exempts, sorted, then
 * "Everyone else". Exempt from every mask: the real value. From none: masked.
 * From some but not all: undecided, since the engine applies whichever mask
 * it evaluates first. Only `exempt.include` is read: an exemption has no
 * `exclude` (D6).
 *
 * **Names as written.** Principals are compared by `{kind, ref}`: groups,
 * roles and workspaces are expanded to people only when compiled, so a
 * person exempted as `user alice` in one mask and through `group data-team`
 * in another is two lines here. The caller says so beside the list. With
 * `truncated`, masks may be missing, so "sees the real value" is qualified.
 *
 * Two `CUSTOM` masks never agree: `ExistingMask` carries no expression to
 * compare.
 */
export function effectivePerPrincipal(
  rule: { maskType: string; exempt: PrincipalSet },
  existing: ExistingMask[],
  opts: { truncated?: boolean } = {},
): PrincipalLine[] {
  const masks = [{ maskType: rule.maskType, exempt: rule.exempt }, ...existing];
  const types = new Set(masks.map((m) => m.maskType));
  const agree = types.size === 1 && !(types.has("CUSTOM") && masks.length > 1);
  const maskedText = agree ? `masked (${maskTypeLabel(masks[0].maskType)})` : "masked; which mask applies is decided by engine order";
  const realText = opts.truncated ? "sees the real value (of the masks listed)" : "sees the real value";

  const principals = new Map<string, PermPrincipal>();
  for (const m of masks) for (const p of m.exempt.include) principals.set(principalLabel(p), p);

  const lines: PrincipalLine[] = [...principals.keys()]
    .sort((a, b) => a.localeCompare(b))
    .map((label) => {
      const p = principals.get(label) as PermPrincipal;
      const from = masks.filter((m) => m.exempt.include.some((q) => q.kind === p.kind && q.ref === p.ref)).length;
      // Every listed principal is exempt from at least one mask: the one it was read from.
      if (from === masks.length) return { principal: label, verdict: "real", text: realText };
      return { principal: label, verdict: "undecided", text: "undecided: the engine applies whichever mask it evaluates first" };
    });
  lines.push({ principal: "Everyone else", verdict: "masked", text: maskedText });
  return lines;
}
