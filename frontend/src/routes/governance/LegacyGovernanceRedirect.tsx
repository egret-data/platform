import { Navigate, useLocation } from "react-router-dom";
import { PEOPLE_BASE } from "@/routes/governance/people/peopleUrl";

/**
 * Old Control Plane URLs of the pages that moved to Governance (SP2): swaps the
 * leading `/control-plane` segment for `/governance` and keeps everything after
 * it — subpath, `?search` (the policy editor's `?scope=tag` picks By-tag mode)
 * and `#hash` — with `replace`, so Back does not return to the dead URL.
 *
 * Mounted by `App` at the top level, beside `/control-plane/*`, not inside
 * `ControlPlaneRoutes`: that tree sits behind `RequireControlPlaneAccess`,
 * which a governance-only user fails, so a redirect in there would never render
 * for the user it exists for.
 */
export function LegacyGovernanceRedirect() {
  const { pathname, search, hash } = useLocation();
  const target = pathname.replace(/^\/control-plane(?=\/)/, "/governance");
  return <Navigate to={{ pathname: target, search, hash }} replace />;
}

/**
 * The URLs of the three pages People & groups replaced (DG redesign #2, spec
 * P8 and §5): Explain access, Data permissions and Mask & filter rules. A deep
 * link to one grant or rule (`?grant=<id>`, `?rule=<id>`, which explain's
 * attributions and the overlap report used to write) opens that grant's or
 * rule's own page; anything else opens People & groups. With `replace`, so
 * Back does not return to the dead URL. Nothing else in the old query had a
 * meaning on the new pages (Explain access kept its question in state, not in
 * the URL), so nothing else is carried.
 */
export function RetiredPermPageRedirect() {
  const { search } = useLocation();
  const params = new URLSearchParams(search);
  const grant = params.get("grant");
  const rule = params.get("rule");
  const target = grant
    ? `/governance/grants/${encodeURIComponent(grant)}`
    : rule
      ? `/governance/rules/${encodeURIComponent(rule)}`
      : PEOPLE_BASE;
  return <Navigate to={target} replace />;
}
