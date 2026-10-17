import { cleanup, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import App from "@/App";
import { queryClient } from "@/api/queryClient";
import type { Me } from "@/api/hooks";

/**
 * SP4b-2 added Data permissions, Mask & filter rules and Explain access; DG
 * redesign #2 (spec P8, §5) replaced them with People & groups and one grant's
 * and one rule's pages. Through the REAL route table (as
 * `governanceRouting.test.tsx` does), so a redirect or guard mis-placement
 * fails here: each old URL opens People & groups, an old `?grant=`/`?rule=`
 * deep link opens that grant's or rule's page, the guards still refuse a user
 * without `policy:read`, and the sidebar no longer lists the three pages.
 *
 * `/api/auth/me` answers with the user under test, and the reads the target
 * pages make answer from the fixtures below; anything else rejects.
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

const GRANT = {
  id: "g-1",
  effect: "allow",
  actions: ["read"],
  scope: { catalog_id: "cat-1", schema: "sales" },
  principals: { include: [{ kind: "role", ref: "analyst" }], exclude: [] },
  origin: { type: "explicit" },
  catalog_name: "wh",
};
const RULE = {
  id: "r-1",
  name: "mask-ssn",
  kind: { type: "mask", mask_type: "MASK_HASH" },
  selector: { type: "tag", key: "pii" },
  exempt: { include: [], exclude: [] },
  tag_name: "PII",
};

function openApp(url: string, as: Me) {
  const bodies: Record<string, unknown> = {
    "/api/auth/me": as,
    "/api/control-plane/perm/people": { items: [], total: 0 },
    "/api/control-plane/perm/grants": [GRANT],
    "/api/control-plane/perm/rules": [RULE],
  };
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const body = bodies[new URL(String(input), "http://localhost").pathname];
      return body === undefined
        ? Promise.reject(new Error("no network in test"))
        : Promise.resolve(new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } }));
    }),
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

describe("the pages People & groups replaced (#2 P8)", () => {
  it.each(["/governance/data-permissions", "/governance/mask-filter-rules", "/governance/explain-access"])(
    "%s opens People & groups",
    async (url) => {
      openApp(url, POLICY_READ);
      expect(await screen.findByRole("heading", { level: 1, name: "People & groups" }, TIMEOUT)).toBeInTheDocument();
      expect(window.location.pathname).toBe("/governance/people");
    },
    15_000,
  );

  it("an old ?grant= deep link opens that grant's page", async () => {
    openApp("/governance/data-permissions?grant=g-1", POLICY_READ);
    expect(
      await screen.findByRole("heading", { level: 1, name: "role analyst can read wh.sales.*" }, TIMEOUT),
    ).toBeInTheDocument();
    expect(window.location.pathname).toBe("/governance/grants/g-1");
  }, 15_000);

  it("an old ?rule= deep link opens that rule's page", async () => {
    openApp("/governance/mask-filter-rules?rule=r-1", POLICY_READ);
    expect(await screen.findByRole("heading", { level: 1, name: "mask-ssn" }, TIMEOUT)).toBeInTheDocument();
    expect(window.location.pathname).toBe("/governance/rules/r-1");
  }, 15_000);

  it("the sidebar no longer lists Data permissions, Mask & filter rules or Explain access", async () => {
    openApp("/governance/people", POLICY_READ);
    await screen.findByRole("heading", { level: 1, name: "People & groups" }, TIMEOUT);
    const sidebar = within(screen.getByTestId("app-sidebar"));
    for (const name of ["Data permissions", "Mask & filter rules", "Explain access"]) {
      expect(sidebar.queryByRole("link", { name })).not.toBeInTheDocument();
    }
  }, 15_000);

  it.each(["/governance/data-permissions", "/governance/grants/g-1", "/governance/rules/r-1"])(
    "403s a user without policy:read at %s",
    async (url) => {
      openApp(url, NO_PERMS);
      expect(await screen.findByText("403 — Forbidden", {}, TIMEOUT)).toBeInTheDocument();
    },
    15_000,
  );
});
