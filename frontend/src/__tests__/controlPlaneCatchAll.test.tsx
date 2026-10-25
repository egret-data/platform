import { cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import App from "@/App";
import { queryClient } from "@/api/queryClient";
import type { Me } from "@/api/hooks";

/**
 * KI-016: an unknown Control Plane URL (`/control-plane/config`) redirected to
 * `workspaces` RELATIVE to the unmatched URL, so the catch-all matched again
 * and appended `workspaces` without end. The REAL route table is mounted, as
 * in `catalogsRouting.test.tsx`; only `/api/auth/me` answers.
 */
const TIMEOUT = { timeout: 4000 };

const me = (permissions: string[], platform_roles: string[] = []): Me => ({
  user_id: "u1",
  email: "u1@example.com",
  platform_roles,
  permissions,
});

function openApp(url: string, as: Me) {
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) =>
      String(input) === "/api/auth/me"
        ? Promise.resolve(
            new Response(JSON.stringify(as), { status: 200, headers: { "content-type": "application/json" } }),
          )
        : Promise.reject(new Error("no network in test")),
    ),
  );
  window.history.pushState({}, "", url);
  render(<App />);
}

afterEach(() => {
  cleanup();
  queryClient.clear();
  vi.unstubAllGlobals();
  window.history.replaceState({}, "", "/");
});

describe("Control Plane catch-all (KI-016)", () => {
  it("sends a platform admin on an unknown section to Workspaces, once", async () => {
    openApp("/control-plane/config", me([], ["platform_admin"]));
    await waitFor(() => expect(window.location.pathname).toBe("/control-plane/workspaces"), TIMEOUT);
    // Still there a moment later: the redirect did not run again.
    await new Promise((r) => setTimeout(r, 300));
    expect(window.location.pathname).toBe("/control-plane/workspaces");
  }, 15_000);

  it("sends a scoped user on an unknown section to the first section they hold", async () => {
    openApp("/control-plane/config/x", me(["engine:read"]));
    await waitFor(() => expect(window.location.pathname).toBe("/control-plane/engines"), TIMEOUT);
  }, 15_000);
});
