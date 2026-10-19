/**
 * Which egret object a compiled Ranger policy came from, read from its NAME:
 * the frontend twin of `attribute_policy_name`
 * (`crates/egret-perm-compile/src/attribution.rs`, spec premise 5). Ownership
 * is read from `managed_by`, never from here; this only builds the Policies
 * tab's "Open source" link.
 *
 * - `perm-grant-{uuid}` → a grant (always a bare id);
 * - `perm-rule-{uuid}` or `perm-rule-{uuid}-{locator}` → a rule;
 * - `perm-clearance-{tag uuid}[-{locator}]` → a classification's clearance;
 * - `perm-clearance-mv` → the engine-wide materialized-view deny (no single source);
 * - `perm-engine-{uuid}` → an engine grant (always a bare id).
 *
 * Anything else, including a truncated or garbled uuid, is `null`. The uuid is
 * the fixed-width 36 characters after the prefix, never found by splitting on
 * `-`: a catalog name inside a locator can hold hyphens of its own.
 */
export type CompiledSource =
  | { kind: "grant" | "engine_grant"; id: string }
  | { kind: "rule" | "clearance"; id: string; locator?: string }
  | { kind: "mv_deny" };

const GRANT = "perm-grant-";
const RULE = "perm-rule-";
const CLEARANCE = "perm-clearance-";
const ENGINE_GRANT = "perm-engine-";
const MV_DENY_SUFFIX = "mv";

const UUID_LEN = 36;
const UUID = /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;

function splitUuid(rest: string): { id: string; remainder: string } | null {
  if (rest.length < UUID_LEN) return null;
  const id = rest.slice(0, UUID_LEN);
  return UUID.test(id) ? { id, remainder: rest.slice(UUID_LEN) } : null;
}

function bareUuid(rest: string): string | null {
  const s = splitUuid(rest);
  return s && s.remainder === "" ? s.id : null;
}

function uuidAndLocator(rest: string): { id: string; locator?: string } | null {
  const s = splitUuid(rest);
  if (!s) return null;
  if (s.remainder === "") return { id: s.id };
  if (!s.remainder.startsWith("-")) return null;
  const locator = s.remainder.slice(1);
  return locator === "" ? null : { id: s.id, locator };
}

export function compiledSource(name: string): CompiledSource | null {
  if (name.startsWith(GRANT)) {
    const id = bareUuid(name.slice(GRANT.length));
    return id ? { kind: "grant", id } : null;
  }
  if (name.startsWith(RULE)) {
    const r = uuidAndLocator(name.slice(RULE.length));
    return r ? { kind: "rule", ...r } : null;
  }
  if (name.startsWith(CLEARANCE)) {
    const rest = name.slice(CLEARANCE.length);
    if (rest === MV_DENY_SUFFIX) return { kind: "mv_deny" };
    const r = uuidAndLocator(rest);
    return r ? { kind: "clearance", ...r } : null;
  }
  if (name.startsWith(ENGINE_GRANT)) {
    const id = bareUuid(name.slice(ENGINE_GRANT.length));
    return id ? { kind: "engine_grant", id } : null;
  }
  return null;
}

/** What a compiled policy's source is called, when its name says (spec §6.2). */
export function compiledSourceLabel(src: CompiledSource | null): string {
  switch (src?.kind) {
    case "grant":
      return "Grant";
    case "rule":
      return "Rule";
    case "clearance":
      return "Classification clearance";
    case "engine_grant":
      return "Engine permission";
    case "mv_deny":
      return "Materialized-view deny";
    default:
      return "Compiled policy";
  }
}
