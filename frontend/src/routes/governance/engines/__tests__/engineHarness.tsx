import { render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { vi } from "vitest";
import type { Me } from "@/api/hooks";
import type {
  EngineCapabilitiesResponse,
  EngineGrantsResponse,
  EngineGrantView,
  EnginePluginResponse,
  ManagedEntry,
  PolicySummary,
  ProfileDto,
} from "@/api/generated/model";
import { AuthContext } from "@/auth/useAuth";
import { TooltipProvider } from "@/components/ui/tooltip";
import EnginesRoutes from "@/routes/governance/engines/EnginesRoutes";

/**
 * Renders the Engine & policy area's own route table (`EnginesRoutes`) at a
 * URL, with `fetch` answered from a table keyed by `METHOD path?query`
 * (`GET` may be left out). An unanswered request is a 404, so a page that asks
 * for something the test did not serve shows it. `"never"` keeps a request
 * pending (the loading state).
 */
export type Answer = { status: number; body: unknown } | "never";

const json = (status: number, body: unknown) =>
  new Response(status === 204 ? null : JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });

export const ok = (body: unknown): Answer => ({ status: 200, body });
export const fail = (status: number, error = "boom"): Answer => ({ status, body: { error } });

export interface Served {
  /** Every request as `METHOD path?query`, in order. */
  asked: string[];
  /** Request bodies by `METHOD path`. */
  bodies: Record<string, unknown[]>;
}

export function serve(answers: Record<string, Answer>): Served {
  const asked: string[] = [];
  const bodies: Record<string, unknown[]> = {};
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
      const url = new URL(String(input), "http://localhost");
      const method = (init?.method ?? "GET").toUpperCase();
      const path = decodeURIComponent(url.pathname.replace(/^\/api\/control-plane/, ""));
      const key = `${method} ${path}${url.search ? decodeURIComponent(url.search) : ""}`;
      asked.push(key);
      if (init?.body) (bodies[`${method} ${path}`] ??= []).push(JSON.parse(String(init.body)));
      const a =
        answers[key] ??
        (method === "GET" ? answers[`${path}${url.search ? decodeURIComponent(url.search) : ""}`] : undefined) ??
        fail(404, "not found");
      return a === "never" ? new Promise<Response>(() => {}) : Promise.resolve(json(a.status, a.body));
    }),
  );
  return { asked, bodies };
}

export const me = (permissions: string[], platform_roles: string[] = []): Me => ({
  user_id: "s1",
  email: "s@example.com",
  platform_roles,
  permissions,
});
export const READER = me(["policy:read"]);
export const STEWARD = me(["policy:read", "policy:update"]);
export const OPERATOR = me(["policy:read", "policy:update", "engine:read", "tag:read"]);

function Where() {
  const loc = useLocation();
  return <output data-testid="where">{loc.pathname + loc.search}</output>;
}
export const where = () => screen.getByTestId("where").textContent;

export function openArea(url: string, as: Me = READER) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <AuthContext.Provider value={{ me: as, isLoading: false }}>
        <TooltipProvider>
          <MemoryRouter initialEntries={[url]}>
            <Routes>
              <Route path="/governance/engines/*" element={<EnginesRoutes />} />
              <Route path="*" element={<p>outside the area</p>} />
            </Routes>
            <Where />
          </MemoryRouter>
        </TooltipProvider>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
}

// ---- fixtures -------------------------------------------------------------

export const E_TR = "7f1c0e8a-0000-4000-8000-0000000000a1";
export const E_SR = "7f1c0e8a-0000-4000-8000-0000000000b2";
export const E_OTHER = "7f1c0e8a-0000-4000-8000-0000000000c3";

export function grantsResponse(over: Partial<EngineGrantsResponse> = {}): EngineGrantsResponse {
  return {
    engine: { id: E_TR, name: "lake-trino", type: "trino", origin: "provisioned", status: "running" },
    profile: { serviceId: 2, name: "lake-tr" },
    reach: { engines: [{ id: E_TR, name: "lake-trino", origin: "provisioned" }], sharedProfile: false },
    grants: [],
    managed: [],
    ...over,
  };
}

export function profileDto(name: string, over: Partial<ProfileDto> = {}): ProfileDto {
  return {
    id: name.length,
    name,
    type: "trino",
    origin: "generated",
    policy_version: 3,
    policy_count: 0,
    engines: [],
    tag_service: null,
    ...over,
  } as ProfileDto;
}

export function policy(id: number, managed_by: string, over: Partial<PolicySummary> = {}): PolicySummary {
  return {
    id,
    name: `policy-${id}`,
    service: "lake-tr",
    policy_type: "access",
    resource_path: "catalog=ice",
    principals: [],
    permissions: ["select"],
    is_enabled: true,
    description: "",
    labels: [],
    managed_by,
    ...over,
  };
}

export function pluginResponse(over: Partial<EnginePluginResponse> = {}): EnginePluginResponse {
  return {
    generatedAt: "2026-10-19T10:00:00Z",
    engine: { id: E_TR, name: "lake-trino", type: "trino", origin: "provisioned", status: "running" },
    profile: { serviceId: 2, name: "lake-tr", policyVersion: 3, tagVersion: 1, updatedAt: "2026-10-19T09:00:00Z" },
    sharedWith: [],
    plugin: { version: "2.5.0", modelled: true, note: null },
    judged: { state: "judged" },
    kinds: [],
    rows: [],
    ...over,
  };
}

export function engineGrant(over: Partial<EngineGrantView> & Pick<EngineGrantView, "id" | "capability">): EngineGrantView {
  return {
    engineId: E_TR,
    engineName: "lake-trino",
    values: [],
    targetUserId: null,
    principals: [],
    policyName: `perm-engine-${over.id}`,
    reach: { engines: [{ id: E_TR, name: "lake-trino", origin: "provisioned" }], sharedProfile: false },
    ...over,
  };
}

export function managedEntry(over: Partial<ManagedEntry> & Pick<ManagedEntry, "policyId" | "kind">): ManagedEntry {
  return {
    policyName: `managed-${over.policyId}`,
    capability: null,
    resources: {},
    accesses: [],
    users: [],
    groups: [],
    enabled: true,
    ...over,
  };
}

export const CAPABILITIES: EngineCapabilitiesResponse = {
  engines: [
    {
      engineType: "trino",
      capabilities: [
        {
          id: "trino.impersonate_user",
          label: "Impersonate a user",
          description: "Run queries as another user.",
          resources: [{ key: "trinouser", valueKind: "username" }],
          access: "impersonate",
        },
        {
          id: "trino.set_catalog_session_property",
          label: "Set catalog session properties",
          description: "Set session properties of a catalog.",
          resources: [
            { key: "catalog", valueKind: "catalog" },
            { key: "sessionproperty", valueKind: "identifier" },
          ],
          access: "alter",
        },
      ],
      notGrantable: [{ what: "Query another user's queries", why: "Ranger's Trino plugin has no such resource." }],
    },
    {
      engineType: "starrocks",
      capabilities: [
        {
          id: "starrocks.impersonate_user",
          label: "Impersonate a user (StarRocks)",
          description: "EXECUTE AS another user.",
          resources: [{ key: "user", valueKind: "username" }],
          access: "impersonate",
        },
      ],
      notGrantable: [{ what: "Grant a role", why: "Roles are egret's." }],
    },
  ],
};
