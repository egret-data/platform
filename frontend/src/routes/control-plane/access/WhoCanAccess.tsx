import { Navigate, useSearchParams } from "react-router-dom";

/**
 * Legacy `who-can-access` route. The object-direction view has been merged into
 * the bidirectional Access Explorer, so this now just redirects there in object
 * direction, forwarding the existing `?ws=&kind=&id=` params to preserve old
 * deep-links.
 */
export default function WhoCanAccess() {
  const [sp] = useSearchParams();
  const next = new URLSearchParams(sp);
  next.set("dir", "object");
  return <Navigate to={`/control-plane/access-explorer?${next.toString()}`} replace />;
}
