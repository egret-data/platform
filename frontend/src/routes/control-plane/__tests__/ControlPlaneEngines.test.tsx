import { render, screen, waitFor } from "@testing-library/react";
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
    expect(await screen.findByText("running")).toBeInTheDocument();
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
});
