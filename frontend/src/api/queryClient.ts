import { QueryClient } from "@tanstack/react-query";
import { clearAllRuleDrafts } from "@/routes/governance/ruleDraft";

/**
 * The single app-wide QueryClient instance. Pulled out of `App.tsx` so that
 * non-React code (`client.ts`) can reach it too: a 403 with
 * `code: "feature_disabled"` invalidates the `["my-workspaces"]` query here,
 * which is what lets `RequireFeature`/`Sidebar` notice a module was switched
 * off mid-session (see `client.ts`).
 */
export const queryClient = new QueryClient({
  defaultOptions: { queries: { refetchOnWindowFocus: false, retry: 1, staleTime: 30000 } },
});

/**
 * Drops every cached query except `["me"]`. Call this at every point where
 * the authenticated identity changes in the same tab — login, logout,
 * impersonation start/end — so the next render can't reuse a *different*
 * user's cached data (e.g. `["my-workspaces"]`, which `HomeRedirect` reads
 * straight from cache) past its `staleTime` (SP2 final review finding F1).
 * `me` itself is left for each caller to handle directly: `setQueryData` on
 * logout (no network needed — we already know we're unauthenticated) or
 * `invalidateQueries` elsewhere (so an active observer refetches it fresh).
 */
export function clearUserScopedCache(qc: QueryClient) {
  qc.removeQueries({ predicate: (query) => query.queryKey[0] !== "me" });
  // The rule dialog's unsaved drafts are the previous identity's too (spec C8.4).
  clearAllRuleDrafts();
}
