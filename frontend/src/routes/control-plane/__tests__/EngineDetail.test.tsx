import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider, useMutation, useQuery } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

import EngineDetail from "@/routes/control-plane/EngineDetail";
import { TooltipProvider } from "@/components/ui/tooltip";
import * as api from "@/api/controlPlane";
import * as engineApi from "@/api/generated/control-plane-engines/control-plane-engines";
import * as wsApi from "@/api/generated/admin-workspaces/admin-workspaces";
import * as registryApi from "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry";
import { toast } from "sonner";
import { ApiError } from "@/api/client";
import { catalogKey, catalogsQueryKey } from "@/routes/control-plane/queryKeys";
import { AuthContext } from "@/auth/useAuth";
import type { Me } from "@/api/hooks";
import type { EngineDependentDto } from "@/api/generated/model/engineDependentDto";
import type { EngineWorkspaceDto } from "@/api/generated/model/engineWorkspaceDto";
import type { WorkspaceDto } from "@/api/generated/model/workspaceDto";
import type { CatalogAttachmentDto, RegistryCatalogDto } from "@/api/generated/model";

vi.mock("@/api/controlPlane");
vi.mock("@/api/generated/control-plane-engines/control-plane-engines");
vi.mock("@/api/generated/admin-workspaces/admin-workspaces");

/**
 * The registry module is mocked **partially** (`importOriginal`, then the four
 * hooks the Catalogs tab reaches), so the query-key BUILDERS stay real:
 * `queryKeys.ts` delegates `catalogKey` to one of them, and an automock would
 * make it `undefined` — which `invalidateQueries` reads as "everything",
 * passing every assertion here while invalidating the wrong thing in the app.
 */
type RegistryModule =
  typeof import("@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry");
vi.mock(
  "@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry",
  async (importOriginal) => ({
    ...(await importOriginal<RegistryModule>()),
    useControlPlaneCatalogRegistryListRegistryCatalogs: vi.fn(),
    useControlPlaneCatalogRegistryRegistryAttachCatalog: vi.fn(),
    useControlPlaneCatalogRegistryDetachCatalog: vi.fn(),
    useControlPlaneCatalogRegistryRetryCatalogApply: vi.fn(),
  }),
);
vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn() }),
}));

// JWT-enabled by default → the engine is assignable.
const ENGINE: api.Engine = {
  id: "e1",
  name: "analytics-trino",
  type: "trino",
  namespace: "engines",
  status: "running",
  spec: {
    coordinatorReplicas: 1,
    workerReplicas: 2,
    cpuMin: "1",
    cpuMax: "2",
    memory: "4Gi",
    imageVersion: "latest",
    trinoConfig: {
      coordinator: {},
      worker: {},
      auth: { passwordUsers: [], jwt: { jwksUri: "http://kc/jwks" } },
    },
  },
};

const WS_A: WorkspaceDto = {
  id: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
  slug: "eng",
  name: "Engineering",
  status: "active",
  created_at: "2026-03-01T00:00:00Z",
};
const WS_B: WorkspaceDto = {
  id: "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
  slug: "data",
  name: "Data",
  status: "active",
  created_at: "2026-03-01T00:00:00Z",
};
// Archived — must never be offered in the Add picker.
const WS_C: WorkspaceDto = {
  id: "cccccccc-cccc-cccc-cccc-cccccccccccc",
  slug: "old",
  name: "Old WS",
  status: "archived",
  created_at: "2026-01-01T00:00:00Z",
};

const ASSIGNED: EngineWorkspaceDto[] = [
  { workspaceId: WS_A.id, name: WS_A.name, slug: WS_A.slug, assignedAt: "2026-03-02T00:00:00Z" },
];

function query<T>(data: T) {
  return {
    data: { data, status: 200, headers: new Headers() },
    isLoading: false,
    isError: false,
    error: null,
  };
}

const setMutate = vi.fn();

function mockGetEngine(spec: api.Engine["spec"]) {
  vi.mocked(api.getEngine).mockResolvedValue({
    ...ENGINE,
    spec,
    status: { phase: "running", message: null, pods: [] },
  } as unknown as Awaited<ReturnType<typeof api.getEngine>>);
}

/**
 * The Overview tab renders the Ranger credential section (Task 7b), which
 * takes two mutation hooks from the automocked engines module. An idle
 * mutation for each, for every test; the section's own behaviour is covered
 * against the real hooks in `RangerCredential.test.tsx`. `clearAllMocks` in
 * the suites below clears calls, not this implementation.
 */
function idleMutation() {
  return { mutate: vi.fn(), reset: vi.fn(), isPending: false, isError: false, error: null };
}
beforeEach(() => {
  vi.mocked(engineApi.useControlPlaneEnginesRotateRangerCredential).mockImplementation(
    () => idleMutation() as unknown as ReturnType<typeof engineApi.useControlPlaneEnginesRotateRangerCredential>,
  );
  vi.mocked(engineApi.useControlPlaneEnginesRetirePreviousRangerCredential).mockImplementation(
    () =>
      idleMutation() as unknown as ReturnType<typeof engineApi.useControlPlaneEnginesRetirePreviousRangerCredential>,
  );
});

/** Keys passed to `invalidateQueries` by the render most recently started. */
let invalidated: unknown[][];

function renderDetail(entry = `/control-plane/engines/${ENGINE.id}`, me: Me | null = null) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  invalidated = [];
  const original = qc.invalidateQueries.bind(qc);
  vi.spyOn(qc, "invalidateQueries").mockImplementation((filters) => {
    invalidated.push((filters as { queryKey?: unknown[] })?.queryKey ?? []);
    return original(filters);
  });
  return render(
    <AuthContext.Provider value={{ me, isLoading: false }}>
      <QueryClientProvider client={qc}>
        <TooltipProvider>
          <MemoryRouter initialEntries={[entry]}>
            <Routes>
              <Route path="/control-plane/engines/:id" element={<EngineDetail />} />
            </Routes>
          </MemoryRouter>
        </TooltipProvider>
      </QueryClientProvider>
    </AuthContext.Provider>,
  );
}

describe("EngineDetail — a provisioned engine (control for the external branch)", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockGetEngine(ENGINE.spec);
    mockRegistryList([]);
    vi.mocked(wsApi.useAdminWorkspacesListWs).mockReturnValue(
      query([WS_A]) as unknown as ReturnType<typeof wsApi.useAdminWorkspacesListWs>,
    );
    vi.mocked(engineApi.useControlPlaneEnginesListEngineWorkspaces).mockReturnValue(
      query(ASSIGNED) as unknown as ReturnType<typeof engineApi.useControlPlaneEnginesListEngineWorkspaces>,
    );
  });

  test("keeps today's tabs and its Restart action, and shows no External chip or Plugin config tab", async () => {
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });
    expect(screen.getAllByRole("tab").map((t) => t.textContent)).toEqual([
      "Overview",
      "Catalogs",
      "Workspaces",
      "Settings",
      "Monitor",
    ]);
    expect(screen.getByRole("button", { name: "Restart" })).toBeInTheDocument();
    expect(screen.queryByText("External")).not.toBeInTheDocument();
    expect(screen.queryByRole("tab", { name: "Plugin config" })).not.toBeInTheDocument();
  });

  // #4a EP12: the two engine pages link to each other.
  test("offers Open in Engine & policy to a policy:read holder, and not without it", async () => {
    const holder: Me = { user_id: "u9", email: "s@example.com", platform_roles: [], permissions: ["engine:read", "policy:read"] };
    renderDetail(undefined, holder);
    await screen.findByRole("heading", { name: "analytics-trino" });
    expect(screen.getByRole("link", { name: "Open in Engine & policy" })).toHaveAttribute("href", "/governance/engines/e1");
  });

  test("hides Open in Engine & policy without policy:read", async () => {
    const infra: Me = { user_id: "u8", email: "i@example.com", platform_roles: [], permissions: ["engine:read"] };
    renderDetail(undefined, infra);
    await screen.findByRole("heading", { name: "analytics-trino" });
    expect(screen.queryByRole("link", { name: "Open in Engine & policy" })).not.toBeInTheDocument();
  });
});

/**
 * B6: an engine the Docker provisioner runs (KI-195, KI-196, KI-198, KI-199,
 * KI-200), with a Kubernetes control where the copy differs.
 */
describe("EngineDetail — an engine Docker runs", () => {
  const PODS = [{ name: "egret-engine-egret-engines-b6-coordinator", phase: "Running", ready: true }];

  function mockRuntime(runtime: api.EngineRuntime, spec: api.Engine["spec"] = ENGINE.spec) {
    vi.mocked(api.getEngine).mockResolvedValue({
      ...ENGINE,
      spec,
      runtime,
      endpoint: "analytics-trino-trino.engines.svc:8443",
      status: { phase: "running", message: null, pods: PODS },
    } as unknown as Awaited<ReturnType<typeof api.getEngine>>);
  }

  const JWT_ONLY: api.Engine["spec"] = {
    ...ENGINE.spec,
    // As the server stores a JWT engine: no `passwordUsers` key at all.
    trinoConfig: { coordinator: {}, worker: {}, auth: { jwt: {} } },
  };

  beforeEach(() => {
    vi.clearAllMocks();
    mockRegistryList([]);
    vi.mocked(engineApi.useControlPlaneEnginesListEngineWorkspaces).mockReturnValue(
      query(ASSIGNED) as unknown as ReturnType<typeof engineApi.useControlPlaneEnginesListEngineWorkspaces>,
    );
  });

  test("lists containers with no Logs or Exec, and says where the log is (KI-195)", async () => {
    mockRuntime("docker");
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });

    const table = await screen.findByRole("table");
    expect(within(table).getByRole("columnheader", { name: "Container" })).toBeInTheDocument();
    expect(within(table).queryByRole("button", { name: "Logs" })).not.toBeInTheDocument();
    expect(within(table).queryByRole("button", { name: "Exec" })).not.toBeInTheDocument();
    expect(screen.getByText(/not available here for an engine Docker runs/i)).toBeInTheDocument();
    expect(screen.getByText("containers")).toBeInTheDocument();
  });

  test("a Kubernetes engine keeps Logs and Exec per pod", async () => {
    mockRuntime("kubernetes");
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });

    const table = await screen.findByRole("table");
    expect(within(table).getByRole("columnheader", { name: "Pod" })).toBeInTheDocument();
    expect(within(table).getByRole("button", { name: "Logs" })).toBeInTheDocument();
    expect(within(table).getByRole("button", { name: "Exec" })).toBeInTheDocument();
  });

  test("Restart and Delete speak of containers, not pods or a Kubernetes cluster (KI-196)", async () => {
    const user = userEvent.setup();
    mockRuntime("docker");
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });

    await user.click(screen.getByRole("button", { name: "Restart" }));
    const restart = await screen.findByRole("alertdialog");
    expect(restart).toHaveTextContent(/restarts the coordinator and worker containers/i);
    expect(restart).toHaveTextContent(/if the containers are gone, they are created again/i);
    expect(restart).not.toHaveTextContent(/pods/i);
    await user.click(within(restart).getByRole("button", { name: "Cancel" }));

    await user.click(screen.getByRole("button", { name: /more engine actions/i }));
    await user.click(await screen.findByRole("menuitem", { name: /delete engine/i }));
    const del = await screen.findByRole("alertdialog");
    expect(del).toHaveTextContent(/removes the engine's containers/i);
    expect(del).not.toHaveTextContent(/kubernetes/i);
  });

  test("the header shows the endpoint the server reports (KI-199)", async () => {
    mockRuntime("docker");
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });

    expect(screen.getByText("analytics-trino-trino.engines.svc:8443")).toBeInTheDocument();
    expect(screen.queryByText(/analytics-trino-coordinator/)).not.toBeInTheDocument();
  });

  test("an ingress with no host is refused beside the field, and Save waits for one (KI-200)", async () => {
    const user = userEvent.setup();
    mockRuntime("docker", JWT_ONLY);
    renderDetail(`/control-plane/engines/${ENGINE.id}?tab=settings&section=security`);
    await screen.findByRole("heading", { name: "analytics-trino" });

    await user.click(await screen.findByRole("switch", { name: /expose via ingress/i }));
    expect(screen.getByText(/enter the host the ingress serves/i)).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Host" })).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByRole("button", { name: "Save changes" })).toBeDisabled();

    await user.type(screen.getByRole("textbox", { name: "Host" }), "b6.example.com");
    expect(screen.queryByText(/enter the host the ingress serves/i)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save changes" })).toBeEnabled();
  });

  test("Save changes lists only the real changes and saves only once confirmed (KI-198)", async () => {
    const user = userEvent.setup();
    vi.mocked(api.updateEngine).mockResolvedValue(ENGINE);
    mockRuntime("docker", JWT_ONLY);
    renderDetail(`/control-plane/engines/${ENGINE.id}?tab=settings&section=security`);
    await screen.findByRole("heading", { name: "analytics-trino" });

    await user.click(await screen.findByRole("switch", { name: /expose via ingress/i }));
    await user.type(screen.getByRole("textbox", { name: "Host" }), "b6.example.com");
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    const dialog = await screen.findByRole("dialog", { name: "Review changes" });
    // The one real change (the new ingress block), and nothing else.
    expect(dialog).toHaveTextContent("1 field will change.");
    expect(dialog).toHaveTextContent(/b6\.example\.com/);
    expect(dialog).not.toHaveTextContent(/passwordUsers/);
    expect(dialog).toHaveTextContent(/recreates the engine's containers/i);
    expect(api.updateEngine).not.toHaveBeenCalled();

    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(api.updateEngine).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "Save changes" }));
    const again = await screen.findByRole("dialog", { name: "Review changes" });
    await user.click(within(again).getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(api.updateEngine).toHaveBeenCalledTimes(1));
    const [, body] = vi.mocked(api.updateEngine).mock.calls[0];
    expect(body.trinoConfig?.auth).toEqual({ jwt: {} });
    expect(body.trinoConfig?.ingress).toMatchObject({ enabled: true, host: "b6.example.com" });
  });
});

describe("EngineDetail — Workspaces tab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockGetEngine(ENGINE.spec);
    mockRegistryList([]);
    vi.mocked(wsApi.useAdminWorkspacesListWs).mockReturnValue(
      query([WS_A, WS_B, WS_C]) as unknown as ReturnType<typeof wsApi.useAdminWorkspacesListWs>,
    );
    vi.mocked(engineApi.useControlPlaneEnginesListEngineWorkspaces).mockReturnValue(
      query(ASSIGNED) as unknown as ReturnType<
        typeof engineApi.useControlPlaneEnginesListEngineWorkspaces
      >,
    );
    vi.mocked(engineApi.useControlPlaneEnginesSetEngineWorkspaces).mockReturnValue({
      mutate: setMutate,
      isPending: false,
    } as unknown as ReturnType<typeof engineApi.useControlPlaneEnginesSetEngineWorkspaces>);
  });

  async function openWorkspacesTab() {
    const user = userEvent.setup();
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });
    await user.click(screen.getByRole("tab", { name: "Workspaces" }));
    await screen.findByText("Engineering");
    return user;
  }

  test("lists assigned workspaces with a Remove action", async () => {
    await openWorkspacesTab();
    expect(screen.getByText("Engineering")).toBeInTheDocument();
    // Data is not assigned → not in the assigned list.
    expect(screen.queryByText("Data")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /add workspace/i })).toBeEnabled();
  });

  test("Remove drops the workspace from the assignment set", async () => {
    const user = await openWorkspacesTab();
    await user.click(screen.getByRole("button", { name: "Remove" }));
    expect(setMutate).toHaveBeenCalledTimes(1);
    const arg = setMutate.mock.calls[0][0] as { id: string; data: { workspaceIds: string[] } };
    expect(arg.id).toBe(ENGINE.id);
    expect(arg.data.workspaceIds).toEqual([]);
  });

  test("Add dialog offers only active, unassigned workspaces and assigns on click", async () => {
    const user = await openWorkspacesTab();
    await user.click(screen.getByRole("button", { name: /add workspace/i }));

    // Dialog lists Data (active, unassigned); NOT Engineering (assigned) or Old WS (archived).
    const dataRow = await screen.findByRole("button", { name: /^Data/ });
    expect(screen.queryByRole("button", { name: /^Old WS/ })).not.toBeInTheDocument();

    await user.click(dataRow);
    const arg = setMutate.mock.calls[0][0] as { id: string; data: { workspaceIds: string[] } };
    expect(new Set(arg.data.workspaceIds)).toEqual(new Set([WS_A.id, WS_B.id]));
  });

  test("Add is disabled when the engine has no JWT auth", async () => {
    mockGetEngine({ ...ENGINE.spec, trinoConfig: { coordinator: {}, worker: {} } });
    const user = userEvent.setup();
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });
    await user.click(screen.getByRole("tab", { name: "Workspaces" }));
    await screen.findByText("Engineering");
    expect(screen.getByRole("button", { name: /add workspace/i })).toBeDisabled();
  });

  test("a blocked Remove shows what still uses the engine instead of a toast", async () => {
    setMutate.mockImplementationOnce(
      (_vars: unknown, opts?: { onError?: (e: unknown) => void }) =>
        opts?.onError?.(
          new ApiError(409, "engine is still in use", "engine_has_dependents", {
            error: "engine is still in use",
            code: "engine_has_dependents",
            dependents: [
              {
                workspaceId: WS_A.id,
                workspaceName: WS_A.name,
                workspaceSlug: WS_A.slug,
                kind: "dataset",
                count: 3,
                sample: ["orders", "refunds", "revenue"],
              },
            ],
          }),
        ),
    );
    const user = await openWorkspacesTab();
    await user.click(screen.getByRole("button", { name: "Remove" }));

    expect(await screen.findByText(/not removed/i)).toBeInTheDocument();
    const table = screen.getByRole("table");
    expect(within(table).getByText("Datasets")).toBeInTheDocument();
    expect(within(table).getByText("orders, refunds, revenue")).toBeInTheDocument();
    // Still assigned: the list row is untouched.
    expect(screen.getByRole("button", { name: "Remove" })).toBeInTheDocument();
  });
});

describe("EngineDetail — StarRocks engine", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockRegistryList([]);
    vi.mocked(engineApi.useControlPlaneEnginesListEngineWorkspaces).mockReturnValue(
      query(ASSIGNED) as unknown as ReturnType<
        typeof engineApi.useControlPlaneEnginesListEngineWorkspaces
      >,
    );
    vi.mocked(api.getEngine).mockResolvedValue({
      ...ENGINE,
      type: "starrocks",
      spec: {
        coordinatorReplicas: 0,
        workerReplicas: 0,
        cpuMin: "",
        cpuMax: "",
        memory: "",
        imageVersion: "starrocks/fe-ubuntu:3.3-latest",
      },
      status: {
        phase: "running",
        message: null,
        pods: [{ name: "sr1-fe-0", phase: "Running", ready: true }],
      },
    } as unknown as Awaited<ReturnType<typeof api.getEngine>>);
  });

  test("exposes exactly 5 tabs with a merged Settings (no Configuration/Security tabs)", async () => {
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });

    // Configuration + Security are merged into ONE Settings tab (Task IA-T4).
    const tabs = screen.getAllByRole("tab");
    expect(tabs.map((t) => t.textContent)).toEqual([
      "Overview",
      "Catalogs",
      "Workspaces",
      "Settings",
      "Monitor",
    ]);
    expect(
      screen.queryByRole("tab", { name: "Configuration" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("tab", { name: "Security & Exposure" }),
    ).not.toBeInTheDocument();

    // Restart is now enabled for StarRocks (backend supports it — Task 4).
    expect(screen.getByRole("button", { name: /restart/i })).toBeEnabled();
  });

  test("Settings tab folds Security & Delegation in as the LAST config-nav item, with one Save", async () => {
    const user = userEvent.setup();
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });

    await user.click(screen.getByRole("tab", { name: "Settings" }));

    // The StarRocks config sections render (FE/CN image is the default section).
    expect(await screen.findByLabelText("FE image")).toBeInTheDocument();

    // Security is folded into the left-nav as the LAST item.
    const nav = screen.getByRole("navigation");
    const navButtons = within(nav).getAllByRole("button");
    expect(navButtons[navButtons.length - 1]).toHaveTextContent(
      "Security & Delegation",
    );

    // Selecting it reveals the delegation control in the SAME pane.
    await user.click(
      within(nav).getByRole("button", { name: /Security & Delegation/ }),
    );
    expect(
      await screen.findByLabelText("Per-user delegation"),
    ).toBeInTheDocument();

    // ONE Save button covers config + security (single shared SaveBar).
    expect(
      screen.getAllByRole("button", { name: "Save changes" }),
    ).toHaveLength(1);
  });

  test("?section=security deep-links straight to the Security & Delegation pane", async () => {
    renderDetail(
      `/control-plane/engines/${ENGINE.id}?tab=settings&section=security`,
    );
    await screen.findByRole("heading", { name: "analytics-trino" });

    expect(
      await screen.findByLabelText("Per-user delegation"),
    ).toBeInTheDocument();
  });
});

describe("EngineDetail — Trino Settings tab (merged Configuration + Security)", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockGetEngine(ENGINE.spec);
    mockRegistryList([]);
    vi.mocked(engineApi.useControlPlaneEnginesListEngineWorkspaces).mockReturnValue(
      query(ASSIGNED) as unknown as ReturnType<
        typeof engineApi.useControlPlaneEnginesListEngineWorkspaces
      >,
    );
  });

  test("exposes exactly 5 tabs with a merged Settings (no Configuration/Security tabs)", async () => {
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });

    const tabs = screen.getAllByRole("tab");
    expect(tabs.map((t) => t.textContent)).toEqual([
      "Overview",
      "Catalogs",
      "Workspaces",
      "Settings",
      "Monitor",
    ]);
    expect(
      screen.queryByRole("tab", { name: "Configuration" }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("tab", { name: /Security & Exposure/ }),
    ).not.toBeInTheDocument();
  });

  test("Settings tab folds Security & Exposure in as the LAST config-nav item, with one Save", async () => {
    const user = userEvent.setup();
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });

    await user.click(screen.getByRole("tab", { name: "Settings" }));

    // Config sections render (Image is the default section).
    const nav = await screen.findByRole("navigation");
    const navButtons = within(nav).getAllByRole("button");
    expect(navButtons[navButtons.length - 1]).toHaveTextContent(
      "Security & Exposure",
    );

    // Selecting Security reveals its controls in the same pane (password users).
    await user.click(
      within(nav).getByRole("button", { name: /Security & Exposure/ }),
    );
    expect(await screen.findByText("Password users")).toBeInTheDocument();

    // ONE Save button covers config + security.
    expect(
      screen.getAllByRole("button", { name: "Save changes" }),
    ).toHaveLength(1);
  });

  test("?section=security deep-links straight to the Security pane", async () => {
    renderDetail(
      `/control-plane/engines/${ENGINE.id}?tab=settings&section=security`,
    );
    await screen.findByRole("heading", { name: "analytics-trino" });

    expect(await screen.findByText("Password users")).toBeInTheDocument();
  });

  test("Security & Exposure rotates secrets only after a pods-will-restart confirmation", async () => {
    vi.mocked(api.rotateEngineSecrets).mockResolvedValue(undefined);
    const user = userEvent.setup();
    renderDetail(
      `/control-plane/engines/${ENGINE.id}?tab=settings&section=security`,
    );
    await screen.findByRole("heading", { name: "analytics-trino" });

    await user.click(
      await screen.findByRole("button", { name: "Rotate secrets" }),
    );
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText(/all pods will restart/i)).toBeInTheDocument();
    expect(api.rotateEngineSecrets).not.toHaveBeenCalled();

    await user.click(within(dialog).getByRole("button", { name: "Rotate" }));
    await waitFor(() =>
      expect(api.rotateEngineSecrets).toHaveBeenCalledWith("e1"),
    );
  });

  test("legacy ?tab=security bookmark resolves to the merged Settings tab", async () => {
    renderDetail(`/control-plane/engines/${ENGINE.id}?tab=security`);
    await screen.findByRole("heading", { name: "analytics-trino" });

    // The old Security tab value maps to Settings — no blank page.
    expect(screen.getByRole("tab", { name: "Settings" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(await screen.findByRole("navigation")).toBeInTheDocument();
  });

  test("legacy ?tab=configuration bookmark resolves to the merged Settings tab", async () => {
    renderDetail(`/control-plane/engines/${ENGINE.id}?tab=configuration`);
    await screen.findByRole("heading", { name: "analytics-trino" });

    expect(screen.getByRole("tab", { name: "Settings" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
  });

  test("an unknown ?tab value falls back to Overview", async () => {
    renderDetail(`/control-plane/engines/${ENGINE.id}?tab=bogus`);
    await screen.findByRole("heading", { name: "analytics-trino" });

    expect(screen.getByRole("tab", { name: "Overview" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    // Overview content (KPI strip) is shown, not a blank page.
    expect(await screen.findByText("phase")).toBeInTheDocument();
  });
});

describe("EngineDetail — header metadata + page-level actions", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockRegistryList([]);
    vi.mocked(engineApi.useControlPlaneEnginesListEngineWorkspaces).mockReturnValue(
      query(ASSIGNED) as unknown as ReturnType<
        typeof engineApi.useControlPlaneEnginesListEngineWorkspaces
      >,
    );
    vi.mocked(api.getEngine).mockResolvedValue({
      ...ENGINE,
      type: "starrocks",
      endpoint: "analytics-trino-fe-service.engines.svc:9030",
      spec: {
        coordinatorReplicas: 0,
        workerReplicas: 0,
        cpuMin: "",
        cpuMax: "",
        memory: "",
        imageVersion: "starrocks/fe-ubuntu:3.3-latest",
      },
      status: {
        phase: "running",
        message: null,
        pods: [{ name: "sr1-fe-0", phase: "Running", ready: true }],
      },
    } as unknown as Awaited<ReturnType<typeof api.getEngine>>);
  });

  test("header shows a metadata row (image · endpoint · delegation)", async () => {
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });

    // image (falls back to imageVersion when starrocksConfig.feImage is absent)
    expect(
      screen.getByText("starrocks/fe-ubuntu:3.3-latest"),
    ).toBeInTheDocument();
    // endpoint host:port as the server reports it (KI-199)
    expect(
      screen.getByText("analytics-trino-fe-service.engines.svc:9030"),
    ).toBeInTheDocument();
    // delegation mode — none configured. Rendered in exactly two places now:
    // the header meta row and the Overview KPI strip's Auth item.
    expect(screen.getAllByText(/no delegation/i)).toHaveLength(2);
  });

  test("Restart lives in the header (single instance) and Delete is in the ⋯ menu", async () => {
    const user = userEvent.setup();
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });

    // exactly one Restart button — the header one; StatusCard no longer has its own
    const restarts = screen.getAllByRole("button", { name: /^restart$/i });
    expect(restarts).toHaveLength(1);
    expect(restarts[0]).toBeEnabled();

    // DangerCard is gone — no "Danger zone" section, no standalone Delete button
    expect(screen.queryByText(/danger zone/i)).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /delete engine/i }),
    ).not.toBeInTheDocument();

    // Delete now lives behind the header overflow menu
    await user.click(
      screen.getByRole("button", { name: /more engine actions/i }),
    );
    expect(
      await screen.findByRole("menuitem", { name: /delete engine/i }),
    ).toBeInTheDocument();
  });
});

describe("EngineDetail — image and Ranger plugin version", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockRegistryList([]);
    vi.mocked(engineApi.useControlPlaneEnginesListEngineWorkspaces).mockReturnValue(
      query(ASSIGNED) as unknown as ReturnType<
        typeof engineApi.useControlPlaneEnginesListEngineWorkspaces
      >,
    );
  });

  function mockWith(extra: Record<string, unknown>) {
    vi.mocked(api.getEngine).mockResolvedValue({
      ...ENGINE,
      type: "starrocks",
      spec: { ...ENGINE.spec, imageVersion: "starrocks/fe-ubuntu:4.1-latest" },
      status: { phase: "running", message: null, pods: [] },
      ...extra,
    } as unknown as Awaited<ReturnType<typeof api.getEngine>>);
  }

  test("an unmodelled engine shows its image and the note, in the warning callout", async () => {
    mockWith({
      image: "starrocks/fe-ubuntu:4.1-latest",
      rangerPluginVersionModelled: false,
      rangerPluginVersionNote: 'image "starrocks/fe-ubuntu:4.1-latest" is not measured; pin the engine to one of: starrocks/fe-ubuntu:4.1.6',
    });
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });
    expect(screen.getByText("starrocks/fe-ubuntu:4.1-latest")).toBeInTheDocument();
    const note = await screen.findByTestId("plugin-version-note");
    expect(note).toHaveAttribute("role", "note");
    expect(note).toHaveTextContent(/pin the engine to one of/);
  });

  test("a modelled engine shows its plugin version and no note", async () => {
    mockWith({
      image: "starrocks/fe-ubuntu:4.1.6",
      rangerPluginVersion: "2.9.0",
      rangerPluginVersionModelled: true,
    });
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });
    expect(screen.getByText("Ranger plugin 2.9.0")).toBeInTheDocument();
    expect(screen.queryByTestId("plugin-version-note")).not.toBeInTheDocument();
  });
});

describe("EngineDetail — StatusCard states", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockRegistryList([]);
    vi.mocked(engineApi.useControlPlaneEnginesListEngineWorkspaces).mockReturnValue(
      query(ASSIGNED) as unknown as ReturnType<
        typeof engineApi.useControlPlaneEnginesListEngineWorkspaces
      >,
    );
  });

  function mockEngineStatus(status: api.EngineStatus) {
    vi.mocked(api.getEngine).mockResolvedValue({
      ...ENGINE,
      status,
    } as unknown as Awaited<ReturnType<typeof api.getEngine>>);
  }

  test("provisioning engine with no pods shows the waiting-for-operator copy", async () => {
    mockEngineStatus({ phase: "Provisioning", message: null, pods: [] });
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });

    expect(
      await screen.findByText(/waiting for the operator to create pods/i),
    ).toBeInTheDocument();
    // The terse legacy empty copy is gone.
    expect(screen.queryByText("No pods yet.")).not.toBeInTheDocument();
  });

  test("failed engine with a status message surfaces that message", async () => {
    mockEngineStatus({
      phase: "Failed",
      message: "ImagePullBackOff: trinodb/trino:bogus not found",
      pods: [],
    });
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });

    // The failure message is shown (in the header description AND the status body).
    expect(
      screen.getAllByText(/ImagePullBackOff: trinodb\/trino:bogus not found/i)
        .length,
    ).toBeGreaterThan(0);
    expect(screen.queryByText("No pods yet.")).not.toBeInTheDocument();
    expect(
      screen.queryByText(/waiting for the operator to create pods/i),
    ).not.toBeInTheDocument();
  });

  test("failed engine with no message falls back to a generic failure line", async () => {
    mockEngineStatus({ phase: "error", message: null, pods: [] });
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });

    expect(
      await screen.findByText(/the engine failed to start/i),
    ).toBeInTheDocument();
  });

  test("running engine with no pods reports the neutral empty line", async () => {
    mockEngineStatus({ phase: "running", message: null, pods: [] });
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });

    expect(await screen.findByText(/no pods reported/i)).toBeInTheDocument();
    expect(screen.queryByText("No pods yet.")).not.toBeInTheDocument();
  });
});

describe("EngineDetail — engine-aware Monitor tab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockRegistryList([]);
    vi.mocked(engineApi.useControlPlaneEnginesListEngineWorkspaces).mockReturnValue(
      query(ASSIGNED) as unknown as ReturnType<
        typeof engineApi.useControlPlaneEnginesListEngineWorkspaces
      >,
    );
  });

  test("StarRocks Monitor renders the pod/cluster health view, not the Trino query monitor", async () => {
    vi.mocked(api.getEngine).mockResolvedValue({
      ...ENGINE,
      type: "starrocks",
      spec: {
        coordinatorReplicas: 0,
        workerReplicas: 0,
        cpuMin: "",
        cpuMax: "",
        memory: "",
        imageVersion: "starrocks/fe-ubuntu:3.3-latest",
        starrocksConfig: {
          feReplicas: 1,
          cnReplicas: 2,
          cnAutoscaling: { minReplicas: 2, maxReplicas: 5, targetCpuPercent: 70 },
        },
      },
      status: {
        phase: "running",
        message: null,
        pods: [
          { name: "sr1-fe-0", phase: "Running", ready: true },
          { name: "sr1-cn-0", phase: "Running", ready: false },
        ],
      },
    } as unknown as Awaited<ReturnType<typeof api.getEngine>>);

    const user = userEvent.setup();
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });
    await user.click(screen.getByRole("tab", { name: "Monitor" }));

    // Health view: pod rows sourced from status.pods.
    expect(await screen.findByText("sr1-fe-0")).toBeInTheDocument();
    expect(screen.getByText("sr1-cn-0")).toBeInTheDocument();

    // FE/CN replica summary + autoscaling line.
    expect(screen.getByText("FE replicas")).toBeInTheDocument();
    expect(screen.getByText("CN replicas")).toBeInTheDocument();
    expect(screen.getByText(/autoscaling: 2.*5.*70% cpu/i)).toBeInTheDocument();

    // Honest coming-soon note.
    expect(
      screen.getByText(/query-level monitoring for starrocks is coming soon/i),
    ).toBeInTheDocument();

    // The Trino query monitor is NOT rendered — no query API calls, no query chrome.
    expect(api.listQueries).not.toHaveBeenCalled();
    expect(api.getCluster).not.toHaveBeenCalled();
    expect(
      screen.queryByText(/live queries on the coordinator/i),
    ).not.toBeInTheDocument();
  });

  test("StarRocks Monitor shows the phase-aware empty state when there are no pods", async () => {
    vi.mocked(api.getEngine).mockResolvedValue({
      ...ENGINE,
      type: "starrocks",
      spec: {
        coordinatorReplicas: 0,
        workerReplicas: 0,
        cpuMin: "",
        cpuMax: "",
        memory: "",
        imageVersion: "starrocks/fe-ubuntu:3.3-latest",
        starrocksConfig: { feReplicas: 1, cnReplicas: 1 },
      },
      status: { phase: "Provisioning", message: null, pods: [] },
    } as unknown as Awaited<ReturnType<typeof api.getEngine>>);

    const user = userEvent.setup();
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });
    await user.click(screen.getByRole("tab", { name: "Monitor" }));

    expect(
      await screen.findByText(/waiting for the operator to create pods/i),
    ).toBeInTheDocument();
    // Still the health view (coming-soon note), never the Trino monitor.
    expect(
      screen.getByText(/query-level monitoring for starrocks is coming soon/i),
    ).toBeInTheDocument();
    expect(api.listQueries).not.toHaveBeenCalled();
  });

  test("StarRocks Monitor omits the Autoscaling line when cnAutoscaling is the disabled sentinel", async () => {
    vi.mocked(api.getEngine).mockResolvedValue({
      ...ENGINE,
      type: "starrocks",
      spec: {
        coordinatorReplicas: 0,
        workerReplicas: 0,
        cpuMin: "",
        cpuMax: "",
        memory: "",
        imageVersion: "starrocks/fe-ubuntu:3.3-latest",
        // #266 disable sentinel: maxReplicas 0 → autoscaling OFF.
        starrocksConfig: { feReplicas: 1, cnReplicas: 1, cnAutoscaling: { maxReplicas: 0 } },
      },
      status: {
        phase: "running",
        message: null,
        pods: [{ name: "sr1-fe-0", phase: "Running", ready: true }],
      },
    } as unknown as Awaited<ReturnType<typeof api.getEngine>>);

    const user = userEvent.setup();
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });
    await user.click(screen.getByRole("tab", { name: "Monitor" }));

    expect(await screen.findByText("sr1-fe-0")).toBeInTheDocument();
    expect(screen.queryByText(/^autoscaling:/i)).not.toBeInTheDocument();
  });

  test("StarRocks Monitor degrades the FE/CN replica summary to em-dashes when starrocksConfig is absent", async () => {
    vi.mocked(api.getEngine).mockResolvedValue({
      ...ENGINE,
      type: "starrocks",
      spec: {
        coordinatorReplicas: 0,
        workerReplicas: 0,
        cpuMin: "",
        cpuMax: "",
        memory: "",
        imageVersion: "starrocks/fe-ubuntu:3.3-latest",
        // No starrocksConfig at all — the summary must not crash or print "undefined".
      },
      status: {
        phase: "running",
        message: null,
        pods: [{ name: "sr1-fe-0", phase: "Running", ready: true }],
      },
    } as unknown as Awaited<ReturnType<typeof api.getEngine>>);

    const user = userEvent.setup();
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });
    await user.click(screen.getByRole("tab", { name: "Monitor" }));

    expect(await screen.findByText("FE replicas")).toBeInTheDocument();
    expect(screen.getByText("CN replicas")).toBeInTheDocument();
    // Both replica values degrade to em-dashes, never the literal "undefined".
    expect(screen.getAllByText("—").length).toBeGreaterThanOrEqual(2);
    expect(screen.queryByText(/undefined/)).not.toBeInTheDocument();
  });

  test("Trino Monitor still renders the Trino query monitor (EngineMonitor)", async () => {
    mockGetEngine(ENGINE.spec);
    vi.mocked(api.getCluster).mockResolvedValue({
      running: 0,
      queued: 0,
      blocked: 0,
      nodes: [],
    } as unknown as Awaited<ReturnType<typeof api.getCluster>>);
    vi.mocked(api.listQueries).mockResolvedValue(
      [] as unknown as Awaited<ReturnType<typeof api.listQueries>>,
    );

    const user = userEvent.setup();
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });
    await user.click(screen.getByRole("tab", { name: "Monitor" }));

    // The Trino monitor chrome renders and it polls the Trino REST proxy.
    expect(
      await screen.findByText(/live queries on the coordinator/i),
    ).toBeInTheDocument();
    await waitFor(() => expect(api.listQueries).toHaveBeenCalled());
    expect(api.getCluster).toHaveBeenCalled();

    // The StarRocks health view is NOT rendered.
    expect(
      screen.queryByText(/query-level monitoring for starrocks is coming soon/i),
    ).not.toBeInTheDocument();
  });
});

describe("EngineDetail — Overview KPI strip", () => {
  // Two catalogs attached to THIS engine → the strip's catalog count resolves
  // to 2. The strip reads the registry list (SP3c-2 Task 3), filtered to this
  // engine's attachments, not a catalog count off the whole registry.
  const CATALOGS: RegistryCatalogDto[] = [
    registryCatalog({ id: "c1", name: "iceberg", kind: "iceberg", engines: [attachment({ isPrimary: true })] }),
    registryCatalog({ id: "c2", name: "pg", kind: "postgresql", engines: [attachment()] }),
  ];

  beforeEach(() => {
    vi.clearAllMocks();
    mockRegistryList(CATALOGS);
    vi.mocked(engineApi.useControlPlaneEnginesListEngineWorkspaces).mockReturnValue(
      query(ASSIGNED) as unknown as ReturnType<
        typeof engineApi.useControlPlaneEnginesListEngineWorkspaces
      >,
    );
  });

  test("StarRocks Overview shows phase, pods, FE/CN replicas, counts, and auth", async () => {
    vi.mocked(api.getEngine).mockResolvedValue({
      ...ENGINE,
      type: "starrocks",
      spec: {
        coordinatorReplicas: 0,
        workerReplicas: 0,
        cpuMin: "",
        cpuMax: "",
        memory: "",
        imageVersion: "starrocks/fe-ubuntu:3.3-latest",
        starrocksConfig: { feReplicas: 1, cnReplicas: 1 },
      },
      status: {
        phase: "running",
        message: null,
        pods: [{ name: "sr1-fe-0", phase: "Running", ready: true }],
      },
    } as unknown as Awaited<ReturnType<typeof api.getEngine>>);

    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });

    // Overview is the default tab — the KPI strip renders above the Status card.
    expect(screen.getByText("phase")).toBeInTheDocument();
    expect(screen.getByText("1/1")).toBeInTheDocument(); // ready/total pods
    expect(screen.getByText("FE 1 · CN 1")).toBeInTheDocument(); // StarRocks replicas
    expect(screen.getByText("catalogs")).toBeInTheDocument();
    expect(screen.getByText("workspaces")).toBeInTheDocument();
    // Catalog count resolves from the registry list, filtered to this engine → 2.
    expect(await screen.findByText("2")).toBeInTheDocument();
    // Auth item (StarRocks with no delegation → "No delegation").
    expect(screen.getByText("auth")).toBeInTheDocument();
    expect(screen.getAllByText("No delegation").length).toBeGreaterThan(0);
  });

  test("Trino Overview shows Coord/Worker replicas and JWT auth", async () => {
    mockGetEngine(ENGINE.spec); // Trino: coord 1 / worker 2, JWT enabled

    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });

    expect(screen.getByText("Coord 1 · Worker 2")).toBeInTheDocument();
    // engineAuthLabel → "JWT auth" (rendered in the strip and the header meta).
    expect(screen.getAllByText("JWT auth").length).toBeGreaterThan(0);
  });

  test("Trino Overview never renders 'undefined' when replica counts are absent", async () => {
    // A DTO that omits coordinatorReplicas/workerReplicas (the fields are only
    // "surfaced back" as a subset, so a real payload can lack them).
    vi.mocked(api.getEngine).mockResolvedValue({
      ...ENGINE,
      spec: {
        cpuMin: "1",
        cpuMax: "2",
        memory: "4Gi",
        imageVersion: "latest",
        trinoConfig: { coordinator: {}, worker: {} },
      },
      status: { phase: "running", message: null, pods: [] },
    } as unknown as Awaited<ReturnType<typeof api.getEngine>>);

    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });

    // The replicas KPI degrades to em-dashes, never the literal "undefined".
    expect(screen.getByText("Coord — · Worker —")).toBeInTheDocument();
    expect(screen.queryByText(/undefined/)).not.toBeInTheDocument();
  });
});

describe("EngineDetail — delete", () => {
  const BLOCKERS: EngineDependentDto[] = [
    {
      workspaceId: WS_A.id,
      workspaceName: WS_A.name,
      workspaceSlug: WS_A.slug,
      kind: "dataapi_endpoint",
      count: 25,
      sample: ["orders-by-day"],
    },
    {
      workspaceId: WS_A.id,
      workspaceName: WS_A.name,
      workspaceSlug: WS_A.slug,
      kind: "dataapi_endpoint_trashed",
      count: 1,
      sample: ["old-orders"],
    },
  ];

  beforeEach(() => {
    vi.clearAllMocks();
    mockGetEngine(ENGINE.spec);
    mockRegistryList([]);
    vi.mocked(engineApi.useControlPlaneEnginesListEngineWorkspaces).mockReturnValue(
      query(ASSIGNED) as unknown as ReturnType<
        typeof engineApi.useControlPlaneEnginesListEngineWorkspaces
      >,
    );
  });

  async function openDeleteDialog() {
    const user = userEvent.setup();
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });
    await user.click(screen.getByRole("button", { name: /more engine actions/i }));
    await user.click(await screen.findByRole("menuitem", { name: /delete engine/i }));
    return { user, dialog: await screen.findByRole("alertdialog") };
  }

  test("states what is destroyed and requires the engine name", async () => {
    const { user, dialog } = await openDeleteDialog();
    expect(within(dialog).getByText(/kubernetes cluster/i)).toBeInTheDocument();
    expect(within(dialog).getByText(/query history and notebook engine access/i)).toBeInTheDocument();
    expect(within(dialog).getByText(/saved queries that use it lose their connection/i)).toBeInTheDocument();

    const confirm = within(dialog).getByRole("button", { name: "Delete" });
    const input = within(dialog).getByPlaceholderText("analytics-trino");
    await user.type(input, "delete");
    expect(confirm).toBeDisabled();
    await user.clear(input);
    await user.type(input, "analytics-trino");
    expect(confirm).toBeEnabled();
  });

  test("a 409 lists the dependents and offers no delete", async () => {
    vi.mocked(api.deleteEngine).mockRejectedValue(
      new ApiError(409, "engine is still in use", "engine_has_dependents", {
        error: "engine is still in use",
        code: "engine_has_dependents",
        dependents: BLOCKERS,
      }),
    );
    const { user, dialog } = await openDeleteDialog();
    await user.type(within(dialog).getByPlaceholderText("analytics-trino"), "analytics-trino");
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));

    const blocked = await screen.findByRole("alertdialog", { name: /still in use/i });
    expect(within(blocked).getByText("Data API endpoints")).toBeInTheDocument();
    expect(within(blocked).getByText("Data API endpoints in trash")).toBeInTheDocument();
    expect(within(blocked).getByText("25")).toBeInTheDocument();
    expect(within(blocked).queryByRole("button", { name: /delete/i })).not.toBeInTheDocument();
    expect(vi.mocked(api.deleteEngine).mock.calls[0][0]).toBe("e1");
  });

  test("a 409 also refetches the engine, so a row left `deleting` by the race shows the retry state", async () => {
    vi.mocked(api.deleteEngine).mockRejectedValue(
      new ApiError(409, "engine is still in use", "engine_has_dependents", {
        error: "engine is still in use",
        code: "engine_has_dependents",
        dependents: BLOCKERS,
      }),
    );
    const { user, dialog } = await openDeleteDialog();
    const callsBeforeDelete = vi.mocked(api.getEngine).mock.calls.length;
    await user.type(within(dialog).getByPlaceholderText("analytics-trino"), "analytics-trino");
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    await screen.findByRole("alertdialog", { name: /still in use/i });

    // The 409 leaves the engine `deleting` when it arrives after the cluster
    // was already torn down (route step 5) — the header must refetch on ANY
    // delete failure, dependents included, not just a non-dependents one.
    await waitFor(() =>
      expect(vi.mocked(api.getEngine).mock.calls.length).toBeGreaterThan(callsBeforeDelete),
    );
  });

  test("a deleting engine shows the retry state and Retry delete deletes again", async () => {
    vi.mocked(api.getEngine).mockResolvedValue({
      ...ENGINE,
      status: {
        phase: "deleting",
        message: "a previous delete did not finish; retry the delete",
        pods: [],
      },
    } as unknown as Awaited<ReturnType<typeof api.getEngine>>);
    vi.mocked(api.deleteEngine).mockResolvedValue(undefined);
    const user = userEvent.setup();
    renderDetail();
    expect(await screen.findByText("Deleting — retry")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /retry delete/i }));
    await waitFor(() => expect(vi.mocked(api.deleteEngine).mock.calls[0]?.[0]).toBe("e1"));
  });
});

// ── Catalogs tab ─────────────────────────────────────────────────────────────
//
// SP3c-1 Task 9. The inline `CatalogForm` master/detail is gone: this tab is
// now the mirror of Control Plane → Catalogs → detail → Engines, over the same
// registry routes, and it is the ONE screen that renders
// `RegistryCatalogDto.configError` for a stored row the tightened validator
// now refuses. SP3c-2 Task 3 moved the field onto the registry list (`GET
// /catalogs`) this tab already reads — it used to live only on the legacy
// `GET /engines/{id}/catalogs`, which this tab read purely to get it, and
// that second query is gone along with the field's old home.

const ADMIN: Me = {
  user_id: "u1",
  email: "admin@example.com",
  platform_roles: ["platform_admin"],
  permissions: [],
};
const READER: Me = {
  user_id: "u2",
  email: "reader@example.com",
  platform_roles: [],
  permissions: ["engine:read", "catalog_registry:read"],
};

function attachment(over: Partial<CatalogAttachmentDto> = {}): CatalogAttachmentDto {
  return {
    engineId: ENGINE.id,
    engineName: ENGINE.name,
    engineType: "trino",
    applyStatus: "applied",
    applyError: null,
    appliedAt: "2026-09-01T00:00:00Z",
    isPrimary: false,
    createdAt: "2026-09-01T00:00:00Z",
    ...over,
  };
}

function registryCatalog(over: Partial<RegistryCatalogDto> = {}): RegistryCatalogDto {
  return {
    id: "c-1",
    name: "sales",
    kind: "jdbc",
    config: {},
    advanced: {},
    description: null,
    warehouseId: null,
    engines: [attachment()],
    workspaceIds: [],
    createdAt: "2026-08-01T10:00:00Z",
    updatedAt: "2026-09-01T10:00:00Z",
    ...over,
  };
}

/** `sales` (applied) and `events` (failed) on this engine; `remote` on another. */
const SALES = registryCatalog();
const EVENTS = registryCatalog({
  id: "c-2",
  name: "events",
  kind: "iceberg",
  engines: [
    attachment({ applyStatus: "failed", applyError: "FE refused: catalog already exists" }),
  ],
});
/** Attached to a DIFFERENT engine — never a row here, always a candidate. */
const REMOTE = registryCatalog({
  id: "c-3",
  name: "remote",
  kind: "hive",
  engines: [attachment({ engineId: "e2", engineName: "sr-eu", engineType: "starrocks" })],
});
/** Attached to nothing at all. */
const SPARE = registryCatalog({ id: "c-4", name: "spare", kind: "tpch", engines: [] });

/** A fixture with its own name and reason: a fixture that contained the
 * literal string its own assertion searches for would be satisfied by the
 * SERVER half whatever the client renders (SP3c Task 7 review F2). */
const CONFIG_ERROR_MSG = "missing field `warehouse` at line 1 column 34";

/** `row`, with `configError` set — the registry's own way of marking a
 * stored configuration the tightened validator now refuses (SP3c-2 Task 3). */
function withConfigError(
  row: RegistryCatalogDto,
  message = CONFIG_ERROR_MSG,
): RegistryCatalogDto {
  return { ...row, configError: message };
}

let registryAttachFn: ReturnType<typeof vi.fn>;
let registryDetachFn: ReturnType<typeof vi.fn>;
let registryRetryFn: ReturnType<typeof vi.fn>;

type MutationHook = (options?: { mutation?: Record<string, unknown> }) => unknown;

/**
 * Wire one generated mutation hook to a REAL `useMutation` over `fn`, keeping
 * the caller's own `mutation` options — that is where `onSuccess`/`onError`
 * live, and a hook mocked to a bare object would swallow both.
 */
function wire(hook: unknown, fn: ReturnType<typeof vi.fn>) {
  vi.mocked(hook as MutationHook).mockImplementation((options) =>
    useMutation({ mutationFn: fn as never, ...options?.mutation }),
  );
}

function mockRegistryList(rows: RegistryCatalogDto[], over: Record<string, unknown> = {}) {
  vi.mocked(registryApi.useControlPlaneCatalogRegistryListRegistryCatalogs).mockReturnValue({
    data: { status: 200, data: rows },
    isLoading: false,
    isError: false,
    error: null,
    ...over,
  } as unknown as ReturnType<
    typeof registryApi.useControlPlaneCatalogRegistryListRegistryCatalogs
  >);
}

/**
 * Wire the registry list hook to a REAL `useQuery` over `queryFn`, keyed
 * exactly like the generated hook (`catalogsQueryKey`) — unlike
 * `mockRegistryList`, this lets `useAttachmentActions`' post-mutation
 * `invalidateQueries({ queryKey: catalogsQueryKey })` trigger a REAL refetch
 * through react-query's own cache mechanics, so a test can reach "a
 * successful load, then a failing refetch" the way the page does, rather
 * than hand-constructing that combined state as a single mocked return value
 * (fix round 1, Important).
 */
function wireRegistryQuery(queryFn: () => Promise<{ status: 200; data: RegistryCatalogDto[] }>) {
  vi.mocked(registryApi.useControlPlaneCatalogRegistryListRegistryCatalogs).mockImplementation(
    ((_params?: unknown, options?: { query?: Record<string, unknown> }) =>
      useQuery({
        queryKey: catalogsQueryKey,
        queryFn,
        retry: false,
        // The production `queryClient` (`api/queryClient.ts`) sets
        // `staleTime: 30000`; this test's own `QueryClient` (`renderDetail`)
        // does not, so without this a SECOND observer mounting on the same
        // key (`EngineSummaryStrip` and `CatalogsCard` both call this hook)
        // would itself trigger a background refetch-on-mount — consuming the
        // "second call fails" this test means to reserve for the Detach's
        // invalidation.
        staleTime: 30000,
        ...(options?.query ?? {}),
      })) as typeof registryApi.useControlPlaneCatalogRegistryListRegistryCatalogs,
  );
}

/** Open the Catalogs tab and hand back the user-event session that did it. */
async function openCatalogs(me: Me | null = ADMIN, entry?: string) {
  const user = userEvent.setup({ pointerEventsCheck: 0 });
  renderDetail(entry ?? `/control-plane/engines/${ENGINE.id}`, me);
  await screen.findByRole("heading", { name: ENGINE.name });
  await user.click(screen.getByRole("tab", { name: "Catalogs" }));
  return user;
}

/**
 * The table row whose **Catalog cell** names `name` — anchored at the start of
 * the row's accessible name, because a blocked-reason sentence in one row can
 * name the catalog that blocked it and an unanchored match then finds two rows.
 */
function catalogRow(name: string) {
  return screen.getByRole("row", { name: new RegExp(`^${name}`) });
}

describe("EngineDetail — Catalogs tab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockGetEngine(ENGINE.spec);
    vi.mocked(engineApi.useControlPlaneEnginesListEngineWorkspaces).mockReturnValue(
      query(ASSIGNED) as unknown as ReturnType<
        typeof engineApi.useControlPlaneEnginesListEngineWorkspaces
      >,
    );
    mockRegistryList([SALES, EVENTS, REMOTE, SPARE]);

    registryAttachFn = vi
      .fn()
      .mockResolvedValue({ status: 201, data: attachment({ applyStatus: "applied" }) });
    registryDetachFn = vi.fn().mockResolvedValue({ status: 204, data: undefined });
    registryRetryFn = vi.fn().mockResolvedValue({
      status: 200,
      data: { engineId: ENGINE.id, engineName: ENGINE.name, status: "applied", error: null },
    });
    wire(registryApi.useControlPlaneCatalogRegistryRegistryAttachCatalog, registryAttachFn);
    wire(registryApi.useControlPlaneCatalogRegistryDetachCatalog, registryDetachFn);
    wire(registryApi.useControlPlaneCatalogRegistryRetryCatalogApply, registryRetryFn);
    testCatalogFn = vi.fn();
    wire(engineApi.useControlPlaneEnginesTestCatalog, testCatalogFn);
  });

  test("lists this engine's attachments with their apply status, and omits catalogs attached elsewhere", async () => {
    await openCatalogs();

    expect(await screen.findByRole("table")).toBeInTheDocument();
    expect(within(catalogRow("sales")).getByText("applied")).toBeInTheDocument();
    expect(within(catalogRow("events")).getByText("failed")).toBeInTheDocument();
    // The two badges must not be one badge with two captions.
    expect(within(catalogRow("sales")).getByText("applied").className).not.toBe(
      within(catalogRow("events")).getByText("failed").className,
    );
    // `remote` is attached to sr-eu, not to this engine.
    expect(screen.queryByRole("row", { name: /remote/ })).not.toBeInTheDocument();
    // `applyError` in full, as text — a `title` is not something an operator
    // can copy or capture in a screenshot.
    expect(screen.getByText(/FE refused: catalog already exists/)).toBeInTheDocument();
  });

  test("the Apply column is not called Status, and the table says what it is not", async () => {
    await openCatalogs();

    const table = await screen.findByRole("table");
    const heads = within(table).getAllByRole("columnheader").map((h) => h.textContent);
    expect(heads).toContain("Apply");
    expect(heads).not.toContain("Status");
    // `applyStatus` answers "did this catalog reach this engine", and keeps
    // answering `applied` after the engine breaks.
    expect(screen.getByTestId("engine-apply-footnote")).toHaveTextContent(
      /not a check that it still works/,
    );
  });

  test("the picker offers only catalogs this engine does not already have", async () => {
    const user = await openCatalogs();

    await user.click(await screen.findByRole("combobox", { name: /Catalog to attach/ }));
    const options = (await screen.findAllByRole("option")).map((o) => o.textContent);
    expect(options.some((o) => o?.includes("remote"))).toBe(true);
    expect(options.some((o) => o?.includes("spare"))).toBe(true);
    expect(options.some((o) => o?.includes("sales"))).toBe(false);
    expect(options.some((o) => o?.includes("events"))).toBe(false);
  });

  test("attach calls the REGISTRY route with { id: catalogId, engineId } — not the legacy engine-mounted one", async () => {
    const user = await openCatalogs();

    await user.click(await screen.findByRole("combobox", { name: /Catalog to attach/ }));
    await user.click(await screen.findByRole("option", { name: /spare/ }));
    await user.click(screen.getByRole("button", { name: /^Attach$/ }));

    await waitFor(() => expect(registryAttachFn).toHaveBeenCalledTimes(1));
    expect(registryAttachFn.mock.calls[0][0]).toEqual({ id: "c-4", engineId: ENGINE.id });
    await waitFor(() => expect(invalidated).toContainEqual(catalogKey("c-4")));
  });

  test("Detach calls the registry route with { id: catalogId, engineId }", async () => {
    const user = await openCatalogs();

    await user.click(within(catalogRow("sales")).getByRole("button", { name: /Detach/ }));
    const dialog = await screen.findByRole("alertdialog");
    await user.type(await within(dialog).findByPlaceholderText("sales"), "sales");
    await user.click(within(dialog).getByRole("button", { name: /^Detach$/ }));

    await waitFor(() => expect(registryDetachFn).toHaveBeenCalledTimes(1));
    expect(registryDetachFn.mock.calls[0][0]).toEqual({ id: "c-1", engineId: ENGINE.id });
  });

  test("Retry calls the per-attachment route with { id: engineId, cid: catalogId }", async () => {
    const user = await openCatalogs();

    await user.click(within(catalogRow("events")).getByRole("button", { name: /Retry/ }));

    await waitFor(() => expect(registryRetryFn).toHaveBeenCalledTimes(1));
    expect(registryRetryFn.mock.calls[0][0]).toEqual({ id: ENGINE.id, cid: "c-2" });
  });

  test("a 400 from a primary-catalog detach surfaces the server's own sentence, not a generic one", async () => {
    registryDetachFn.mockRejectedValue(
      new ApiError(400, "sales leads analytics-trino; delete the engine instead"),
    );
    const user = await openCatalogs();

    await user.click(within(catalogRow("sales")).getByRole("button", { name: /Detach/ }));
    const dialog = await screen.findByRole("alertdialog");
    await user.type(await within(dialog).findByPlaceholderText("sales"), "sales");
    await user.click(within(dialog).getByRole("button", { name: /^Detach$/ }));

    await waitFor(() => expect(toast.error).toHaveBeenCalled());
    expect(vi.mocked(toast.error).mock.calls[0][0]).toBe(
      "sales leads analytics-trino; delete the engine instead",
    );
  });

  test("a primary attachment cannot be detached here and says where it can", async () => {
    mockRegistryList([
      registryCatalog({ engines: [attachment({ isPrimary: true })] }),
      EVENTS,
    ]);
    await openCatalogs();

    expect(within(catalogRow("sales")).getByRole("button", { name: /Detach/ })).toBeDisabled();
    expect(within(catalogRow("sales")).getByText(/primary catalog/)).toBeInTheDocument();
    // …and a non-primary row in the same table still can, so the assertion
    // above is about `isPrimary` and not about the table being inert.
    expect(within(catalogRow("events")).getByRole("button", { name: /Detach/ })).toBeEnabled();
  });

  test("an engine with no catalogs says so instead of showing an empty table", async () => {
    mockRegistryList([REMOTE, SPARE]);
    await openCatalogs();

    expect(await screen.findByText(/no catalogs, so it can query nothing/)).toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });

  test("the header says what attaching does to THIS engine kind — Trino rolls the coordinator", async () => {
    await openCatalogs();

    expect(await screen.findByTestId("engine-catalogs-apply-prose")).toHaveTextContent(
      /restarts the Trino coordinator/,
    );
  });

  test("a StarRocks engine is not promised a pod roll", async () => {
    vi.mocked(api.getEngine).mockResolvedValue({
      ...ENGINE,
      type: "starrocks",
      status: { phase: "running", message: null, pods: [] },
    } as unknown as Awaited<ReturnType<typeof api.getEngine>>);
    await openCatalogs();

    const prose = await screen.findByTestId("engine-catalogs-apply-prose");
    expect(prose).toHaveTextContent(/on the StarRocks FE in place/);
    expect(prose).not.toHaveTextContent(/restart/i);
  });
});

describe("EngineDetail — Catalogs tab: a stored configuration that no longer parses", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockGetEngine(ENGINE.spec);
    vi.mocked(engineApi.useControlPlaneEnginesListEngineWorkspaces).mockReturnValue(
      query(ASSIGNED) as unknown as ReturnType<
        typeof engineApi.useControlPlaneEnginesListEngineWorkspaces
      >,
    );
    mockRegistryList([SALES, EVENTS, REMOTE, SPARE]);
    registryRetryFn = vi.fn().mockResolvedValue({ status: 200, data: {} });
    registryDetachFn = vi.fn().mockResolvedValue({ status: 204, data: undefined });
    registryAttachFn = vi.fn().mockResolvedValue({ status: 201, data: attachment() });
    wire(registryApi.useControlPlaneCatalogRegistryRegistryAttachCatalog, registryAttachFn);
    wire(registryApi.useControlPlaneCatalogRegistryDetachCatalog, registryDetachFn);
    wire(registryApi.useControlPlaneCatalogRegistryRetryCatalogApply, registryRetryFn);
    testCatalogFn = vi.fn();
    wire(engineApi.useControlPlaneEnginesTestCatalog, testCatalogFn);
  });

  test("the broken row is marked, its reason is on screen, and it links to where it is repaired", async () => {
    mockRegistryList([SALES, withConfigError(EVENTS), REMOTE, SPARE]);
    await openCatalogs();

    // Marked in the row, so scanning the table finds it…
    expect(within(await screen.findByRole("row", { name: /^events/ })).getByText(
      /config invalid/i,
    )).toBeInTheDocument();
    // …and the reason is quoted in full, verbatim, below the table.
    const box = screen.getByTestId("engine-catalog-config-errors");
    expect(box).toHaveTextContent(CONFIG_ERROR_MSG);
    // The repair path: the catalog's own page, named and linked.
    expect(within(box).getByRole("link", { name: /events/ })).toHaveAttribute(
      "href",
      "/control-plane/catalogs/c-2",
    );
  });

  test("Retry is offered but cannot work on a broken row, and says why", async () => {
    mockRegistryList([SALES, withConfigError(EVENTS), REMOTE, SPARE]);
    await openCatalogs();

    const row = await screen.findByRole("row", { name: /^events/ });
    expect(within(row).getByRole("button", { name: /Retry/ })).toBeDisabled();
    expect(within(row).getByText(/cannot render it for this engine/)).toBeInTheDocument();
    // The ROW's reason is about this row's own stored configuration. A healthy
    // row on the same Trino engine is blocked too, but for the other reason —
    // if the two collapsed into one sentence the screen would be telling an
    // operator that `sales` is the file that will not parse.
    const other = catalogRow("sales");
    expect(within(other).queryByText(/cannot parse this catalog's stored configuration/)).toBeNull();
    expect(within(other).getByText(/rebuilds every remaining catalog/)).toBeInTheDocument();
  });

  test("a row that parses is never marked broken", async () => {
    await openCatalogs();

    await screen.findByRole("table");
    expect(screen.queryByText(/config invalid/i)).not.toBeInTheDocument();
    expect(screen.queryByTestId("engine-catalog-config-errors")).not.toBeInTheDocument();
  });
});

describe("EngineDetail — Catalogs tab: permissions", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockGetEngine(ENGINE.spec);
    vi.mocked(engineApi.useControlPlaneEnginesListEngineWorkspaces).mockReturnValue(
      query(ASSIGNED) as unknown as ReturnType<
        typeof engineApi.useControlPlaneEnginesListEngineWorkspaces
      >,
    );
    mockRegistryList([SALES, EVENTS, REMOTE, SPARE]);
    registryAttachFn = vi.fn();
    registryDetachFn = vi.fn();
    registryRetryFn = vi.fn();
    wire(registryApi.useControlPlaneCatalogRegistryRegistryAttachCatalog, registryAttachFn);
    wire(registryApi.useControlPlaneCatalogRegistryDetachCatalog, registryDetachFn);
    wire(registryApi.useControlPlaneCatalogRegistryRetryCatalogApply, registryRetryFn);
    testCatalogFn = vi.fn();
    wire(engineApi.useControlPlaneEnginesTestCatalog, testCatalogFn);
  });

  test("an operator who may only read gets the reason and no control that could not work", async () => {
    await openCatalogs(READER);

    await screen.findByRole("table");
    expect(screen.queryByRole("button", { name: /Detach/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Attach$/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: /Catalog to attach/ })).not.toBeInTheDocument();
    const note = screen.getByTestId("engine-catalogs-readonly");
    expect(note).toHaveTextContent("catalog_registry:update");
    expect(note).toHaveTextContent(/administrator/i);
  });

  test("without catalog_registry:read nothing renders, and the missing permission is named", async () => {
    // SP3c-2 Task 3: the registry list is now the ONLY read, so a 403 on it
    // means egret cannot show even the names — the legacy engine-mounted
    // fallback that used to keep them visible is gone with the second query.
    mockRegistryList([], {
      data: undefined,
      isError: true,
      error: new ApiError(403, "forbidden"),
    });
    await openCatalogs();

    const note = await screen.findByTestId("engine-catalogs-unreadable");
    expect(note).toHaveTextContent("catalog_registry:read");
    // Not the bare word `Error::Forbidden` renders: who can grant it, too.
    expect(note).toHaveTextContent(/administrator/i);
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox", { name: /Catalog to attach/ })).not.toBeInTheDocument();
  });

  test("a non-403 registry failure shows the server's own sentence, not a permission story", async () => {
    mockRegistryList([], {
      data: undefined,
      isError: true,
      error: new ApiError(500, "registry store unavailable"),
    });
    await openCatalogs();

    const note = await screen.findByTestId("engine-catalogs-unreadable");
    expect(note).toHaveTextContent("registry store unavailable");
    expect(note).not.toHaveTextContent("catalog_registry:read");
  });
});

// ── Catalogs tab, fix round 1 ────────────────────────────────────────────────
//
// F-1 an error must never render as a fact about the system; F-2 the blocked
// scope is the BACKEND's, which is engine-wide on Trino and row-wide on
// StarRocks; F-3 the per-engine Test restored, because it is the only route
// that tests a NAMED engine's view of a catalog and the only one the
// `engine:read`-only persona can reach.

/** `engine:read` and nothing else — the persona the union exists for. */
const ENGINE_ONLY: Me = {
  user_id: "u3",
  email: "ops@example.com",
  platform_roles: [],
  permissions: ["engine:read"],
};

let testCatalogFn: ReturnType<typeof vi.fn>;

/** `sales` is failed-but-renderable (so Retry shows); `events` is broken. */
const SALES_FAILED = registryCatalog({
  engines: [attachment({ applyStatus: "failed", applyError: "coordinator was rolling" })],
});
const EVENTS_OK = registryCatalog({
  id: "c-2",
  name: "events",
  kind: "iceberg",
  engines: [attachment()],
});

/** `rows`, with the `events` catalog (`id: "c-2"`) marked broken and mocked as
 * the registry list — `configError` is catalog-scoped now (SP3c-2 Task 3), so
 * this is the one place a test needs to touch to make `events` unparseable. */
function mockBrokenEvents(rows: RegistryCatalogDto[]) {
  mockRegistryList(rows.map((r) => (r.id === "c-2" ? withConfigError(r) : r)));
}

function catalogsTabBeforeEach() {
  vi.clearAllMocks();
  mockGetEngine(ENGINE.spec);
  vi.mocked(engineApi.useControlPlaneEnginesListEngineWorkspaces).mockReturnValue(
    query(ASSIGNED) as unknown as ReturnType<
      typeof engineApi.useControlPlaneEnginesListEngineWorkspaces
    >,
  );
  mockRegistryList([SALES, EVENTS, REMOTE, SPARE]);
  registryAttachFn = vi.fn().mockResolvedValue({ status: 201, data: attachment() });
  registryDetachFn = vi.fn().mockResolvedValue({ status: 204, data: undefined });
  registryRetryFn = vi.fn().mockResolvedValue({ status: 200, data: {} });
  testCatalogFn = vi.fn();
  wire(registryApi.useControlPlaneCatalogRegistryRegistryAttachCatalog, registryAttachFn);
  wire(registryApi.useControlPlaneCatalogRegistryDetachCatalog, registryDetachFn);
  wire(registryApi.useControlPlaneCatalogRegistryRetryCatalogApply, registryRetryFn);
  wire(engineApi.useControlPlaneEnginesTestCatalog, testCatalogFn);
}

describe("EngineDetail — Catalogs tab: an unreadable list is not an empty one", () => {
  beforeEach(catalogsTabBeforeEach);

  test("when the read fails the tab does not claim the engine has no catalogs", async () => {
    mockRegistryList([], { data: undefined, isError: true, error: new ApiError(500, "store unavailable") });
    await openCatalogs();

    const box = await screen.findByTestId("engine-catalogs-unreadable");
    expect(screen.queryByText(/no catalogs, so it can query nothing/)).not.toBeInTheDocument();
    // What it does say: not that there are none, only that it could not look.
    expect(box).toHaveTextContent(/could not read/i);
    expect(box).toHaveTextContent(/not evidence/i);
    expect(box).toHaveTextContent("store unavailable");
  });

  test("an engine that egret DID read and found empty still says so", async () => {
    mockRegistryList([REMOTE, SPARE]);
    await openCatalogs();

    expect(await screen.findByText(/no catalogs, so it can query nothing/)).toBeInTheDocument();
    expect(screen.queryByTestId("engine-catalogs-unreadable")).not.toBeInTheDocument();
  });
});

/**
 * Fix round 1, Important. `data !== undefined && isError` — react-query
 * keeping the last successful list through a failed refetch — was reachable
 * (the shared query has `retry: 1` / `staleTime: 30000`, and every
 * attach/detach/retry invalidates it) and rendered as a plain, current-looking
 * table: `rows.length === 0` was false, so the empty-state alert never
 * mounted, `registryRead` was false so nothing else said the read had failed,
 * and the attach picker quietly disappeared. That is "silence reads as an
 * all-clear" — worse than showing nothing, because a stale table LOOKS like a
 * fact about the engine.
 *
 * Driven the way the page actually reaches it: a real `useQuery` loads once
 * successfully, then a Detach succeeds and its `onSuccess` invalidates the
 * SAME query key, triggering a real refetch that fails — not a single mocked
 * return value asserting both `data` and `isError` at once.
 */
describe("EngineDetail — Catalogs tab: a refetch that fails after a good load", () => {
  beforeEach(catalogsTabBeforeEach);

  test("keeps the stale rows on screen, says they may be out of date, and blocks Retry/Detach with a reason", async () => {
    let calls = 0;
    wireRegistryQuery(async () => {
      calls += 1;
      if (calls === 1) return { status: 200, data: [SALES, EVENTS] };
      throw new ApiError(500, "registry store unavailable");
    });
    registryDetachFn.mockResolvedValue({ status: 204, data: undefined });

    const user = await openCatalogs();

    // First load succeeded: the ordinary table, no stale banner yet.
    await screen.findByRole("row", { name: /^sales/ });
    expect(screen.queryByTestId("engine-catalogs-stale")).not.toBeInTheDocument();

    // Detach `events` — it succeeds, and `useAttachmentActions` invalidates
    // `catalogsQueryKey`, which is THIS query: a real refetch fires and its
    // second call (mocked above) fails.
    await user.click(within(catalogRow("events")).getByRole("button", { name: /Detach/ }));
    const dialog = await screen.findByRole("alertdialog");
    await user.type(await within(dialog).findByPlaceholderText("events"), "events");
    await user.click(within(dialog).getByRole("button", { name: /^Detach$/ }));
    await waitFor(() => expect(registryDetachFn).toHaveBeenCalledTimes(1));

    // The failed refetch does not blank the table: `sales` is still the
    // STALE row from the first successful read (react-query never clears
    // `data` on a failed refetch).
    const note = await screen.findByTestId("engine-catalogs-stale");
    expect(within(await screen.findByRole("row", { name: /^sales/ })).getByText("sales")).toBeInTheDocument();
    expect(note).toHaveTextContent(/out of date/i);
    expect(note).toHaveTextContent("registry store unavailable");
    expect(note).toHaveTextContent(/apply status/i);
    expect(note).toHaveTextContent(/Retry/);
    expect(note).toHaveTextContent(/Detach/);
    expect(note).toHaveTextContent(/attach picker/i);
    // Not an empty-table claim — that alert is for when there is nothing to
    // show, which is not this state.
    expect(screen.queryByTestId("engine-catalogs-unreadable")).not.toBeInTheDocument();

    // Retry/Detach on the stale row are disabled with a reason, not silently
    // still-clickable against data that might not be current any more.
    const sales = catalogRow("sales");
    expect(within(sales).getByRole("button", { name: /Detach/ })).toBeDisabled();
    expect(within(sales).getByText(/may be showing stale information/i)).toBeInTheDocument();
    // …and the attach picker is the control that used to vanish silently —
    // it is still gone, but now the banner above says why.
    expect(screen.queryByRole("combobox", { name: /Catalog to attach/ })).not.toBeInTheDocument();
  });
});

describe("EngineDetail — Catalogs tab: the blocked scope is the backend's", () => {
  beforeEach(catalogsTabBeforeEach);

  test("on Trino one broken catalog blocks Retry AND Detach on every other row, and names it", async () => {
    mockBrokenEvents([SALES_FAILED, EVENTS_OK]);
    await openCatalogs();

    const sales = await screen.findByRole("row", { name: /^sales/ });
    expect(within(sales).getByRole("button", { name: /Retry/ })).toBeDisabled();
    expect(within(sales).getByRole("button", { name: /Detach/ })).toBeDisabled();
    // TWO reasons, not one: a Retry rebuilds every catalog, a Detach rebuilds
    // every REMAINING one. They are different sentences about different
    // backend paths and each must name the catalog that caused it.
    expect(within(sales).getByText(/apply rebuilds every catalog on this engine/)).toBeInTheDocument();
    expect(within(sales).getByText(/detach rebuilds every remaining catalog/)).toBeInTheDocument();
    expect(within(sales).getAllByText(/events/)).toHaveLength(2);
  });

  test("on Trino the broken catalog itself can still be detached — it is left out of the rebuild", async () => {
    mockBrokenEvents([SALES_FAILED, EVENTS_OK]);
    await openCatalogs();

    const events = await screen.findByRole("row", { name: /^events/ });
    expect(within(events).getByRole("button", { name: /Detach/ })).toBeEnabled();
  });

  test("on StarRocks a broken catalog blocks only its own Retry", async () => {
    vi.mocked(api.getEngine).mockResolvedValue({
      ...ENGINE,
      type: "starrocks",
      status: { phase: "running", message: null, pods: [] },
    } as unknown as Awaited<ReturnType<typeof api.getEngine>>);
    mockBrokenEvents([SALES_FAILED, EVENTS_OK]);
    await openCatalogs();

    const sales = await screen.findByRole("row", { name: /^sales/ });
    expect(within(sales).getByRole("button", { name: /Retry/ })).toBeEnabled();
    expect(within(sales).getByRole("button", { name: /Detach/ })).toBeEnabled();
    // …and a detach of the broken row is a DROP that needs no render.
    const events = screen.getByRole("row", { name: /^events/ });
    expect(within(events).getByRole("button", { name: /Detach/ })).toBeEnabled();
  });

  test("the blocked-scope sentence is Trino's on Trino", async () => {
    mockBrokenEvents([SALES_FAILED, EVENTS_OK]);
    await openCatalogs();

    const box = await screen.findByTestId("engine-catalog-config-errors");
    expect(box).toHaveTextContent(/every catalog/i);
    expect(box).toHaveTextContent(/Settings tab/);
  });

  test("the blocked-scope sentence does not warn a StarRocks operator about a Settings save", async () => {
    vi.mocked(api.getEngine).mockResolvedValue({
      ...ENGINE,
      type: "starrocks",
      status: { phase: "running", message: null, pods: [] },
    } as unknown as Awaited<ReturnType<typeof api.getEngine>>);
    mockBrokenEvents([SALES_FAILED, EVENTS_OK]);
    await openCatalogs();

    const box = await screen.findByTestId("engine-catalog-config-errors");
    // `update_starrocks_engine` never renders attachments, so this save is not
    // in danger and the screen must not say it is.
    expect(box).not.toHaveTextContent(/Settings tab, is refused/);
    expect(box).toHaveTextContent(/unaffected/i);
  });
});

describe("EngineDetail — Catalogs tab: Test runs against THIS engine", () => {
  beforeEach(catalogsTabBeforeEach);

  async function runTest(row: string, result: { ok: boolean; message: string } | ApiError) {
    if (result instanceof ApiError) testCatalogFn.mockRejectedValue(result);
    else testCatalogFn.mockResolvedValue({ status: 200, data: result });
    const user = await openCatalogs();
    await user.click(
      within(await screen.findByRole("row", { name: new RegExp(`^${row}`) })).getByRole("button", {
        name: /^Test$/,
      }),
    );
    return { user, box: await screen.findByTestId("engine-catalog-test-result") };
  }

  test("a successful test names the statement and the engine it ran on", async () => {
    // A message that names neither the statement nor the engine, so only the
    // CLIENT's own line can satisfy the assertions below.
    const { box } = await runTest("sales", { ok: true, message: "2 rows." });

    expect(testCatalogFn.mock.calls[0][0]).toEqual({ id: ENGINE.id, cid: "c-1" });
    expect(box).toHaveAttribute("data-tone", "ok");
    expect(box).toHaveTextContent("SHOW SCHEMAS FROM sales");
    expect(box).toHaveTextContent(ENGINE.name);
    expect(box).toHaveTextContent("2 rows.");
  });

  test("a failed test does not claim the catalog's credentials were exercised", async () => {
    const { box } = await runTest("sales", {
      ok: false,
      message: "line 1:1: Catalog does not exist",
    });

    expect(box).toHaveAttribute("data-tone", "bad");
    expect(box).toHaveTextContent("line 1:1: Catalog does not exist");
    expect(box).toHaveTextContent(/did not succeed/);
    expect(box).not.toHaveTextContent(/which exercises the catalog's credentials/);
  });

  test("a coordinator egret could not reach is neutral, not a verdict about the catalog", async () => {
    const { box } = await runTest("sales", new ApiError(502, "bad gateway"));

    // Red here would say "this catalog is broken", which is not what a 502
    // measured — it measured nothing.
    expect(box).toHaveAttribute("data-tone", "neutral");
    expect(box).toHaveTextContent(/nothing was measured/i);
    expect(box).toHaveTextContent("bad gateway");
  });

  test("a catalog whose stored configuration does not parse can still be tested", async () => {
    mockBrokenEvents([SALES, EVENTS, REMOTE, SPARE]);
    testCatalogFn.mockResolvedValue({ status: 200, data: { ok: true, message: "fine" } });
    const user = await openCatalogs();

    const events = await screen.findByRole("row", { name: /^events/ });
    // The route reads only the catalog's NAME, so a broken row is testable —
    // and the answer ("Trino still serves it") is worth having.
    expect(within(events).getByRole("button", { name: /^Test$/ })).toBeEnabled();
    await user.click(within(events).getByRole("button", { name: /^Test$/ }));
    await waitFor(() => expect(testCatalogFn.mock.calls[0][0]).toEqual({ id: ENGINE.id, cid: "c-2" }));
  });

  test("an operator who may only read engines can still test", async () => {
    testCatalogFn.mockResolvedValue({ status: 200, data: { ok: true, message: "fine" } });
    await openCatalogs(ENGINE_ONLY);

    await screen.findByRole("table");
    expect(screen.getAllByRole("button", { name: /^Test$/ }).length).toBeGreaterThan(0);
    expect(screen.queryByRole("button", { name: /Detach/ })).not.toBeInTheDocument();
  });

  test("a StarRocks engine is not offered a test that only speaks Trino", async () => {
    vi.mocked(api.getEngine).mockResolvedValue({
      ...ENGINE,
      type: "starrocks",
      status: { phase: "running", message: null, pods: [] },
    } as unknown as Awaited<ReturnType<typeof api.getEngine>>);
    await openCatalogs();

    await screen.findByRole("table");
    expect(screen.queryByRole("button", { name: /^Test$/ })).not.toBeInTheDocument();
    // Not silence: the reason, and where a check that does work lives.
    expect(screen.getByTestId("engine-catalogs-test-unavailable")).toHaveTextContent(
      /Trino coordinator/,
    );
  });

  test("a successful change drops the Test verdict it has just invalidated", async () => {
    const { user, box } = await runTest("sales", { ok: true, message: "2 rows." });
    expect(box).toBeInTheDocument();

    await user.click(
      within(screen.getByRole("row", { name: /^sales/ })).getByRole("button", { name: /Detach/ }),
    );
    const dialog = await screen.findByRole("alertdialog");
    await user.type(await within(dialog).findByPlaceholderText("sales"), "sales");
    await user.click(within(dialog).getByRole("button", { name: /^Detach$/ }));

    await waitFor(() =>
      expect(screen.queryByTestId("engine-catalog-test-result")).not.toBeInTheDocument(),
    );
  });
});

describe("EngineDetail — Catalogs tab: one problem, said once", () => {
  beforeEach(catalogsTabBeforeEach);

  test("a reader blocked at catalog_registry:read is not also advised about :update", async () => {
    mockRegistryList([], { data: undefined, isError: true, error: new ApiError(403, "forbidden") });
    await openCatalogs(ENGINE_ONLY);

    const note = await screen.findByTestId("engine-catalogs-unreadable");
    expect(note).toHaveTextContent("catalog_registry:read");
    // Advice they cannot act on yet, in a near-identical second paragraph.
    expect(screen.queryByTestId("engine-catalogs-readonly")).not.toBeInTheDocument();
  });

  test("a reader who HAS :read but not :update is still told which key is missing", async () => {
    await openCatalogs(READER);

    await screen.findByRole("table");
    expect(screen.getByTestId("engine-catalogs-readonly")).toHaveTextContent(
      "catalog_registry:update",
    );
  });
});
