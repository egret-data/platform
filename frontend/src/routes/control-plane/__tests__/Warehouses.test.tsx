import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

import Warehouses from "@/routes/control-plane/Warehouses";
import { catalogsQueryKey, warehousesQueryKey } from "@/routes/control-plane/queryKeys";
import * as api from "@/api/controlPlane";
import * as hooks from "@/api/hooks";

vi.mock("@/api/controlPlane");
vi.mock("@/api/hooks");

const WAREHOUSE: api.Warehouse = {
  id: "w1",
  name: "analytics",
  workspaceId: "ws1",
  storageConnId: "s1",
  lakekeeperWarehouseId: "lk-123",
};

function renderPage(entry = "/control-plane/warehouses") {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const invalidate = vi.spyOn(qc, "invalidateQueries");
  const utils = render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[entry]}>
        <Warehouses />
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return { ...utils, qc, invalidate };
}

describe("Warehouses", () => {
  beforeEach(() => {
    vi.mocked(api.listStorage).mockResolvedValue([]);
    vi.mocked(hooks.useMyWorkspaces).mockReturnValue({
      data: [{ id: "ws1", slug: "eng", name: "Engineering" }],
    } as ReturnType<typeof hooks.useMyWorkspaces>);
  });

  test("renders warehouses with workspace name + provisioned badge", async () => {
    vi.mocked(api.listWarehouses).mockResolvedValue([WAREHOUSE]);
    renderPage();
    expect(screen.getByText("Warehouses")).toBeInTheDocument();
    expect(await screen.findByText("analytics")).toBeInTheDocument();
    expect(screen.getByText("Engineering")).toBeInTheDocument();
    // "Provisioned" appears as both the column header and the status badge.
    expect(screen.getAllByText("Provisioned").length).toBeGreaterThanOrEqual(2);
  });

  test("shows the empty state", async () => {
    vi.mocked(api.listWarehouses).mockResolvedValue([]);
    renderPage();
    expect(await screen.findByText(/No warehouses yet/i)).toBeInTheDocument();
  });

  test("?workspace= filter scopes the list to one workspace + shows a filter banner", async () => {
    vi.mocked(hooks.useMyWorkspaces).mockReturnValue({
      data: [
        { id: "ws1", slug: "eng", name: "Engineering" },
        { id: "ws2", slug: "sales", name: "Sales" },
      ],
    } as ReturnType<typeof hooks.useMyWorkspaces>);
    vi.mocked(api.listWarehouses).mockResolvedValue([
      WAREHOUSE,
      { ...WAREHOUSE, id: "w2", name: "sales-wh", workspaceId: "ws2" },
    ]);
    renderPage("/control-plane/warehouses?workspace=ws1");

    // Only ws1's warehouse is shown; ws2's is filtered out.
    expect(await screen.findByText("analytics")).toBeInTheDocument();
    expect(screen.queryByText("sales-wh")).not.toBeInTheDocument();

    // Filter banner names the workspace and offers a clear link.
    expect(screen.getByText(/Filtered to workspace/i)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Show all workspaces/i })).toHaveAttribute(
      "href",
      "/control-plane/warehouses",
    );
  });

  test("opens the Add dialog", async () => {
    vi.mocked(api.listWarehouses).mockResolvedValue([]);
    const user = userEvent.setup();
    renderPage();
    await user.click(screen.getByRole("button", { name: /Add warehouse/i }));
    await waitFor(() =>
      expect(screen.getByRole("heading", { name: /Add warehouse/i })).toBeInTheDocument(),
    );
  });

  // SP3c-1 F7, the mirror image of F6 below: CREATING a warehouse creates its
  // registry catalog too, so a create that refreshes only this key leaves the
  // Catalogs list short a row for the same thirty seconds, with the sign
  // flipped. Task 5's `CatalogCreate.tsx` already invalidates both; this is the
  // other door into the same pair of caches.
  test("creating a warehouse refreshes the Catalogs list too", async () => {
    vi.mocked(api.listWarehouses).mockResolvedValue([]);
    vi.mocked(api.listStorage).mockResolvedValue([
      { id: "s1", name: "lake", bucket: "egret-lake" } as api.Storage,
    ]);
    vi.mocked(api.createWarehouse).mockResolvedValue(WAREHOUSE);
    const user = userEvent.setup();
    const { invalidate } = renderPage();

    await user.click(screen.getByRole("button", { name: /Add warehouse/i }));
    const dialog = await screen.findByRole("dialog");
    await user.type(within(dialog).getByLabelText("Name"), "analytics");
    await user.click(within(dialog).getByLabelText("Workspace"));
    await user.click(await screen.findByRole("option", { name: "Engineering" }));
    await user.click(within(dialog).getByLabelText("Storage connection"));
    await user.click(await screen.findByRole("option", { name: /lake/ }));
    await user.click(within(dialog).getByRole("button", { name: /^Add warehouse$/ }));

    await waitFor(() => expect(api.createWarehouse).toHaveBeenCalled());
    await waitFor(() => expect(invalidate).toHaveBeenCalledWith({ queryKey: catalogsQueryKey }));
    expect(invalidate).toHaveBeenCalledWith({ queryKey: warehousesQueryKey });
  });

  // SP3c-1 F6: every egret warehouse mirrors a registry catalog (migration
  // 0147's CHECK) and the delete cascades to it, so a delete here must refresh
  // the Catalogs list as well. Through SP3c-1 both sections are live (D5) and
  // the app-wide `staleTime` is 30s, which is long enough to click the sidebar
  // and see the row you just deleted. `catalogs/Catalogs.tsx` does the mirror
  // of this; Task 8 removes both.
  test("deleting a warehouse refreshes the Catalogs list too", async () => {
    vi.mocked(api.listWarehouses).mockResolvedValue([WAREHOUSE]);
    vi.mocked(api.deleteWarehouse).mockResolvedValue(undefined);
    const user = userEvent.setup();
    const { invalidate } = renderPage();

    await user.click(await screen.findByRole("button", { name: "Delete analytics" }));
    await user.type(await screen.findByPlaceholderText("delete"), "delete");
    await user.click(screen.getByRole("button", { name: /^Delete$/ }));

    await waitFor(() => expect(invalidate).toHaveBeenCalledWith({ queryKey: catalogsQueryKey }));
    expect(invalidate).toHaveBeenCalledWith({ queryKey: warehousesQueryKey });
  });
});
