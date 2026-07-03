import { render, screen } from "@testing-library/react";
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
    vi.mocked(api.listRoles).mockResolvedValue([]);
    vi.mocked(api.listClients).mockResolvedValue([]);
    vi.mocked(api.listNamespaces).mockResolvedValue([{ parts: ["analytics"] }]);
    vi.mocked(api.listTabulars).mockResolvedValue([{ name: "events", kind: "table" }]);
    vi.mocked(api.loadSchema).mockResolvedValue({ columns: [{ name: "id", type: "long", required: true }], partitionBy: [], sortBy: [] });
  });
  test("browse namespaces -> tables -> schema", async () => {
    const user = userEvent.setup();
    renderAt();
    await user.click(await screen.findByRole("tab", { name: /Browse/ }));
    await user.click(await screen.findByText("analytics"));
    await user.click(await screen.findByText("events"));
    expect(await screen.findByText("id")).toBeInTheDocument();
    expect(screen.getByText("long")).toBeInTheDocument();
  });
});
