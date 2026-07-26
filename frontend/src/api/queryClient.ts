import { QueryClient } from "@tanstack/react-query";

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
