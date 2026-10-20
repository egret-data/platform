/**
 * Shared harness for the external-engine page tests. The tests drive the REAL
 * generated hooks and the real `apiFetch`, with only `fetch` stubbed, so the
 * request URL, method and body they assert are the ones the browser would send
 * and an `ApiError` carries the status, code and body the server sent.
 */
import { vi } from "vitest";
import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, useLocation } from "react-router-dom";

import { TooltipProvider } from "@/components/ui/tooltip";
import { AuthContext } from "@/auth/useAuth";
import type { Me } from "@/api/hooks";

export const ADMIN: Me = {
  user_id: "u1",
  email: "admin@example.com",
  platform_roles: ["platform_admin"],
  permissions: [],
};

export function userWith(...permissions: string[]): Me {
  return { user_id: "u2", email: "ops@example.com", platform_roles: [], permissions };
}

export interface Recorded {
  method: string;
  path: string;
  body: unknown;
}

type Reply = { status?: number; body?: unknown };
type Handler = Reply | ((req: Recorded) => Reply);

/**
 * Stub `fetch` with `"METHOD /path"` routes (path without query). An unrouted
 * request answers 599 so a test never passes on a request it did not expect.
 */
export function stubFetch(routes: Record<string, Handler>) {
  const calls: Recorded[] = [];
  const impl = async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(String(input), "http://localhost");
    const method = (init?.method ?? "GET").toUpperCase();
    const body = typeof init?.body === "string" ? JSON.parse(init.body) : undefined;
    const rec = { method, path: url.pathname, body };
    calls.push(rec);
    const h = routes[`${method} ${url.pathname}`];
    if (!h) return new Response(JSON.stringify({ error: `unrouted ${method} ${url.pathname}` }), { status: 599 });
    const r = typeof h === "function" ? h(rec) : h;
    if (r.status === 204) return new Response(null, { status: 204 });
    return new Response(JSON.stringify(r.body ?? {}), {
      status: r.status ?? 200,
      headers: { "content-type": "application/json" },
    });
  };
  vi.stubGlobal("fetch", vi.fn(impl));
  return calls;
}

export function LocationProbe() {
  const l = useLocation();
  return <div data-testid="location">{l.pathname + l.search}</div>;
}

export function Harness({
  me = ADMIN,
  entry,
  client,
  children,
}: {
  me?: Me | null;
  entry: string;
  /** Pass one to inspect its caches after the run; a fresh one otherwise. */
  client?: QueryClient;
  children: ReactNode;
}) {
  const qc = client ?? new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return (
    <AuthContext.Provider value={{ me, isLoading: false }}>
      <QueryClientProvider client={qc}>
        <TooltipProvider>
          <MemoryRouter initialEntries={[entry]}>{children}</MemoryRouter>
        </TooltipProvider>
      </QueryClientProvider>
    </AuthContext.Provider>
  );
}

export const PROFILES = [
  { id: 11, name: "trino-main", type: "trino", policy_version: 1, policy_count: 0, engines: [], origin: "manual" },
  { id: 12, name: "trino-taken", type: "trino", policy_version: 1, policy_count: 0, engines: [{ id: "e9", name: "other-trino", status: "running" }], origin: "manual" },
  { id: 21, name: "sr-main", type: "starrocks", policy_version: 1, policy_count: 0, engines: [], origin: "manual" },
];

export const CATALOGS = [
  { id: "c1", name: "lake", kind: "iceberg", config: {}, advanced: {}, engines: [], workspaceIds: [], createdAt: "2026-01-01T00:00:00Z", updatedAt: "2026-01-01T00:00:00Z" },
  { id: "c2", name: "sales", kind: "iceberg", config: {}, advanced: {}, engines: [], workspaceIds: [], createdAt: "2026-01-01T00:00:00Z", updatedAt: "2026-01-01T00:00:00Z" },
];

export const EXTERNAL_ENGINE = {
  id: "x1",
  name: "legacy-trino",
  type: "trino",
  namespace: "",
  status: { phase: "running", message: null, pods: [] },
  spec: {},
  origin: "external",
  host: "trino.corp.example",
  port: 8080,
  rangerPluginVersion: "2.5.0",
  rangerPluginVersionModelled: true,
};

/** What `GET /engines/modelled-ranger-plugins` answers (egret's MODELLED_RANGER_PLUGINS). */
export const MODELLED_PLUGINS = [
  { type: "trino", version: "2.5.0" },
  { type: "trino", version: "2.8.0" },
  { type: "starrocks", version: "2.8.0" },
  { type: "starrocks", version: "2.9.0" },
];
