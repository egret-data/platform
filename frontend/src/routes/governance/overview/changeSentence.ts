import type { ChangeEntry } from "@/api/generated/model";
import { MASK_TYPE_LABEL, TERM } from "@/routes/governance/glossary";

/*
 * Recent changes in the glossary's words (spec §4; live result N18, finding 6).
 * The server builds `ChangeEntry.sentence` (crates/egret-controlplane/src/
 * governance_changes.rs) in older words: "tagged … with", "mask MASK on
 * tag:<key>", and a table-level path with a trailing dot (the column is "").
 * These rewrites read only what the server sends; anything they cannot read
 * (an unknown shape, a summary too large to keep) keeps the server's sentence.
 */

type Obj = Record<string, unknown>;

const isObj = (v: unknown): v is Obj => typeof v === "object" && v !== null && !Array.isArray(v);
const str = (o: Obj, k: string): string | undefined => (typeof o[k] === "string" ? (o[k] as string) : undefined);

/** "a.b.c." → "a.b.c": a table-level assignment's path ends in an empty column. */
const trimPath = (p: string) => p.replace(/\.+$/, "");

/** The noun, lower case: "classification". */
const CLASSIFICATION = TERM.classification.toLowerCase();

/** The server's name for a classification it can no longer find. */
const DELETED_CLASSIFICATION = "a deleted classification";

const classificationPhrase = (name: string) =>
  name === DELETED_CLASSIFICATION ? name : `${CLASSIFICATION} ${name}`;

/**
 * `tag:assign` / `tag:unassign`: the server's sentence is `<who> tagged <path>
 * with <name>` / `<who> removed <name> from <path>`, with names (catalog,
 * classification) only it can look up, so it is re-worded rather than rebuilt.
 * `who` is the same actor phrase the server starts with.
 */
function tagSentence(e: ChangeEntry, who: string): string | null {
  const assign = e.action === "tag:assign";
  const head = `${who} ${assign ? "tagged" : "removed"} `;
  if (!e.sentence.startsWith(head)) return null;
  const rest = e.sentence.slice(head.length);
  if (assign) {
    // A path has no " with "; a classification's name may.
    const at = rest.indexOf(" with ");
    if (at < 0) return null;
    return `${who} attached ${classificationPhrase(rest.slice(at + 6))} to ${trimPath(rest.slice(0, at))}`;
  }
  // A path has no " from "; a classification's name may.
  const at = rest.lastIndexOf(" from ");
  if (at < 0) return null;
  return `${who} removed ${classificationPhrase(rest.slice(0, at))} from ${trimPath(rest.slice(at + 6))}`;
}

const RULE_VERB: Record<string, string> = {
  "perm_rule:create": "created",
  "perm_rule:update": "changed",
  "perm_rule:delete": "deleted",
};

/**
 * `perm_rule:*`: rebuilt from the rule summary the event carries
 * (`governance_events.rs` `rule_summary`: `{name, kind, selector, function}`,
 * `selector` a path or `tag:<key>`, `function` the mask type or the filter).
 */
function ruleSentence(e: ChangeEntry, who: string): string | null {
  const verb = RULE_VERB[e.action];
  const d = e.detail;
  if (!verb || !isObj(d) || d.truncated === true) return null;
  const r = isObj(d.after) ? d.after : isObj(d.before) ? d.before : null;
  if (!r) return null;
  const name = str(r, "name");
  const selector = str(r, "selector");
  const fn = str(r, "function");
  const kind = str(r, "kind");
  if (name === undefined || selector === undefined || fn === undefined) return null;
  // A stored mask type the glossary does not list is shown as stored, never dropped.
  const what = kind === "row_filter" ? `row filter ${fn}` : `${MASK_TYPE_LABEL[fn] ?? fn} mask`;
  const where = selector.startsWith("tag:") ? `${CLASSIFICATION} ${selector.slice(4)}` : `path ${trimPath(selector)}`;
  return `${who} ${verb} rule ${name}: ${what} on ${where}`;
}

/** The entry's sentence in the glossary's words, or the server's own when it cannot be read. */
export function changeSentence(e: ChangeEntry, who: string): string {
  const mine =
    e.action === "tag:assign" || e.action === "tag:unassign"
      ? tagSentence(e, who)
      : e.action.startsWith("perm_rule:")
        ? ruleSentence(e, who)
        : null;
  return mine ?? e.sentence;
}
