import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

import ControlPlaneEngines from "@/routes/control-plane/ControlPlaneEngines";
import * as api from "@/api/controlPlane";

vi.mock("@/api/controlPlane");

const ENGINE: api.Engine = {
  id: "e1",
  name: "analytics-trino",
  type: "trino",
  namespace: "engines",
  warehouseId: "w1",
  status: "running",
  spec: {
    coordinatorReplicas: 1,
    workerReplicas: 2,
    cpuMin: "1",
    cpuMax: "2",
    memory: "4Gi",
    imageVersion: "latest",
  },
};

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>
        <ControlPlaneEngines />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("ControlPlaneEngines", () => {
  beforeEach(() => {
    vi.mocked(api.listEngines).mockResolvedValue([ENGINE]);
    // `getEngine`'s return type intersects Engine (status: string) with a live
    // EngineStatus, so a plain literal can't satisfy it — cast for the mock.
    vi.mocked(api.getEngine).mockResolvedValue({
      ...ENGINE,
      status: { phase: "running", message: null, pods: [] },
    } as unknown as Awaited<ReturnType<typeof api.getEngine>>);
    vi.mocked(api.listWarehouses).mockResolvedValue([]);
  });

  test("renders the engine list", async () => {
    renderPage();
    expect(screen.getByText("Engines")).toBeInTheDocument();
    expect(await screen.findByText("analytics-trino")).toBeInTheDocument();
  });

  test("shows a live status badge", async () => {
    renderPage();
    // "running" also appears as a SummaryStrip label, so scope to the engine row.
    const row = (await screen.findByText("analytics-trino")).closest("tr")!;
    expect(within(row).getByText("running")).toBeInTheDocument();
  });

  test("opens the Add engine dialog on the Trino tab", async () => {
    const user = userEvent.setup();
    renderPage();
    await user.click(screen.getByRole("button", { name: /Add engine/i }));
    await waitFor(() =>
      expect(
        screen.getByRole("heading", { name: /Add Trino engine/i }),
      ).toBeInTheDocument(),
    );
  });

  test("StarRocks tab: fills the FE/CN form and submits kind + starrocks body", async () => {
    const user = userEvent.setup();
    const warehouse: api.Warehouse = {
      id: "w1",
      name: "demo-wh",
      workspaceId: "ws1",
      storageConnId: "sc1",
      lakekeeperWarehouseId: "lk1",
    };
    vi.mocked(api.listWarehouses).mockResolvedValue([warehouse]);
    const created: api.Engine = {
      id: "e2",
      name: "sr1",
      type: "starrocks",
      namespace: "engines",
      warehouseId: warehouse.id,
      status: "provisioning",
      spec: { coordinatorReplicas: 1, workerReplicas: 2, cpuMin: "1", cpuMax: "2", memory: "4Gi", imageVersion: "latest" },
    };
    vi.mocked(api.createEngine).mockResolvedValue(created);

    renderPage();

    // Switch to the StarRocks tab, then open the Add dialog.
    await user.click(screen.getByRole("tab", { name: "StarRocks" }));
    await user.click(screen.getByRole("button", { name: /Add engine/i }));
    await screen.findByRole("heading", { name: /Add StarRocks engine/i });

    await user.type(screen.getByLabelText("Name"), "sr1");
    await user.click(screen.getByLabelText("Warehouse"));
    await user.click(await screen.findByRole("option", { name: "demo-wh" }));

    // Images/replicas keep their defaults — just confirm the StarRocks fields
    // are the ones rendered (Trino-only fields must not appear).
    expect(screen.getByLabelText("FE image")).toBeInTheDocument();
    expect(screen.getByLabelText("CN image")).toBeInTheDocument();
    expect(screen.queryByLabelText("Coordinator replicas")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Add engine" }));

    await waitFor(() => expect(api.createEngine).toHaveBeenCalledTimes(1));
    const body = vi.mocked(api.createEngine).mock.calls[0][0];
    expect(body.kind).toBe("starrocks");
    expect(body.warehouseId).toBe(warehouse.id);
    expect(body.starrocks).toEqual({
      feImage: "starrocks/fe-ubuntu:3.3-latest",
      cnImage: "starrocks/cn-ubuntu:3.3-latest",
      feReplicas: 1,
      cnReplicas: 1,
    });
    // Required Trino-only fields still sent with harmless defaults.
    expect(body.coordinatorReplicas).toBe(1);
    expect(body.workerReplicas).toBe(2);
    expect(body.cpuMin).toBe("1");
    expect(body.cpuMax).toBe("2");
    expect(body.memory).toBe("4Gi");
  });

  test("StarRocks tab: selecting a delegation mode puts delegationMode + tlsEnabled in the POST body", async () => {
    const user = userEvent.setup();
    const warehouse: api.Warehouse = {
      id: "w1",
      name: "demo-wh",
      workspaceId: "ws1",
      storageConnId: "sc1",
      lakekeeperWarehouseId: "lk1",
    };
    vi.mocked(api.listWarehouses).mockResolvedValue([warehouse]);
    const created: api.Engine = {
      id: "e2",
      name: "sr1",
      type: "starrocks",
      namespace: "engines",
      warehouseId: warehouse.id,
      status: "provisioning",
      spec: { coordinatorReplicas: 1, workerReplicas: 2, cpuMin: "1", cpuMax: "2", memory: "4Gi", imageVersion: "latest" },
    };
    // Reset call history — a prior test in this file also calls createEngine,
    // and vitest's automock doesn't clear call counts between tests.
    vi.mocked(api.createEngine).mockClear();
    vi.mocked(api.createEngine).mockResolvedValue(created);

    renderPage();

    await user.click(screen.getByRole("tab", { name: "StarRocks" }));
    await user.click(screen.getByRole("button", { name: /Add engine/i }));
    await screen.findByRole("heading", { name: /Add StarRocks engine/i });

    await user.type(screen.getByLabelText("Name"), "sr1");
    await user.click(screen.getByLabelText("Warehouse"));
    await user.click(await screen.findByRole("option", { name: "demo-wh" }));

    // Delegation mode defaults to "None" — TLS toggle hidden.
    expect(screen.queryByLabelText("Enable TLS")).not.toBeInTheDocument();

    await user.click(screen.getByLabelText("Per-user delegation"));
    await user.click(await screen.findByRole("option", { name: "JWT over TLS" }));

    // TLS toggle appears once a mode is selected, defaulting to on.
    const tlsSwitch = screen.getByLabelText("Enable TLS");
    expect(tlsSwitch).toBeChecked();

    await user.click(screen.getByRole("button", { name: "Add engine" }));

    await waitFor(() => expect(api.createEngine).toHaveBeenCalledTimes(1));
    const body = vi.mocked(api.createEngine).mock.calls[0][0];
    expect(body.starrocks).toEqual({
      feImage: "starrocks/fe-ubuntu:3.3-latest",
      cnImage: "starrocks/cn-ubuntu:3.3-latest",
      feReplicas: 1,
      cnReplicas: 1,
      delegationMode: "jwt",
      tlsEnabled: true,
    });
  });
});
