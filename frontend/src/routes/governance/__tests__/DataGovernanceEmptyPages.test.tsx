import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import App from "@/App";
import { queryClient } from "@/api/queryClient";
import type { Me } from "@/api/hooks";

/**
 * SP4b-2 Task 1: the three new Governance pages are reachable and gated the
 * same way the existing Policies/Classifications routes are. Mounts the REAL
 * route table (as `governanceRouting.test.tsx` and `catalogsRouting.test.tsx`
 * do) rather than a copy, so a redirect or guard mis-placement would fail
 * here too.
 *
 * `/api/auth/me` answers with the user under test; every other request
 * rejects, so each page renders with no data — fine, since only the route
 * shell (heading) is asserted here.
 */
const TIMEOUT = { timeout: 4000 };

const me = (permissions: string[], platform_roles: string[] = []): Me => ({
  user_id: "u1",
  email: "u1@example.com",
  platform_roles,
  permissions,
});
const POLICY_READ = me(["policy:read"]);
const NO_PERMS = me([]);

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

describe("SP4b-2 Governance page shells", () => {
  it("renders the Data permissions heading for a policy:read user", async () => {
    openApp("/governance/data-permissions", POLICY_READ);
    expect(
      await screen.findByRole("heading", { name: "Data permissions" }, TIMEOUT),
    ).toBeInTheDocument();
  }, 15_000);

  it("renders the Mask & filter rules heading for a policy:read user", async () => {
    openApp("/governance/mask-filter-rules", POLICY_READ);
    expect(
      await screen.findByRole("heading", { name: "Mask & filter rules" }, TIMEOUT),
    ).toBeInTheDocument();
  }, 15_000);

  it("renders the Explain access heading for a policy:read user", async () => {
    openApp("/governance/explain-access", POLICY_READ);
    expect(
      await screen.findByRole("heading", { name: "Explain access" }, TIMEOUT),
    ).toBeInTheDocument();
  }, 15_000);

  it("403s a user without policy:read on Data permissions", async () => {
    openApp("/governance/data-permissions", NO_PERMS);
    expect(await screen.findByText("403 — Forbidden", {}, TIMEOUT)).toBeInTheDocument();
  }, 15_000);
});
