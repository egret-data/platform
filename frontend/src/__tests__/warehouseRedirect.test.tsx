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
 * D5 is asserted too, in the last case: the Warehouses LIST is still a page in
 * SP3c-1, and only the detail path redirects. That is the whole scope of the
 * change — a future commit that redirects the list as well turns that case red
 * rather than passing quietly.
 */
const TIMEOUT = { timeout: 4000 };

const ME: Me = {
  user_id: "u1",
  email: "u1@example.com",
  platform_roles: [],
  // Both keys, which is what every platform role holding `warehouse:read` has
  // had since migration 0148. `catalog_registry:read` is what the redirect's
  // own guard asks for; `warehouse:read` is what the list it came from asks.
  permissions: ["warehouse:read", "catalog_registry:read"],
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
function openApp(url: string, rows: RegistryCatalogDto[]) {
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
      if (u === "/api/auth/me") return json(ME);
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

  it("leaves the Warehouses LIST a page — D5 removes nothing in SP3c-1", async () => {
    openApp("/control-plane/warehouses", [catalog()]);
    await screen.findByTestId("app-sidebar", {}, TIMEOUT);
    // It did not redirect…
    expect(currentUrl()).toBe("/control-plane/warehouses");
    // …and it is the list, not a 403 and not an empty shell.
    expect(
      await screen.findByRole("heading", { name: "Warehouses" }, TIMEOUT),
    ).toBeInTheDocument();
    expect(screen.queryByText("403 — Forbidden")).not.toBeInTheDocument();
    // The sidebar entry that reaches it is still there (D5).
    expect(screen.getByRole("link", { name: /Warehouses/ })).toBeInTheDocument();
  }, 15_000);
});
