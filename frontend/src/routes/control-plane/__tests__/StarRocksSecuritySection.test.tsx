import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

import type { Engine } from "@/api/controlPlane";
import * as api from "@/api/controlPlane";
import { SaveBar } from "@/routes/control-plane/EngineDetail";
import { StarRocksSecuritySection } from "@/routes/control-plane/StarRocksSecuritySection";
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
      delegation: { mode: "none" },
    },
  },
};

/**
 * Instantiate the hook (as EngineTabs does) and render the section against it,
 * alongside the shared `SaveBar` — mirroring how `StarRocksConfigTab` renders the
 * folded-in Security & Delegation section under the single Save.
 */
function Harness({ engine }: { engine: Omit<Engine, "status"> }) {
  const cfg = useStarRocksConfigForm(engine);
  return (
    <>
      <StarRocksSecuritySection cfg={cfg} />
      <SaveBar cfg={cfg} />
    </>
  );
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

describe("StarRocksSecuritySection", () => {
  beforeEach(() => {
    vi.mocked(api.updateEngine).mockReset();
    vi.mocked(api.updateEngine).mockResolvedValue({
      ...ENGINE,
      status: "provisioning",
    } as unknown as Engine);
  });

  test("seeds delegation from spec; TLS switch hidden when mode is none", () => {
    renderTab();
    expect(screen.getByLabelText("Per-user delegation")).toBeInTheDocument();
    // mode === "none" → no TLS switch.
    expect(screen.queryByLabelText("Enable TLS")).not.toBeInTheDocument();
    // Save disabled — nothing changed.
    expect(screen.getByRole("button", { name: "Save changes" })).toBeDisabled();
  });

  test("engine WITH delegation: None is selectable and actually disables delegation (no longer a no-op)", async () => {
    const user = userEvent.setup();
    const withDeleg: Omit<Engine, "status"> = {
      ...ENGINE,
      spec: {
        ...ENGINE.spec,
        starrocksConfig: {
          ...ENGINE.spec.starrocksConfig,
          delegation: { mode: "jwt", tlsEnabled: true },
        },
      },
    };
    renderTab(withDeleg);

    // "None" is a freely reachable choice — picking it now sends "none"
    // literally (build_starrocks_spec rewrites the block), so it isn't
    // disabled the way it was under the old omit=preserve semantics.
    await user.click(screen.getByLabelText("Per-user delegation"));
    const noneOpt = await screen.findByRole("option", {
      name: "None (open engine)",
    });
    expect(noneOpt).not.toHaveAttribute("aria-disabled", "true");

    await user.click(noneOpt);
    // TLS toggle hides again once "none" is picked.
    expect(screen.queryByLabelText("Enable TLS")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() => expect(api.updateEngine).toHaveBeenCalledTimes(1));
    const [, body] = vi.mocked(api.updateEngine).mock.calls[0];
    expect(body.starrocks).toEqual(
      expect.objectContaining({ delegationMode: "none" }),
    );
  });

  test("changing Auto→Impersonate reveals TLS and Save PUTs the new mode", async () => {
    const user = userEvent.setup();
    renderTab();

    // Radix Select trigger is a combobox associated with the label.
    await user.click(screen.getByLabelText("Per-user delegation"));
    await user.click(
      await screen.findByRole("option", { name: "Impersonate (service user)" }),
    );

    // TLS switch appears (defaults on) once a real mode is chosen.
    expect(screen.getByLabelText("Enable TLS")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() => expect(api.updateEngine).toHaveBeenCalledTimes(1));
    const [id, body] = vi.mocked(api.updateEngine).mock.calls[0];
    expect(id).toBe("sr1");
    expect(body).toEqual(
      expect.objectContaining({
        kind: "starrocks",
        starrocks: expect.objectContaining({
          delegationMode: "impersonate",
          tlsEnabled: true,
        }),
      }),
    );
  });
});
