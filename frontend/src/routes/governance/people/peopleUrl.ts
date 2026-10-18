import type { PermPrincipal } from "@/api/generated/model";
import { decodeSegment, encodeSegment } from "@/routes/governance/data/objectPath";

/**
 * Where a principal's People page lives (spec P1):
 * `/governance/people/users/:userId`, `/groups/:name`, `/roles/:name`, and
 * `/users/by-name/:username`, which `ByNameRedirect` resolves to the id and
 * replaces. `?tab=` selects the page's tab.
 *
 * Statically importable, outside the lazy area's bundle guard (as
 * `data/objectPath.ts` is): pages outside the area link into it through here.
 *
 * A name is one path segment, encoded as the Data area encodes its names
 * (`objectPath.ts`'s module doc gives the reasons): a `/`, `%` or space stays
 * in its segment, and a dot-only name survives the router and the browser.
 * Read a param back with `principalParam`.
 */
export type PrincipalLink =
  | { kind: "user"; userId: string }
  | { kind: "user"; username: string }
  | { kind: "group"; name: string }
  | { kind: "role"; name: string };

export const PEOPLE_BASE = "/governance/people";

/** The URL of `p`'s page, on `tab` when given. */
export function principalUrl(p: PrincipalLink, tab?: string): string {
  let path: string;
  if (p.kind === "user") {
    path = "userId" in p ? `users/${encodeSegment(p.userId)}` : `users/by-name/${encodeSegment(p.username)}`;
  } else {
    path = `${p.kind === "group" ? "groups" : "roles"}/${encodeSegment(p.name)}`;
  }
  const query = tab ? `?${new URLSearchParams({ tab }).toString()}` : "";
  return `${PEOPLE_BASE}/${path}${query}`;
}

/** A route param of the area as the router hands it back, with a dot-only name restored. */
export function principalParam(param: string | undefined): string {
  return decodeSegment(param ?? "");
}

/**
 * The page a stored grant principal or rule exemption names. A user ref is a
 * username, so it links through the by-name route. A workspace's members have
 * no People page (workspace pages are out of #2, P16).
 */
export function principalLinkFor(p: PermPrincipal): PrincipalLink | null {
  switch (p.kind) {
    case "user":
      return { kind: "user", username: p.ref };
    case "group":
      return { kind: "group", name: p.ref };
    case "role":
      return { kind: "role", name: p.ref };
    default:
      return null;
  }
}
