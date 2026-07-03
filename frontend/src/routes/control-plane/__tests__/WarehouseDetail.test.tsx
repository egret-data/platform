import { render, screen, within, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";
import WarehouseDetail from "@/routes/control-plane/WarehouseDetail";
import * as api from "@/api/controlPlane";
import * as hooks from "@/api/hooks";
vi.mock("@/api/controlPlane");
vi.mock("@/api/hooks");

function renderAt() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/control-plane/warehouses/w1"]}>
        <Routes><Route path="/control-plane/warehouses/:id" element={<WarehouseDetail />} /></Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}
describe("WarehouseDetail browse", () => {
  beforeEach(() => {
    vi.mocked(hooks.useMyWorkspaces).mockReturnValue({ data: [] } as unknown as ReturnType<typeof hooks.useMyWorkspaces>);
    vi.mocked(api.getWarehouse).mockResolvedValue({ id: "w1", name: "test-wh", workspaceId: "x", storageConnId: "y", lakekeeperWarehouseId: "lk" });
    vi.mocked(api.listRoles).mockResolvedValue([
      { id: "r1", name: "engine", consumerType: "engine", description: null },
    ]);
    vi.mocked(api.listGrants).mockResolvedValue([]);
    vi.mocked(api.addGrant).mockResolvedValue(undefined);
    vi.mocked(api.listClients).mockResolvedValue([]);
    vi.mocked(api.listNamespaces).mockResolvedValue([{ parts: ["analytics"], id: "ns1", protected: false }]);
    vi.mocked(api.listTabulars).mockResolvedValue([{ name: "events", kind: "table", id: "t1", protected: false }]);
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
    renderAt();
    await user.click(await screen.findByRole("tab", { name: /Browse/ }));
    // Click the namespace to expand it, then its table.
    await user.click(await screen.findByText("analytics"));
    await user.click(await screen.findByText("events"));
    // Table detail loads with the schema tab showing the column.
    expect(await screen.findByText("id")).toBeInTheDocument();
    expect(screen.getByText("long")).toBeInTheDocument();
  });

  test("add grant via browse picker", async () => {
    const user = userEvent.setup();
    renderAt();
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
});
