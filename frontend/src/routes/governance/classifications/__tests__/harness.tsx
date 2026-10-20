import { render } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { Me } from "@/api/hooks";
import type { Tag } from "@/api/tags";
import { AuthContext } from "@/auth/useAuth";
import { TooltipProvider } from "@/components/ui/tooltip";
import ClassificationsRoutes from "@/routes/governance/classifications/ClassificationsRoutes";
import { me } from "@/routes/governance/engines/__tests__/engineHarness";

/**
 * Renders the Classification area's own route table (`ClassificationsRoutes`)
 * at a URL, with the real hooks; answer `fetch` with `serve` from the Engine &
 * policy harness (keys `METHOD path?query`, `/api/control-plane` stripped; an
 * unanswered request is a 404, `"never"` stays pending).
 */
export { created, fail, me, ok, serve, where, type Answer } from "@/routes/governance/engines/__tests__/engineHarness";

/** A `tag:read` holder only (`vt` in the live check). */
export const TAG_READER = me(["tag:read"]);
/** `tag:read` and `policy:read`, no writes. */
export const TAG_POLICY_READER = me(["tag:read", "policy:read"]);
/** The steward: `tag:read`, `tag:manage` and `policy:read` (`vs`). */
export const STEWARD = me(["tag:read", "tag:manage", "policy:read"]);
/** A `tag:manage` holder without `policy:read`. */
export const TAG_MANAGER = me(["tag:read", "tag:manage"]);
export const ADMIN = me([], ["platform_admin"]);

function Where() {
  const loc = useLocation();
  return <output data-testid="where">{loc.pathname + loc.search}</output>;
}

export function openArea(url: string, as: Me = STEWARD) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const utils = render(
    <QueryClientProvider client={qc}>
      <AuthContext.Provider value={{ me: as, isLoading: false }}>
        <TooltipProvider>
          <MemoryRouter initialEntries={[url]}>
            <Routes>
              <Route path="/governance/classifications/*" element={<ClassificationsRoutes />} />
              <Route path="*" element={<p>outside the area</p>} />
            </Routes>
            <Where />
          </MemoryRouter>
        </TooltipProvider>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
  return { ...utils, qc };
}

/** A tag row as `GET /control-plane/tags` answers it. */
export function tag(over: Partial<Tag> & Pick<Tag, "id" | "key" | "name">): Tag {
  return {
    description: "",
    color: "#e11d48",
    assignment_count: 0,
    created_at: "2026-10-01T00:00:00Z",
    required_roles: [],
    required_groups: [],
    catalog_tables: 0,
    catalog_columns: 0,
    ...over,
  };
}
