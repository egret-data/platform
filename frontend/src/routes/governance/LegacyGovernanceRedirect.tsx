import { Navigate, useLocation } from "react-router-dom";

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
