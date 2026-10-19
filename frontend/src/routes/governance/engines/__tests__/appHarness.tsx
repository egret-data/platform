import { render } from "@testing-library/react";
import { vi } from "vitest";
import App from "@/App";
import type { Me } from "@/api/hooks";
import type { ProfileDto } from "@/api/generated/model";

/**
 * The REAL route table (`App`), as `src/__tests__/governanceRouting.test.tsx`
 * renders it: where a route or a redirect is mounted, and behind which guard,
 * is what these tests pin, and a copy of the table could not see it.
 *
 * `/api/auth/me` answers with the user under test and
 * `/api/control-plane/policies/services` with `profiles`; every other request
 * rejects (pages render their error states; only routing is asserted).
 */
export const TIMEOUT = { timeout: 4000 };

export const me = (permissions: string[], platform_roles: string[] = []): Me => ({
  user_id: "u1",
  email: "u1@example.com",
  platform_roles,
  permissions,
});

export const POLICY_READER = me(["policy:read"]);
export const TAG_ONLY = me(["tag:read"]);

const json = (body: unknown) =>
  Promise.resolve(new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } }));

/** A profile row as `GET /policies/services` returns it; only `name` and `engines` matter to routing. */
export function profile(name: string, engines: { id: string; name: string }[] = []): ProfileDto {
  return {
    id: name.length,
    name,
    type: "trino",
    origin: "manual",
    policy_version: 1,
    policy_count: 0,
    engines: engines.map((e) => ({ ...e, status: "running" })),
  } as ProfileDto;
}

/** `profiles: "error"` answers the profile list with a 500. */
export function openApp(url: string, as: Me, profiles: ProfileDto[] | "error" = []) {
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const s = String(input);
    if (s === "/api/auth/me") return json(as);
    if (s === "/api/control-plane/policies/services") {
      return profiles === "error"
        ? Promise.resolve(new Response(JSON.stringify({ error: "boom" }), { status: 500 }))
        : json(profiles);
    }
    return Promise.reject(new Error("no network in test"));
  });
  vi.stubGlobal("fetch", fetchMock);
  window.history.pushState({}, "", url);
  render(<App />);
  return fetchMock;
}

export const currentUrl = () => `${window.location.pathname}${window.location.search}${window.location.hash}`;
