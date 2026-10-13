import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

import type { Engine } from "@/api/controlPlane";
import * as api from "@/api/controlPlane";
import { StarRocksConfigTab } from "@/routes/control-plane/StarRocksConfigTab";
import {
  DEFAULT_SR_CN_IMAGE,
  DEFAULT_SR_FE_IMAGE,
  useStarRocksConfigForm,
} from "@/routes/control-plane/useStarRocksConfigForm";

vi.mock("@/api/controlPlane");

const ENGINE: Omit<Engine, "status"> = {
  id: "sr1",
  name: "sr-one",
  type: "starrocks",
  namespace: "engines",
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

function tree(
  engine: Omit<Engine, "status">,
  qc: QueryClient,
  initialEntries: string[] = ["/"],
) {
  return (
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={initialEntries}>
        <Harness engine={engine} />
      </MemoryRouter>
    </QueryClientProvider>
  );
}

function renderTab(engine = ENGINE, initialEntries?: string[]) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(tree(engine, qc, initialEntries));
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

/** An engine whose stored pod already carries structured env + toleration lists
 * (drives the structured-editor seeding + spurious-dirty guard tests). */
const ENGINE_WITH_POD_LISTS: Omit<Engine, "status"> = {
  ...ENGINE,
  spec: {
    ...ENGINE.spec,
    starrocksConfig: {
      ...ENGINE.spec.starrocksConfig,
      pod: {
        env: [{ name: "A", value: "B" }],
        tolerations: [{ key: "k", operator: "Exists" }],
      },
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

  test("merges Compute + Resources into one 'Compute & Resources' nav section", () => {
    renderTab();
    const nav = screen.getByRole("navigation");
    // The merged item is present…
    expect(
      within(nav).getByRole("button", { name: /^Compute & Resources/ }),
    ).toBeInTheDocument();
    // …and there is no longer a standalone "Resources" (or "Compute") item.
    expect(
      within(nav).queryByRole("button", { name: "Resources" }),
    ).not.toBeInTheDocument();
    expect(
      within(nav).queryByRole("button", { name: "Compute" }),
    ).not.toBeInTheDocument();
  });

  test("selecting 'Compute & Resources' renders BOTH replicas and CPU/memory inputs", async () => {
    const user = userEvent.setup();
    renderTab();
    await user.click(
      screen.getByRole("button", { name: /^Compute & Resources/ }),
    );
    // Compute group.
    expect(screen.getByLabelText("FE replicas")).toBeInTheDocument();
    expect(screen.getByLabelText("CN replicas")).toBeInTheDocument();
    // Resources group.
    expect(screen.getByLabelText("FE CPU request")).toBeInTheDocument();
    expect(screen.getByLabelText("CN memory limit")).toBeInTheDocument();
  });

  test("legacy ?section=resources deep-link resolves to the merged section", () => {
    renderTab(ENGINE, ["/?tab=settings&section=resources"]);
    // Both the replicas and the resource fields render under the merged section.
    expect(screen.getByLabelText("FE replicas")).toBeInTheDocument();
    expect(screen.getByLabelText("FE CPU request")).toBeInTheDocument();
  });

  test("merged badge sums the former compute + resources non-default counts", () => {
    // ENGINE_WITH_RES carries one set FE resource ⇒ badge count 1 on the merged item.
    renderTab(ENGINE_WITH_RES);
    const merged = within(screen.getByRole("navigation")).getByRole("button", {
      name: /^Compute & Resources/,
    });
    expect(merged).toHaveTextContent("1");
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

    // Replicas live under the merged Compute & Resources section.
    await user.click(screen.getByRole("button", { name: /^Compute & Resources/ }));
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

    await user.click(screen.getByRole("button", { name: /^Compute & Resources/ }));
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

    // Resources now live under the merged Compute & Resources section; its badge
    // still reflects the set resource, so match by prefix.
    await user.click(screen.getByRole("button", { name: /^Compute & Resources/ }));
    const memReq = screen.getByLabelText("FE memory request");
    expect(memReq).toHaveValue("4Gi");

    // Clearing the only set resource field ⇒ the PUT would omit feResources ⇒
    // the server preserves the stored value ⇒ no actual change ⇒ not dirty.
    await user.clear(memReq);

    expect(screen.getByRole("button", { name: "Save changes" })).toBeDisabled();
  });

  test("typing the default images onto an engine that stores none is a change to save", async () => {
    // The update route never defaults an image (no stored, no body → 400), so
    // the dirty check must not either: pinning an unpinned engine to exactly
    // the server's default is a real change.
    const user = userEvent.setup();
    const { feImage: _fe, cnImage: _cn, ...rest } = ENGINE.spec.starrocksConfig!;
    renderTab({ ...ENGINE, spec: { ...ENGINE.spec, starrocksConfig: rest } });
    expect(screen.getByRole("button", { name: "Save changes" })).toBeDisabled();

    const fe = screen.getByLabelText("FE image");
    expect(fe).toHaveAttribute("placeholder", DEFAULT_SR_FE_IMAGE);
    await user.type(fe, DEFAULT_SR_FE_IMAGE);
    await user.type(screen.getByLabelText("CN image"), DEFAULT_SR_CN_IMAGE);

    await user.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(api.updateEngine).toHaveBeenCalledTimes(1));
    const [, body] = vi.mocked(api.updateEngine).mock.calls[0];
    expect(body.starrocks).toEqual(
      expect.objectContaining({ feImage: DEFAULT_SR_FE_IMAGE, cnImage: DEFAULT_SR_CN_IMAGE }),
    );
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

    // The nav button carries a non-default-count badge, so match by prefix.
    await user.click(screen.getByRole("button", { name: /^Autoscaling/ }));
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

  test("folds Security & Delegation in as the LAST nav item, with one shared Save", async () => {
    const user = userEvent.setup();
    renderTab();

    // Security is the last left-nav item (config sections + Security last).
    const nav = screen.getByRole("navigation");
    const navButtons = within(nav).getAllByRole("button");
    expect(navButtons[navButtons.length - 1]).toHaveTextContent(
      "Security & Delegation",
    );

    // Selecting it reveals the delegation control in the same pane.
    await user.click(
      within(nav).getByRole("button", { name: /Security & Delegation/ }),
    );
    expect(screen.getByLabelText("Per-user delegation")).toBeInTheDocument();

    // ONE Save button covers config + security (single shared SaveBar).
    expect(
      screen.getAllByRole("button", { name: "Save changes" }),
    ).toHaveLength(1);
  });

  test("editing delegation in the Security section drives the shared Save", async () => {
    const user = userEvent.setup();
    renderTab();

    await user.click(
      within(screen.getByRole("navigation")).getByRole("button", {
        name: /Security & Delegation/,
      }),
    );

    // Switch Auto → Impersonate and Save via the single shared SaveBar.
    await user.click(screen.getByLabelText("Per-user delegation"));
    await user.click(
      await screen.findByRole("option", { name: "Impersonate (service user)" }),
    );
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() => expect(api.updateEngine).toHaveBeenCalledTimes(1));
    const [, body] = vi.mocked(api.updateEngine).mock.calls[0];
    expect(body.starrocks).toEqual(
      expect.objectContaining({ delegationMode: "impersonate" }),
    );
  });

  test("invalid affinity JSON in Pod & Containers disables Save until valid/blank", async () => {
    const user = userEvent.setup();
    renderTab();

    await user.click(screen.getByRole("button", { name: "Pod & Containers" }));

    // A real edit makes the form dirty ⇒ Save would be enabled.
    await user.type(screen.getByLabelText("Service account"), "sr-sa");
    expect(screen.getByRole("button", { name: "Save changes" })).toBeEnabled();

    // Invalid JSON in the affinity raw field registers invalid ⇒ Save blocked.
    const affinity = screen.getByLabelText("Affinity (advanced JSON)");
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

  test("Pod & Containers exposes only the Affinity raw textarea (env/tolerations/containers are structured)", async () => {
    const user = userEvent.setup();
    const { container } = renderTab();
    await user.click(screen.getByRole("button", { name: "Pod & Containers" }));

    // The ONLY remaining raw-JSON textarea is the advanced affinity escape.
    const textareas = container.querySelectorAll("textarea");
    expect(textareas).toHaveLength(1);
    const affinity = screen.getByLabelText("Affinity (advanced JSON)");
    expect(affinity.tagName).toBe("TEXTAREA");
    expect(textareas[0]).toBe(affinity);
  });

  test("adding an Env row via the structured editor PUTs starrocks.pod.env", async () => {
    const user = userEvent.setup();
    renderTab();
    await user.click(screen.getByRole("button", { name: "Pod & Containers" }));

    await user.click(screen.getByRole("button", { name: "Add env var" }));
    await user.type(screen.getByLabelText("Variable name"), "TZ");
    await user.type(screen.getByLabelText("Variable value"), "UTC");

    await user.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() => expect(api.updateEngine).toHaveBeenCalledTimes(1));
    const [, body] = vi.mocked(api.updateEngine).mock.calls[0];
    expect(body.starrocks?.pod?.env).toEqual([{ name: "TZ", value: "UTC" }]);
  });

  test("adding a toleration row via the structured editor PUTs starrocks.pod.tolerations", async () => {
    const user = userEvent.setup();
    renderTab();
    await user.click(screen.getByRole("button", { name: "Pod & Containers" }));

    await user.click(screen.getByRole("button", { name: "Add toleration" }));
    await user.type(screen.getByPlaceholderText("key"), "dedicated");
    await user.type(screen.getByPlaceholderText("Equal / Exists"), "Exists");

    await user.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() => expect(api.updateEngine).toHaveBeenCalledTimes(1));
    const [, body] = vi.mocked(api.updateEngine).mock.calls[0];
    expect(body.starrocks?.pod?.tolerations).toEqual([
      { key: "dedicated", operator: "Exists" },
    ]);
  });

  test("adding an init container via the structured editor PUTs starrocks.pod.initContainers", async () => {
    const user = userEvent.setup();
    renderTab();
    await user.click(screen.getByRole("button", { name: "Pod & Containers" }));

    await user.click(screen.getByRole("button", { name: "Add init container" }));
    await user.type(screen.getByLabelText("name"), "wait");
    await user.type(screen.getByLabelText("image"), "busybox:1.36");

    await user.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() => expect(api.updateEngine).toHaveBeenCalledTimes(1));
    const [, body] = vi.mocked(api.updateEngine).mock.calls[0];
    expect(body.starrocks?.pod?.initContainers).toEqual([
      { name: "wait", image: "busybox:1.36" },
    ]);
  });

  test("seeds structured env from the stored pod without a spurious dirty", async () => {
    const user = userEvent.setup();
    renderTab(ENGINE_WITH_POD_LISTS);
    await user.click(screen.getByRole("button", { name: /^Pod & Containers/ }));

    // The env editor is prefilled from the stored [{name:A,value:B}] block…
    expect(screen.getByLabelText("Variable name")).toHaveValue("A");
    expect(screen.getByLabelText("Variable value")).toHaveValue("B");
    // …and a seed → no-edit round-trip is NOT spuriously dirty.
    expect(screen.getByRole("button", { name: "Save changes" })).toBeDisabled();
  });

  test("a just-added blank env row stays visible while typing (working-state buffer)", async () => {
    const user = userEvent.setup();
    renderTab();
    await user.click(screen.getByRole("button", { name: "Pod & Containers" }));

    // Add a row and type only the name — the lossy serializer drops blank-key
    // rows, so without an internal working buffer this row would vanish.
    await user.click(screen.getByRole("button", { name: "Add env var" }));
    const nameInput = screen.getByLabelText("Variable name");
    await user.type(nameInput, "X");
    expect(screen.getByLabelText("Variable name")).toBeInTheDocument();
    expect(screen.getByLabelText("Variable name")).toHaveValue("X");
  });
});
