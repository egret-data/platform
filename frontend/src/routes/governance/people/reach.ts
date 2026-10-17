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

export type ReachGroup = "direct" | "group" | "role" | "workspace" | "public" | "excluded" | "egret_only";

/** The order a page lists the groups in, the most direct first. */
export const REACH_GROUPS: readonly ReachGroup[] = ["direct", "group", "role", "workspace", "public", "excluded", "egret_only"];

const BY_VIA: Partial<Record<Via["kind"], ReachGroup>> = {
  itself: "direct",
  group: "group",
  role: "role",
  workspace: "workspace",
  public: "public",
};

/**
 * Which group a grant is listed under. An exclude wins over any include, as
 * Ranger's exclude does. Otherwise the most direct path that applies on
 * engines names it; when none does, it is `egret_only`.
 */
export function reachGroup(r: Reach[]): ReachGroup {
  if (r.some((x) => x.position === "exclude")) return "excluded";
  const groups = r.filter((x) => x.onEngines).map((x) => BY_VIA[x.via.kind] ?? "egret_only");
  for (const g of REACH_GROUPS) if (groups.includes(g)) return g;
  return "egret_only";
}
