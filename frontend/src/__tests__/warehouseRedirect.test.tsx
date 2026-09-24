import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import App from "@/App";
import { queryClient } from "@/api/queryClient";
import type { Me } from "@/api/hooks";
import type { RegistryCatalogDto } from "@/api/generated/model";

/**
 * SP3c §3: `/control-plane/warehouses/:id` redirects to the matching catalog.
 *
 * The REAL route table, not a copy — the same reason `catalogsRouting.test.tsx`
 * mounts `<App />`. Nothing here would catch a route registered in the wrong
 * `<Routes>`, or a guard that 403s before the redirect ever renders, if the
 * table were hand-rebuilt in the test.
 *
 * SP3c-2 (Task 8) deleted the Warehouses LIST page and its route outright —
 * D5 only ever promised the DETAIL path would keep working, unlisted, and
 * this file is what pins that promise. `Warehouses.test.tsx` (which tested
 * the list) went with the page; there is no "list still renders" case here
 * any more to keep green.
 */
const TIMEOUT = { timeout: 4000 };

const ME: Me = {
  user_id: "u1",
  email: "u1@example.com",
  platform_roles: [],
  // `catalog_registry:read` alone: SP3c-2 (Task 7) retired `warehouse:read`
  // and re-gated this redirect (and, before Task 8 deleted it, the list route) onto the
  // one key, so it is the only permission either needs.
  permissions: ["catalog_registry:read"],
};

function catalog(over: Partial<RegistryCatalogDto> = {}): RegistryCatalogDto {
  return {
    id: "c-wh",
    name: "analytics",
    kind: "iceberg",
    config: { metastore: { type: "egret", warehouse_id: "wh-1" } },
    advanced: {},
    description: null,
    warehouseId: "wh-1",
    engines: [],
    workspaceIds: [],
    createdAt: "2026-08-01T10:00:00Z",
    updatedAt: "2026-09-01T10:00:00Z",
    ...over,
  };
}

/**
 * `/api/auth/me` answers with the user, `/api/control-plane/catalogs` with
 * `rows`, and every other request rejects — so each page renders its error
 * state and only the routing is under test.
 */
function openApp(url: string, rows: RegistryCatalogDto[], as: Me = ME) {
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const u = String(input);
      const json = (body: unknown) =>
        Promise.resolve(
          new Response(JSON.stringify(body), {
            status: 200,
            headers: { "content-type": "application/json" },
          }),
        );
      if (u === "/api/auth/me") return json(as);
      if (u === "/api/control-plane/catalogs") return json(rows);
      // The destination page's own read, so "the URL changed" can be followed
      // by "and the page it named actually rendered".
      const one = rows.find((c) => u === `/api/control-plane/catalogs/${c.id}`);
      if (one) return json(one);
      return Promise.reject(new Error("no network in test"));
    }),
  );
  window.history.pushState({}, "", url);
  render(<App />);
}

const currentUrl = () =>
  `${window.location.pathname}${window.location.search}${window.location.hash}`;

afterEach(() => {
  cleanup();
  queryClient.clear();
  vi.unstubAllGlobals();
  window.history.replaceState({}, "", "/");
});

describe("/control-plane/warehouses/:id redirects onto the catalog", () => {
  it("sends a warehouse id to the catalog whose warehouseId it is", async () => {
    // Two rows, so "found the right one" is distinguishable from "took the
    // first one it was given".
    openApp("/control-plane/warehouses/wh-1", [
      catalog({ id: "c-other", name: "other", warehouseId: "wh-9" }),
      catalog(),
    ]);
    await waitFor(() => expect(currentUrl()).toBe("/control-plane/catalogs/c-wh"), TIMEOUT);
    await screen.findByTestId("catalog-detail-frame", {}, TIMEOUT);
    expect(screen.queryByText("403 — Forbidden")).not.toBeInTheDocument();
  }, 15_000);

  it("keeps ?tab= across the redirect, so a bookmarked tab still opens", async () => {
    openApp("/control-plane/warehouses/wh-1?tab=browse", [catalog()]);
    await waitFor(
      () => expect(currentUrl()).toBe("/control-plane/catalogs/c-wh?tab=browse"),
      TIMEOUT,
    );
    const tab = await screen.findByRole("tab", { name: /Browse/ }, TIMEOUT);
    expect(tab).toHaveAttribute("aria-selected", "true");
  }, 15_000);

  it("lands a warehouse with no registry catalog on the list, not a blank page", async () => {
    openApp("/control-plane/warehouses/wh-missing", [catalog()]);
    await waitFor(() => expect(currentUrl()).toBe("/control-plane/catalogs"), TIMEOUT);
    // The list really rendered — `replace` to a path that renders nothing
    // would satisfy the URL assertion on its own.
    expect(await screen.findByRole("heading", { name: "Catalogs" }, TIMEOUT)).toBeInTheDocument();
  }, 15_000);

  it("no longer has a Warehouses LIST route — the bare path lands elsewhere, not on a list", async () => {
    // No `<Route path="warehouses">` remains (Task 8); react-router falls
    // through to the `*` catch-all inside `ControlPlaneRoutes`, which sends a
    // `catalog_registry:read` holder to the first section that permission
    // unlocks — `catalogs` — not to a "Warehouses" heading.
    openApp("/control-plane/warehouses", [catalog()]);
    await waitFor(() => expect(currentUrl()).toBe("/control-plane/catalogs"), TIMEOUT);
    expect(await screen.findByRole("heading", { name: "Catalogs" }, TIMEOUT)).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Warehouses" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: /Warehouses/ })).not.toBeInTheDocument();
  }, 15_000);
});

/**
 * Whole-branch review, F5 — through SP3c-1. Migration 0148 granted
 * `catalog_registry:read` to every platform role that held `warehouse:read`
 * — ONCE. A role created after the upgrade held `warehouse:read` alone: it
 * saw the Warehouses section in the sidebar, the list rendered, and before
 * that fix every row 403'd, taking roles/grants, Clients, Browse, Connect and
 * Settings with it. That was the capability removal D5 existed to prevent, so
 * (through SP3c-1) the path was gated on the key the LIST used and the
 * missing destination key was explained rather than refused.
 *
 * SP3c-2 (Task 7) retires `warehouse:read` outright (migration 0153 re-runs
 * 0148's grant first, so no LIVE role loses anything in the move) and
 * re-gates the redirect route, and `CONTROL_PLANE_PERMS` itself
 * (`auth/appAccess.ts`), on `catalog_registry:read`. The population this
 * block used to describe — a role holding `warehouse:read` and nothing
 * else — cannot exist going forward: 0153 guarantees every role that ever
 * held `warehouse:read` also holds `catalog_registry:read`, and the
 * permission can no longer be granted at all (it left
 * `egret_core::rbac::PERMISSIONS`). So a session carrying `warehouse:read`
 * alone is now indistinguishable from one carrying no relevant permission at
 * all — `RequireControlPlaneAccess`, the outer gate for all of
 * `/control-plane/*`, refuses it before either case below reaches a route —
 * the friendly `WarehouseRedirect` explanation is defense-in-depth for a
 * stale `me.permissions` snapshot, not something this population reaches (see
 * its doc comment in `App.tsx`).
 */
describe("a session holding only the retired warehouse:read (SP3c-2)", () => {
  const RETIRED_KEY_ONLY: Me = {
    user_id: "u2",
    email: "wh@example.com",
    platform_roles: [],
    permissions: ["warehouse:read"],
  };

  it("403s the redirect route — warehouse:read no longer admits anyone", async () => {
    openApp("/control-plane/warehouses/wh-1", [catalog()], RETIRED_KEY_ONLY);
    expect(await screen.findByText("403 — Forbidden", {}, TIMEOUT)).toBeInTheDocument();
    expect(screen.queryByTestId("warehouse-redirect-blocked")).not.toBeInTheDocument();
    expect(currentUrl()).toBe("/control-plane/warehouses/wh-1");
  }, 15_000);

  it("403s the bare warehouses path too — the outer Control Plane gate refuses it before any route (list or catch-all) is even matched", async () => {
    openApp("/control-plane/warehouses", [catalog()], RETIRED_KEY_ONLY);
    expect(await screen.findByText("403 — Forbidden", {}, TIMEOUT)).toBeInTheDocument();
  }, 15_000);
});
