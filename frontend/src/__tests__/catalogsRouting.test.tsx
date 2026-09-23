import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import App from "@/App";
import { queryClient } from "@/api/queryClient";
import type { Me } from "@/api/hooks";

/**
 * The REAL route table, not a copy — the same reason `governanceRouting.test.tsx`
 * mounts `<App />`.
 *
 * This is the lock-out SP3c D3 exists to avoid, asserted end to end. Three
 * separate lists have to agree before a `catalog_registry:read` user can see a
 * catalog, and each lives in a different file:
 *
 *   1. `CONTROL_PLANE_PERMS` (`auth/appAccess.ts`) — may they enter the app AT
 *      ALL. This is the dangerous one: it gates the whole Control Plane, not a
 *      page, so a user re-gated onto the new key before it joined the list
 *      loses Engines, Storage, Secrets and the rest along with Catalogs.
 *   2. `FIRST_SECTION_BY_PERM` (`App.tsx`) — where `/control-plane` sends them.
 *      Matching nothing here drops a non-admin onto the admin-only
 *      "workspaces" fallback, i.e. a 403.
 *   3. `RequirePermission` on the route itself.
 *
 * `/api/auth/me` answers with the user under test; every other request rejects,
 * so the page renders its error state — only routing is asserted. "Mounted
 * without a 403" is read off `app-sidebar`, which `AppShell` renders inside the
 * area guard and around the per-route guard.
 */
const TIMEOUT = { timeout: 4000 };

const me = (permissions: string[], platform_roles: string[] = []): Me => ({
  user_id: "u1",
  email: "u1@example.com",
  platform_roles,
  permissions,
});
const CATALOG_ONLY = me(["catalog_registry:read"]);
const ENGINE_ONLY = me(["engine:read"]);

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

const currentUrl = () => `${window.location.pathname}${window.location.search}${window.location.hash}`;

afterEach(() => {
  cleanup();
  queryClient.clear();
  vi.unstubAllGlobals();
  window.history.replaceState({}, "", "/");
});

describe("Catalogs routing in App", () => {
  it("lets a user holding only catalog_registry:read into Control Plane and onto Catalogs", async () => {
    openApp("/control-plane", CATALOG_ONLY);
    await waitFor(() => expect(currentUrl()).toBe("/control-plane/catalogs"), TIMEOUT);
    await screen.findByTestId("app-sidebar", {}, TIMEOUT);
    expect(screen.queryByText("403 — Forbidden")).not.toBeInTheDocument();
    expect(await screen.findByRole("heading", { name: "Catalogs" }, TIMEOUT)).toBeInTheDocument();
  }, 15_000);

  it("prefers Catalogs over Warehouses for a user holding both", async () => {
    // Catalogs sits immediately after `engines` and BEFORE `warehouses` in
    // `FIRST_SECTION_BY_PERM`, so the release that introduces the section also
    // makes it the landing for everyone who can see both (D5 keeps Warehouses
    // reachable, just not first).
    openApp("/control-plane", me(["warehouse:read", "catalog_registry:read"]));
    await waitFor(() => expect(currentUrl()).toBe("/control-plane/catalogs"), TIMEOUT);
  }, 15_000);

  it("still lands an engine-holder on Engines, which stays ahead of Catalogs", async () => {
    openApp("/control-plane", me(["engine:read", "catalog_registry:read"]));
    await waitFor(() => expect(currentUrl()).toBe("/control-plane/engines"), TIMEOUT);
  }, 15_000);

  it("403s a Control Plane user who lacks catalog_registry:read, without changing the URL", async () => {
    openApp("/control-plane/catalogs", ENGINE_ONLY);
    expect(await screen.findByText("403 — Forbidden", {}, TIMEOUT)).toBeInTheDocument();
    expect(currentUrl()).toBe("/control-plane/catalogs");
  }, 15_000);

  it("guards the create route on catalog_registry:create, not :read", async () => {
    openApp("/control-plane/catalogs/new", CATALOG_ONLY);
    expect(await screen.findByText("403 — Forbidden", {}, TIMEOUT)).toBeInTheDocument();
    expect(currentUrl()).toBe("/control-plane/catalogs/new");
  }, 15_000);

  it("admits catalog_registry:read to a catalog's detail route", async () => {
    openApp("/control-plane/catalogs/c1", CATALOG_ONLY);
    await screen.findByTestId("app-sidebar", {}, TIMEOUT);
    expect(screen.queryByText("403 — Forbidden")).not.toBeInTheDocument();
    expect(currentUrl()).toBe("/control-plane/catalogs/c1");
  }, 15_000);
});
