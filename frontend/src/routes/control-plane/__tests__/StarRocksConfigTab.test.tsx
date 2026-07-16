import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

import type { Engine } from "@/api/controlPlane";
import * as api from "@/api/controlPlane";
import { StarRocksConfigTab } from "@/routes/control-plane/StarRocksConfigTab";
import { useStarRocksConfigForm } from "@/routes/control-plane/useStarRocksConfigForm";

vi.mock("@/api/controlPlane");

const ENGINE: Omit<Engine, "status"> = {
  id: "sr1",
  name: "sr-one",
  type: "starrocks",
  namespace: "engines",
  warehouseId: "w1",
  spec: {
    coordinatorReplicas: 1,
    workerReplicas: 2,
    cpuMin: "1",
    cpuMax: "2",
    memory: "4Gi",
    imageVersion: "3.4-latest",
    starrocksConfig: {
      feImage: "starrocks/fe-ubuntu:3.4-latest",
      cnImage: "starrocks/cn-ubuntu:3.4-latest",
      feReplicas: 1,
      cnReplicas: 1,
    },
  },
};

/** Instantiate the hook (as EngineTabs does) and render the tab against it. */
function Harness({ engine }: { engine: Omit<Engine, "status"> }) {
  const cfg = useStarRocksConfigForm(engine);
  return <StarRocksConfigTab cfg={cfg} />;
}

function renderTab(engine = ENGINE) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>
        <Harness engine={engine} />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("StarRocksConfigTab", () => {
  beforeEach(() => {
    vi.mocked(api.updateEngine).mockReset();
    vi.mocked(api.updateEngine).mockResolvedValue({
      ...ENGINE,
      status: "provisioning",
    } as unknown as Engine);
  });

  test("seeds FE/CN image + replicas from spec.starrocksConfig", () => {
    renderTab();
    expect(screen.getByLabelText("FE image")).toHaveValue(
      "starrocks/fe-ubuntu:3.4-latest",
    );
    expect(screen.getByLabelText("CN image")).toHaveValue(
      "starrocks/cn-ubuntu:3.4-latest",
    );
  });

  test("editing FE replicas + Save PUTs kind:starrocks with feReplicas", async () => {
    const user = userEvent.setup();
    renderTab();

    // Replicas live under the Compute section.
    await user.click(screen.getByRole("button", { name: "Compute" }));
    const feReplicas = screen.getByLabelText("FE replicas");
    await user.clear(feReplicas);
    await user.type(feReplicas, "2");

    await user.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() => expect(api.updateEngine).toHaveBeenCalledTimes(1));
    const [id, body] = vi.mocked(api.updateEngine).mock.calls[0];
    expect(id).toBe("sr1");
    expect(body).toEqual(
      expect.objectContaining({
        kind: "starrocks",
        starrocks: expect.objectContaining({ feReplicas: 2 }),
      }),
    );
    // Required Trino-only body fields still ride along.
    expect(body.coordinatorReplicas).toBe(1);
    expect(body.workerReplicas).toBe(2);
  });

  test("Save is disabled until a field changes", async () => {
    renderTab();
    expect(screen.getByRole("button", { name: "Save changes" })).toBeDisabled();
  });
});
