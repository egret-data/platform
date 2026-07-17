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

function tree(engine: Omit<Engine, "status">, qc: QueryClient) {
  return (
    <QueryClientProvider client={qc}>
      <MemoryRouter>
        <Harness engine={engine} />
      </MemoryRouter>
    </QueryClientProvider>
  );
}

function renderTab(engine = ENGINE) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(tree(engine, qc));
}

/** An engine whose stored spec already carries FE resources. */
const ENGINE_WITH_RES: Omit<Engine, "status"> = {
  ...ENGINE,
  spec: {
    ...ENGINE.spec,
    starrocksConfig: {
      ...ENGINE.spec.starrocksConfig,
      feResources: { memRequest: "4Gi" },
    },
  },
};

/** An engine whose stored spec has a JWT delegation block. */
const ENGINE_WITH_DELEG: Omit<Engine, "status"> = {
  ...ENGINE,
  spec: {
    ...ENGINE.spec,
    starrocksConfig: {
      ...ENGINE.spec.starrocksConfig,
      delegation: { mode: "jwt", tlsEnabled: true },
    },
  },
};

/** An engine whose stored spec already carries a CN autoscaling policy. */
const ENGINE_WITH_AUTOSCALING: Omit<Engine, "status"> = {
  ...ENGINE,
  spec: {
    ...ENGINE.spec,
    starrocksConfig: {
      ...ENGINE.spec.starrocksConfig,
      cnAutoscaling: { maxReplicas: 2, targetCpuPercent: 60 },
    },
  },
};

/** An engine whose stored spec carries pod overrides (edited by the Pod tab). */
const ENGINE_WITH_POD: Omit<Engine, "status"> = {
  ...ENGINE,
  spec: {
    ...ENGINE.spec,
    starrocksConfig: {
      ...ENGINE.spec.starrocksConfig,
      pod: { nodeSelector: { disktype: "ssd" } },
    },
  },
};

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

  test("dirty resets after save + refetch returns the submitted spec", async () => {
    const user = userEvent.setup();
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const { rerender } = render(tree(ENGINE, qc));

    await user.click(screen.getByRole("button", { name: "Compute" }));
    const feReplicas = screen.getByLabelText("FE replicas");
    await user.clear(feReplicas);
    await user.type(feReplicas, "2");

    const save = screen.getByRole("button", { name: "Save changes" });
    expect(save).toBeEnabled();
    await user.click(save);
    await waitFor(() => expect(api.updateEngine).toHaveBeenCalledTimes(1));

    // Simulate the invalidated engine query refetching the now-persisted spec
    // (feReplicas: 2). The baseline must re-sync so dirty settles to false.
    const refetched: Omit<Engine, "status"> = {
      ...ENGINE,
      spec: {
        ...ENGINE.spec,
        starrocksConfig: { ...ENGINE.spec.starrocksConfig, feReplicas: 2 },
      },
    };
    rerender(tree(refetched, qc));

    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Save changes" })).toBeDisabled(),
    );
  });

  test("blanking a resource that was set does NOT strand dirty", async () => {
    const user = userEvent.setup();
    renderTab(ENGINE_WITH_RES);

    await user.click(screen.getByRole("button", { name: "Resources" }));
    const memReq = screen.getByLabelText("FE memory request");
    expect(memReq).toHaveValue("4Gi");

    // Clearing the only set resource field ⇒ the PUT would omit feResources ⇒
    // the server preserves the stored value ⇒ no actual change ⇒ not dirty.
    await user.clear(memReq);

    expect(screen.getByRole("button", { name: "Save changes" })).toBeDisabled();
  });

  test("a config-only save keeps the stored delegation in the PUT body", async () => {
    const user = userEvent.setup();
    renderTab(ENGINE_WITH_DELEG);

    // Edit an image field only (Image is the default section) — delegation is
    // untouched here but must not be dropped from the body.
    const feImage = screen.getByLabelText("FE image");
    await user.clear(feImage);
    await user.type(feImage, "starrocks/fe-ubuntu:3.4.1");

    await user.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() => expect(api.updateEngine).toHaveBeenCalledTimes(1));
    const [, body] = vi.mocked(api.updateEngine).mock.calls[0];
    expect(body.starrocks).toEqual(
      expect.objectContaining({
        feImage: "starrocks/fe-ubuntu:3.4.1",
        delegationMode: "jwt",
        tlsEnabled: true,
      }),
    );
  });

  test("adding an FE config override PUTs starrocks.feConfig", async () => {
    const user = userEvent.setup();
    renderTab();

    await user.click(screen.getByRole("button", { name: "FE config" }));
    await user.click(screen.getByRole("button", { name: /Add property/i }));
    await user.type(screen.getByPlaceholderText("Search properties…"), "sys_log_level");
    await user.click(screen.getByRole("button", { name: /Add custom key/i }));
    // The added row renders a plain value input (no catalog) — fill it.
    const valueInput = screen.getByPlaceholderText("value");
    await user.type(valueInput, "WARN");

    await user.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() => expect(api.updateEngine).toHaveBeenCalledTimes(1));
    const [, body] = vi.mocked(api.updateEngine).mock.calls[0];
    expect(body.starrocks?.feConfig).toEqual({ sys_log_level: "WARN" });
  });

  test("enabling autoscaling PUTs cnAutoscaling with max + targetCpu", async () => {
    const user = userEvent.setup();
    renderTab();

    await user.click(screen.getByRole("button", { name: "Autoscaling" }));
    await user.click(screen.getByLabelText("Enable CN autoscaling"));
    await user.type(screen.getByLabelText("Max replicas"), "3");
    await user.type(screen.getByLabelText("Target CPU %"), "70");

    await user.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() => expect(api.updateEngine).toHaveBeenCalledTimes(1));
    const [, body] = vi.mocked(api.updateEngine).mock.calls[0];
    expect(body.starrocks?.cnAutoscaling).toEqual({
      maxReplicas: 3,
      targetCpuPercent: 70,
    });
  });

  test("enabling autoscaling with a blank Max disables Save and never emits the {maxReplicas:0} sentinel", async () => {
    const user = userEvent.setup();
    renderTab();

    await user.click(screen.getByRole("button", { name: "Autoscaling" }));
    await user.click(screen.getByLabelText("Enable CN autoscaling"));
    // Max left blank; fill only CPU so the ONLY thing missing is Max.
    await user.type(screen.getByLabelText("Target CPU %"), "70");

    // Save is gated (blank Max) — the red hint has teeth.
    expect(screen.getByRole("button", { name: "Save changes" })).toBeDisabled();

    // Even if forced, the body must NOT carry the {maxReplicas:0} disable sentinel
    // (no stored policy ⇒ half-filled enable resolves to omit).
    await user.type(screen.getByLabelText("Max replicas"), "3");
    await user.clear(screen.getByLabelText("Max replicas"));
    expect(screen.getByRole("button", { name: "Save changes" })).toBeDisabled();
    expect(api.updateEngine).not.toHaveBeenCalled();
  });

  test("enabling autoscaling with a blank target CPU disables Save", async () => {
    const user = userEvent.setup();
    renderTab();

    await user.click(screen.getByRole("button", { name: "Autoscaling" }));
    await user.click(screen.getByLabelText("Enable CN autoscaling"));
    await user.type(screen.getByLabelText("Max replicas"), "3");
    // target CPU blank ⇒ Save disabled (backend requires it when enabled).

    expect(screen.getByRole("button", { name: "Save changes" })).toBeDisabled();
  });

  test("disabling autoscaling on a stored-autoscaling engine PUTs the {maxReplicas:0} sentinel", async () => {
    const user = userEvent.setup();
    renderTab(ENGINE_WITH_AUTOSCALING);

    await user.click(screen.getByRole("button", { name: "Autoscaling" }));
    const toggle = screen.getByLabelText("Enable CN autoscaling");
    expect(toggle).toBeChecked();
    await user.click(toggle); // turn OFF

    await user.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() => expect(api.updateEngine).toHaveBeenCalledTimes(1));
    const [, body] = vi.mocked(api.updateEngine).mock.calls[0];
    expect(body.starrocks?.cnAutoscaling).toEqual({ maxReplicas: 0 });
  });

  test("a config-only save carries the stored pod block through the PUT", async () => {
    const user = userEvent.setup();
    renderTab(ENGINE_WITH_POD);

    const feImage = screen.getByLabelText("FE image");
    await user.clear(feImage);
    await user.type(feImage, "starrocks/fe-ubuntu:3.4.1");

    await user.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() => expect(api.updateEngine).toHaveBeenCalledTimes(1));
    const [, body] = vi.mocked(api.updateEngine).mock.calls[0];
    expect(body.starrocks?.pod).toEqual({ nodeSelector: { disktype: "ssd" } });
  });

  test("editing Pod & Containers serviceAccount + nodeSelector PUTs starrocks.pod", async () => {
    const user = userEvent.setup();
    renderTab();

    await user.click(screen.getByRole("button", { name: "Pod & Containers" }));

    // serviceAccount text field.
    await user.type(screen.getByLabelText("Service account"), "sr-sa");

    // nodeSelector is the first keyed-property editor in the section.
    const addButtons = screen.getAllByRole("button", { name: /Add property/i });
    await user.click(addButtons[0]);
    await user.type(
      screen.getByPlaceholderText("Search properties…"),
      "disktype",
    );
    await user.click(screen.getByRole("button", { name: /Add custom key/i }));
    await user.type(screen.getByPlaceholderText("value"), "ssd");

    await user.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() => expect(api.updateEngine).toHaveBeenCalledTimes(1));
    const [, body] = vi.mocked(api.updateEngine).mock.calls[0];
    expect(body.starrocks?.pod).toEqual({
      serviceAccount: "sr-sa",
      nodeSelector: { disktype: "ssd" },
    });
  });

  test("invalid affinity JSON in Pod & Containers disables Save until valid/blank", async () => {
    const user = userEvent.setup();
    renderTab();

    await user.click(screen.getByRole("button", { name: "Pod & Containers" }));

    // A real edit makes the form dirty ⇒ Save would be enabled.
    await user.type(screen.getByLabelText("Service account"), "sr-sa");
    expect(screen.getByRole("button", { name: "Save changes" })).toBeEnabled();

    // Invalid JSON in the affinity raw field registers invalid ⇒ Save blocked.
    const affinity = screen.getByLabelText("Affinity");
    await user.type(affinity, "notjson");
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Save changes" })).toBeDisabled(),
    );

    // Clearing the raw field back to blank clears the invalid flag ⇒ Save re-enabled.
    await user.clear(affinity);
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Save changes" })).toBeEnabled(),
    );
  });
});
