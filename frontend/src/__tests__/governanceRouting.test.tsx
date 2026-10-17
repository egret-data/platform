import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import App from "@/App";
import { queryClient } from "@/api/queryClient";
import type { Me } from "@/api/hooks";

/**
 * The REAL route table, not a copy. The failure this guards is one a copy
 * cannot see: where a redirect is mounted relative to an area guard. A
 * governance-only user fails `RequireControlPlaneAccess`, so any redirect
 * inside `ControlPlaneRoutes` would 403 them instead of moving them.
 *
 * `/api/auth/me` answers with the user under test; every other request rejects
 * (pages render their error states, which is fine — only routing is asserted).
 * "Page mounted without a 403" is read off `app-sidebar`: `AppShell` renders
 * inside both area guards and around the per-route `RequirePermission`, so once
 * the sidebar is in the DOM the per-route guard has rendered too.
 */
const TIMEOUT = { timeout: 4000 };

const me = (permissions: string[], platform_roles: string[] = []): Me => ({
  user_id: "u1",
  email: "u1@example.com",
  platform_roles,
  permissions,
});
const GOVERNANCE_ONLY = me(["policy:read", "tag:read"]);
const TAG_ONLY = me(["tag:read"]);
const INFRA_ONLY = me(["engine:read"]);
const ADMIN = me([], ["platform_admin"]);

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

/**
 * Like `openApp`, but also answers `/api/me/workspaces` (the real endpoint
 * behind `useMyWorkspaces`) so a `HomeRedirect` test can exercise the
 * has-workspaces branch instead of `useMyWorkspaces` rejecting.
 */
function openWithWorkspaces(url: string, as: Me, workspaces: { id: string; slug: string; name: string; features: string[] }[]) {
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const s = String(input);
      if (s === "/api/auth/me") {
        return Promise.resolve(
          new Response(JSON.stringify(as), { status: 200, headers: { "content-type": "application/json" } }),
        );
      }
      if (s === "/api/me/workspaces") {
        return Promise.resolve(
          new Response(JSON.stringify(workspaces), { status: 200, headers: { "content-type": "application/json" } }),
        );
      }
      return Promise.reject(new Error("no network in test"));
    }),
  );
  window.history.pushState({}, "", url);
  render(<App />);
}

const currentUrl = () => `${window.location.pathname}${window.location.search}${window.location.hash}`;

afterEach(() => {
  cleanup();
  queryClient.clear();
  vi.unstubAllGlobals();
  window.history.replaceState({}, "", "/");
});

describe("Governance routing in App", () => {
  it("sends a governance-only user who opens /control-plane to Governance, not a 403", async () => {
    openApp("/control-plane", GOVERNANCE_ONLY);
    await waitFor(() => expect(currentUrl()).toBe("/governance/data"), TIMEOUT);
    await screen.findByTestId("app-sidebar", {}, TIMEOUT);
    expect(screen.queryByText("403 — Forbidden")).not.toBeInTheDocument();
  }, 15_000);

  it("lands an old policy-editor bookmark on the Governance editor with its query and hash", async () => {
    openApp("/control-plane/policies/trino/access/new?scope=tag#deny", GOVERNANCE_ONLY);
    await waitFor(() => expect(currentUrl()).toBe("/governance/policies/trino/access/new?scope=tag#deny"), TIMEOUT);
    await screen.findByTestId("app-sidebar", {}, TIMEOUT);
    expect(screen.queryByText("403 — Forbidden")).not.toBeInTheDocument();
  }, 15_000);

  it("lands an old tag bookmark on the Governance tag page", async () => {
    openApp("/control-plane/classifications/t1?tab=clearance", TAG_ONLY);
    await waitFor(() => expect(currentUrl()).toBe("/governance/classifications/t1?tab=clearance"), TIMEOUT);
    await screen.findByTestId("app-sidebar", {}, TIMEOUT);
    expect(screen.queryByText("403 — Forbidden")).not.toBeInTheDocument();
  }, 15_000);

  it("moves the Data Lineage placeholder for an admin", async () => {
    openApp("/control-plane/data-lineage", ADMIN);
    await waitFor(() => expect(currentUrl()).toBe("/governance/data-lineage"), TIMEOUT);
    expect(await screen.findByRole("heading", { name: "Data Lineage" }, TIMEOUT)).toBeInTheDocument();
  }, 15_000);

  it.each([
    ["a governance user", GOVERNANCE_ONLY],
    ["a tag-only user", TAG_ONLY],
    ["a policy-only user", me(["policy:read"])],
    ["an admin", ADMIN],
  ])("opens the Governance index on Data for %s", async (_who, as) => {
    openApp("/governance", as);
    await waitFor(() => expect(currentUrl()).toBe("/governance/data"), TIMEOUT);
    expect(await screen.findByRole("heading", { level: 1, name: "Data" }, TIMEOUT)).toBeInTheDocument();
    expect(screen.queryByText("403 — Forbidden")).not.toBeInTheDocument();
  }, 15_000);

  it("sends an unknown Governance URL to Data", async () => {
    openApp("/governance/nope", GOVERNANCE_ONLY);
    await waitFor(() => expect(currentUrl()).toBe("/governance/data"), TIMEOUT);
  }, 15_000);

  it("keeps a Data object URL, names and all", async () => {
    openApp("/governance/data/my%20cat/50%25/a%2Fb?tab=details", TAG_ONLY);
    expect(await screen.findByRole("heading", { level: 1, name: "a/b" }, TIMEOUT)).toBeInTheDocument();
    expect(currentUrl()).toBe("/governance/data/my%20cat/50%25/a%2Fb?tab=details");
  }, 15_000);

  it("403s an infra-only user who opens a Governance URL, without changing the URL", async () => {
    openApp("/governance/policies", INFRA_ONLY);
    expect(await screen.findByText("403 — Forbidden", {}, TIMEOUT)).toBeInTheDocument();
    expect(currentUrl()).toBe("/governance/policies");
  }, 15_000);

  it("still lands an infra-only user on their first Control Plane section", async () => {
    openApp("/control-plane", INFRA_ONLY);
    await waitFor(() => expect(currentUrl()).toBe("/control-plane/engines"), TIMEOUT);
  }, 15_000);

  it("sends a Control Plane user who matches no section to Governance instead of 403ing", async () => {
    // `workspace:read` alone unlocks Control Plane (`RequireControlPlaneAccess`)
    // but matches nothing in `FIRST_SECTION_BY_PERM`, so `ControlPlaneIndex`
    // used to fall back to the platform_admin-only "workspaces" section — a 403
    // for anyone but an admin. With a Governance permission too, there is
    // somewhere else to send them.
    openApp("/control-plane", me(["workspace:read", "policy:read"]));
    await waitFor(() => expect(currentUrl()).toBe("/governance/data"), TIMEOUT);
    expect(screen.queryByText("403 — Forbidden")).not.toBeInTheDocument();
  }, 15_000);

  it("sends a user with no workspace but Governance access from / to Governance", async () => {
    openApp("/", GOVERNANCE_ONLY);
    await waitFor(() => expect(currentUrl()).toBe("/governance/data"), TIMEOUT);
  }, 15_000);

  it("sends a user with no workspace but only Control Plane access from / to Control Plane", async () => {
    openApp("/", INFRA_ONLY);
    await waitFor(() => expect(currentUrl().startsWith("/control-plane")).toBe(true), TIMEOUT);
  }, 15_000);

  it("still tells a plain user with no workspace to ask an admin", async () => {
    openApp("/", me([]));
    expect(await screen.findByText(/No workspaces yet/, {}, TIMEOUT)).toBeInTheDocument();
    expect(currentUrl()).toBe("/");
  }, 15_000);

  it("does not bounce a member with workspaces to Governance while the workspace list is still loading", async () => {
    openWithWorkspaces("/", me(["policy:read"]), [{ id: "w1", slug: "w1", name: "W1", features: [] }]);
    await waitFor(() => expect(currentUrl()).toBe("/explore/w1"), TIMEOUT);
  }, 15_000);
});
