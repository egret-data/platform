import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

import EngineCreate from "@/routes/control-plane/EngineCreate";
import * as api from "@/api/controlPlane";

vi.mock("@/api/controlPlane");

const WAREHOUSE: api.Warehouse = {
  id: "w1",
  name: "demo-wh",
  workspaceId: "ws1",
  storageConnId: "sc1",
  lakekeeperWarehouseId: "lk1",
} as unknown as api.Warehouse;

function renderCreate(type: "trino" | "starrocks") {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[`/control-plane/engines/new?type=${type}`]}>
        <Routes>
          <Route path="/control-plane/engines/new" element={<EngineCreate />} />
          <Route path="/control-plane/engines/:id" element={<div>DETAIL</div>} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

async function pickWarehouse(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByLabelText("Catalog warehouse"));
  await user.click(await screen.findByRole("option", { name: "demo-wh" }));
}

describe("EngineCreate", () => {
  beforeEach(() => {
    vi.mocked(api.listWarehouses).mockResolvedValue([WAREHOUSE]);
    vi.mocked(api.listSecrets).mockResolvedValue([]);
    vi.mocked(api.createEngine).mockClear();
    vi.mocked(api.createEngine).mockResolvedValue({
      id: "e2",
      name: "x",
      type: "starrocks",
      namespace: "engines",
      warehouseId: WAREHOUSE.id,
      status: "provisioning",
      spec: {},
    } as unknown as api.Engine);
  });

  test("Trino: submits kind trino with a trinoConfig (coordinator/worker)", async () => {
    const user = userEvent.setup();
    renderCreate("trino");
    await user.type(screen.getByLabelText("Name"), "analytics");
    await pickWarehouse(user);
    // StarRocks-only fields must not be present on the Trino path.
    expect(screen.queryByLabelText("FE image")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Create engine" }));
    await waitFor(() => expect(api.createEngine).toHaveBeenCalledTimes(1));
    const body = vi.mocked(api.createEngine).mock.calls[0][0];
    expect(body.kind).toBeUndefined();
    expect(body.warehouseId).toBe(WAREHOUSE.id);
    expect(body.trinoConfig?.coordinator).toBeDefined();
    expect(body.trinoConfig?.worker).toBeDefined();
  });

  test("StarRocks: submits kind + starrocks body with defaults", async () => {
    const user = userEvent.setup();
    renderCreate("starrocks");
    await user.type(screen.getByLabelText("Name"), "sr1");
    await pickWarehouse(user);
    expect(screen.getByLabelText("FE image")).toBeInTheDocument();
    expect(screen.getByLabelText("CN image")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Create engine" }));
    await waitFor(() => expect(api.createEngine).toHaveBeenCalledTimes(1));
    const body = vi.mocked(api.createEngine).mock.calls[0][0];
    expect(body.kind).toBe("starrocks");
    expect(body.starrocks).toEqual({
      feImage: "starrocks/fe-ubuntu:4.1-latest",
      cnImage: "starrocks/cn-ubuntu:4.1-latest",
      feReplicas: 1,
      cnReplicas: 1,
    });
    // Required Trino-only body fields still sent with harmless defaults.
    expect(body.coordinatorReplicas).toBe(1);
    expect(body.memory).toBe("4Gi");
  });

  test("StarRocks: a delegation mode adds delegationMode + tlsEnabled", async () => {
    const user = userEvent.setup();
    renderCreate("starrocks");
    await user.type(screen.getByLabelText("Name"), "sr1");
    await pickWarehouse(user);

    expect(screen.queryByLabelText("Enable TLS")).not.toBeInTheDocument();
    await user.click(screen.getByLabelText("Per-user delegation"));
    await user.click(await screen.findByRole("option", { name: "JWT (per-user)" }));
    expect(screen.getByLabelText("Enable TLS")).toBeChecked();

    await user.click(screen.getByRole("button", { name: "Create engine" }));
    await waitFor(() => expect(api.createEngine).toHaveBeenCalledTimes(1));
    const body = vi.mocked(api.createEngine).mock.calls[0][0];
    expect(body.starrocks).toEqual(
      expect.objectContaining({ delegationMode: "jwt", tlsEnabled: true }),
    );
  });

  test("StarRocks: explicit None sends delegationMode 'none' literally", async () => {
    const user = userEvent.setup();
    renderCreate("starrocks");
    await user.type(screen.getByLabelText("Name"), "sr2");
    await pickWarehouse(user);

    await user.click(screen.getByLabelText("Per-user delegation"));
    await user.click(
      await screen.findByRole("option", { name: "None (open engine)" }),
    );
    expect(screen.queryByLabelText("Enable TLS")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Create engine" }));
    await waitFor(() => expect(api.createEngine).toHaveBeenCalledTimes(1));
    const body = vi.mocked(api.createEngine).mock.calls[0][0];
    expect(body.starrocks).toEqual(
      expect.objectContaining({ delegationMode: "none" }),
    );
  });
});
