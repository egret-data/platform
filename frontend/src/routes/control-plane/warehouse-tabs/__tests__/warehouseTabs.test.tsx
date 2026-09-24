import { render, screen, within, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";
import { BrowseTab } from "@/routes/control-plane/warehouse-tabs/BrowseTab";
import { ClientsTab } from "@/routes/control-plane/warehouse-tabs/ClientsTab";
import { RolesTab } from "@/routes/control-plane/warehouse-tabs/RolesTab";
import { SettingsTab } from "@/routes/control-plane/warehouse-tabs/SettingsTab";
import * as api from "@/api/controlPlane";
import * as hooks from "@/api/hooks";
import { ApiError } from "@/api/client";
vi.mock("@/api/controlPlane");
vi.mock("@/api/hooks");

/**
 * The tabs that were `WarehouseDetail`'s (SP3c-1 Task 8, design D1).
 *
 * The first two cases are `WarehouseDetail.test.tsx`'s, moved with the
 * components and keeping every mock and assertion. The ONLY change is what is
 * rendered: the component itself instead of the page that used to wrap it —
 * and, in the browse case, the dropped `click(tab "Browse")`, because there is
 * no tab strip around the component any more. Everything after that line is
 * untouched, which is what makes the move falsifiable rather than a rewrite.
 */
function renderTab(ui: React.ReactElement) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>{ui}</MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("warehouse tabs", () => {
  beforeEach(() => {
    vi.mocked(hooks.useMyWorkspaces).mockReturnValue({ data: [] } as unknown as ReturnType<
      typeof hooks.useMyWorkspaces
    >);
    vi.mocked(api.listRoles).mockResolvedValue([
      { id: "r1", name: "engine", consumerType: "engine", description: null },
    ]);
    vi.mocked(api.listGrants).mockResolvedValue([]);
    vi.mocked(api.addGrant).mockResolvedValue(undefined);
    vi.mocked(api.listClients).mockResolvedValue([]);
    vi.mocked(api.listNamespaces).mockResolvedValue([
      { parts: ["analytics"], id: "ns1", protected: false },
    ]);
    vi.mocked(api.listTabulars).mockResolvedValue([
      { name: "events", kind: "table", id: "t1", protected: false },
    ]);
    vi.mocked(api.loadTableDetail).mockResolvedValue({
      location: "s3://bucket/t",
      formatVersion: 2,
      lastUpdatedMs: 0,
      currentSnapshotId: null,
      snapshotCount: 0,
      columns: [{ name: "id", type: "long", required: true }],
      partitionFields: [],
      sortFields: [],
      properties: {},
      snapshots: [],
    });
  });

  test("browse namespaces -> tables -> schema", async () => {
    const user = userEvent.setup();
    renderTab(<BrowseTab warehouseId="w1" />);
    // Click the namespace to expand it, then its table.
    await user.click(await screen.findByText("analytics"));
    await user.click(await screen.findByText("events"));
    // Table detail loads with the schema tab showing the column.
    expect(await screen.findByText("id")).toBeInTheDocument();
    expect(screen.getByText("long")).toBeInTheDocument();
  });

  test("add grant via browse picker", async () => {
    const user = userEvent.setup();
    renderTab(<RolesTab warehouseId="r-wh" />);
    // Default tab is "Roles & grants" — click the role's "Add grant" button.
    await user.click(await screen.findByRole("button", { name: /Add grant/i }));

    // Dialog opens. Wait for the Namespace combobox to appear with loaded options.
    const dialog = await screen.findByRole("dialog");
    const nsTrigger = await within(dialog).findByRole("combobox", { name: /namespace/i });

    // Open the Namespace select and pick "analytics".
    await user.click(nsTrigger);
    await user.click(await screen.findByRole("option", { name: "analytics" }));

    // Open the Table select and pick "events".
    const tableTrigger = await within(dialog).findByRole("combobox", { name: /table/i });
    await user.click(tableTrigger);
    await user.click(await screen.findByRole("option", { name: "events" }));

    // Submit.
    await user.click(within(dialog).getByRole("button", { name: /Add grant/i }));

    await waitFor(() => {
      expect(vi.mocked(api.addGrant)).toHaveBeenCalledWith(
        "r1",
        expect.objectContaining({
          securableType: "table",
          namespace: ["analytics"],
          tableName: "events",
          privilege: expect.anything(),
        }),
      );
    });
  });

  /**
   * The one property the SPLIT itself could break silently.
   *
   * `rolesKey` and `clientsKey` used to be two consts in the file that held
   * both tabs; they now live in `shared.tsx` and are imported by each. If
   * either file grew its own literal, nothing would throw and nothing above
   * would go red — `RolesTab` reads the CLIENTS cache to draw its per-role
   * client count, and `ClientsTab` is what fills that cache. So render both
   * against one `QueryClient` and assert the count arrives: a divergent key
   * gives two cache entries, `listClients` is called for each, and the count
   * is still right. It is the RESOLVED key that has to be identical, which is
   * what `toHaveBeenCalledTimes(1)` pins.
   */
  test("both tabs read the roles and clients caches under one key each", async () => {
    vi.mocked(api.listClients).mockResolvedValue([
      { id: "cl1", name: "trino-prod", consumerType: "engine", catalogRoleId: "r1", clientId: "cid" },
    ]);
    // Call counts are the assertion here, and this file's mocks are module
    // singletons that the earlier cases have already driven. `mockClear` drops
    // the history without touching the resolved values set above.
    vi.mocked(api.listClients).mockClear();
    vi.mocked(api.listRoles).mockClear();
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={qc}>
        <MemoryRouter>
          <RolesTab warehouseId="shared-wh" />
          <ClientsTab warehouseId="shared-wh" />
        </MemoryRouter>
      </QueryClientProvider>,
    );

    // The role row's Clients column, filled from the same cache the Clients
    // tab writes. "No grants" is the roles table's own cell, so it picks that
    // row out of the two tables without depending on a name both tables show.
    await waitFor(() => {
      const roleRow = screen
        .getAllByRole("row")
        .find((r) => within(r).queryByText("No grants"));
      expect(roleRow).toBeDefined();
      expect(within(roleRow!).getByText("1")).toBeInTheDocument();
    });

    // One RESOLVED key per resource => one fetch, however many components read
    // it. A literal that had drifted in either file would fetch twice here and
    // still render the same screen.
    await waitFor(() => expect(vi.mocked(api.listClients)).toHaveBeenCalledTimes(1));
    expect(vi.mocked(api.listClients)).toHaveBeenCalledWith("shared-wh");
    expect(vi.mocked(api.listRoles)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(api.listRoles)).toHaveBeenCalledWith("shared-wh");
  });

  /**
   * Review F1's rule, carried to the second place that broke it.
   *
   * `SettingsTab` read `config.isError` and said *"This warehouse isn't
   * provisioned yet"* — true of the 404 it was written for, and a fabrication
   * on any other failure, which is the same mistake `ConnectTab` made with the
   * warehouse row. The pair below is what makes the narrowing mean something:
   * if the check went back to bare `isError`, the 403 case fails; if the 404
   * branch were dropped, the 404 case fails.
   */
  test.each([
    [404, "not found", /isn't provisioned yet/, null],
    [403, "forbidden", null, /Couldn't read this warehouse's settings/],
  ] as const)(
    "a %i on the warehouse config says the right thing",
    async (status, message, present, instead) => {
      vi.mocked(api.getWarehouseConfig).mockRejectedValue(new ApiError(status, message));
      vi.mocked(api.listWarehouseTasks).mockResolvedValue([]);
      renderTab(<SettingsTab warehouseId="w1" storageConnId="st-1" />);

      if (present) {
        expect(await screen.findByText(present)).toBeInTheDocument();
        expect(screen.queryByTestId("warehouse-config-error")).toBeNull();
      } else {
        const err = await screen.findByTestId("warehouse-config-error");
        expect(err).toHaveTextContent(instead!);
        // The server's own words, and NOT a claim about the warehouse.
        expect(err).toHaveTextContent(message);
        expect(screen.queryByText(/isn't provisioned yet/)).toBeNull();
      }
    },
  );

  /**
   * SP3c-2 Task 7 fix round 1. The four panes that used to render
   * `ApiError.message` raw on a non-404 error — `Error::Forbidden`'s Display,
   * the bare word "forbidden", naming no permission and offering no way
   * forward. Each is driven the way the page actually reaches it: the
   * underlying `api.*` call (NOT `useQuery` itself, which is real here) is
   * made to reject with a 403 `ApiError`, so `isError`/`error` are genuine
   * react-query state rather than a hand-built mock return value. A future
   * `enabled: canRead…` added to any of these queries would starve the
   * `queryFn` and turn these tests red — that is the point of driving the
   * rejection through the real fetch instead of asserting on a string handed
   * straight to the component.
   */
  test("RolesTab names the permission and the remedy on a 403, not the bare word forbidden", async () => {
    vi.mocked(api.listRoles).mockRejectedValue(new ApiError(403, "forbidden"));
    renderTab(<RolesTab warehouseId="w1" />);

    const cell = await screen.findByText(/catalog_registry:read/);
    expect(cell).toHaveTextContent("catalog_registry:read");
    expect(cell).toHaveTextContent(/platform administrator can grant it/i);
    expect(cell.textContent).not.toBe("forbidden");
    await waitFor(() => expect(vi.mocked(api.listRoles)).toHaveBeenCalledWith("w1"));
  });

  test("ClientsTab names the permission and the remedy on a 403, not the bare word forbidden", async () => {
    vi.mocked(api.listClients).mockRejectedValue(new ApiError(403, "forbidden"));
    renderTab(<ClientsTab warehouseId="w1" />);

    const cell = await screen.findByText(/catalog_registry:read/);
    expect(cell).toHaveTextContent("catalog_registry:read");
    expect(cell).toHaveTextContent(/platform administrator can grant it/i);
    expect(cell.textContent).not.toBe("forbidden");
    await waitFor(() => expect(vi.mocked(api.listClients)).toHaveBeenCalledWith("w1"));
  });

  test("BrowseTab names the permission and the remedy on a 403, not the bare word forbidden", async () => {
    vi.mocked(api.listNamespaces).mockRejectedValue(new ApiError(403, "forbidden"));
    renderTab(<BrowseTab warehouseId="w1" />);

    const cell = await screen.findByText(/catalog_registry:read/);
    expect(cell).toHaveTextContent("catalog_registry:read");
    expect(cell).toHaveTextContent(/platform administrator can grant it/i);
    expect(cell.textContent).not.toBe("forbidden");
    await waitFor(() => expect(vi.mocked(api.listNamespaces)).toHaveBeenCalledWith("w1"));
  });

  test("SettingsTab's background-tasks card names the permission and the remedy on a 403", async () => {
    // Independent of the config read above it (already covered by the 404/403
    // pair test): give it a clean 200 so only the tasks query's own failure is
    // under test here.
    vi.mocked(api.getWarehouseConfig).mockResolvedValue({
      active: true,
      deleteProfileSeconds: null,
      numTables: 0,
      numViews: 0,
      statsUpdatedMs: 0,
    });
    vi.mocked(api.listWarehouseTasks).mockRejectedValue(new ApiError(403, "forbidden"));
    renderTab(<SettingsTab warehouseId="w1" storageConnId="st-1" />);

    const cell = await screen.findByText(/catalog_registry:read/);
    expect(cell).toHaveTextContent("catalog_registry:read");
    expect(cell).toHaveTextContent(/platform administrator can grant it/i);
    expect(cell.textContent).not.toBe("forbidden");
    await waitFor(() => expect(vi.mocked(api.listWarehouseTasks)).toHaveBeenCalledWith("w1"));
  });
});
