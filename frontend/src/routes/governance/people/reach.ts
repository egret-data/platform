import type { NotOnEngines, PermPrincipal, PrincipalSet, StandsFor, Via } from "@/api/generated/model";

/** One way a grant's (or rule's) principal set meets the principal a page is about. */
export type Reach = { position: "include" | "exclude"; via: Via; onEngines: boolean; notOnEngines?: NotOnEngines };

/**
 * A uuid's 32 hex digits, lower case, from any form the Rust `uuid` crate
 * parses (hyphenated, simple, braced, `urn:uuid:`); null when `s` is none.
 */
export function parseUuid(s: string): string | null {
  let t = s.toLowerCase();
  if (t.startsWith("urn:uuid:")) t = t.slice("urn:uuid:".length);
  else if (t.startsWith("{") && t.endsWith("}")) t = t.slice(1, -1);
  if (/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(t)) return t.replace(/-/g, "");
  return /^[0-9a-f]{32}$/.test(t) ? t : null;
}

/**
 * Whether a stored principal is the entry of `standsFor` (spec P4): same kind
 * and the same ref byte for byte, as the compiler joins. A `workspace_members`
 * ref is the exception the server documents (`StandsFor`): it is compared as a
 * parsed uuid, since a non-canonical ref stored before writes canonicalised it
 * is kept as written and still applies on engines.
 */
function meets(p: PermPrincipal, s: StandsFor): boolean {
  if (p.kind !== s.kind) return false;
  if (p.kind !== "workspace_members") return p.ref === s.ref;
  const a = parseUuid(p.ref);
  return a !== null && a === parseUuid(s.ref);
}

/** Every way `standsFor` meets one of the set's principals, include first, in the set's order. */
export function setReach(set: PrincipalSet, standsFor: StandsFor[]): Reach[] {
  const out: Reach[] = [];
  for (const position of ["include", "exclude"] as const) {
    for (const p of set[position]) {
      for (const s of standsFor) {
        if (!meets(p, s)) continue;
        const r: Reach = { position, via: s.via, onEngines: s.onEngines };
        if (s.notOnEngines) r.notOnEngines = s.notOnEngines;
        out.push(r);
      }
    }
  }
  return out;
}

export type ReachGroup =
  | "direct"
  | "group"
  | "role"
  | "workspace"
  | "public"
  | "excluded"
  | "egret_only"
  | "not_on_engines";

/** The order a page lists the groups in, the most direct first. */
export const REACH_GROUPS: readonly ReachGroup[] = [
  "direct",
  "group",
  "role",
  "workspace",
  "public",
  "excluded",
  "egret_only",
  "not_on_engines",
];

const BY_VIA: Partial<Record<Via["kind"], ReachGroup>> = {
  itself: "direct",
  group: "group",
  role: "role",
  workspace: "workspace",
  public: "public",
};

/**
 * Which group a grant is listed under.
 *
 * - `excluded`: an exclude path that applies on engines. Ranger's exclude wins
 *   over any include, so the engines leave the principal out.
 * - An exclude path engines do not apply (a role held through a group, say)
 *   excludes nobody on engines, so it does not decide the group: the includes
 *   do, and the page says the exclude is egret's only.
 * - Otherwise the most direct include that applies on engines names the group.
 * - No include applies on engines: `egret_only` when one is held through a
 *   group (egret's own checks count it, the compiler does not), else
 *   `not_on_engines` (expired, or no username yet).
 */
export function reachGroup(r: Reach[]): ReachGroup {
  if (r.some((x) => x.position === "exclude" && x.onEngines)) return "excluded";
  const includes = r.filter((x) => x.position === "include");
  const onEngines = includes.filter((x) => x.onEngines).map((x) => BY_VIA[x.via.kind] ?? "egret_only");
  for (const g of REACH_GROUPS) if (onEngines.includes(g)) return g;
  return includes.some((x) => x.notOnEngines === "held_through_group") ? "egret_only" : "not_on_engines";
}

/**
 * Whether a grant concerns the principal at all: some include reaches it, or
 * an exclude that applies on engines leaves it out. An exclude engines do not
 * apply, with no include, changes nothing for the principal anywhere.
 */
export function reachesAtAll(r: Reach[]): boolean {
  return r.some((x) => x.position === "include" || x.onEngines);
}
