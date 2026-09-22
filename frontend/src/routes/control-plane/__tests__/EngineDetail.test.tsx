import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider, useMutation } from "@tanstack/react-query";
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
import { catalogKey } from "@/routes/control-plane/queryKeys";
import { AuthContext } from "@/auth/useAuth";
import type { Me } from "@/api/hooks";
import type { EngineDependentDto } from "@/api/generated/model/engineDependentDto";
import type { EngineWorkspaceDto } from "@/api/generated/model/engineWorkspaceDto";
import type { WorkspaceDto } from "@/api/generated/model/workspaceDto";
import type {
  CatalogAttachmentDto,
  EngineCatalogDto,
  RegistryCatalogDto,
} from "@/api/generated/model";

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
  warehouseId: "w1",
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

describe("EngineDetail — Workspaces tab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockGetEngine(ENGINE.spec);
    vi.mocked(api.listEngineCatalogs).mockResolvedValue([]);
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
    vi.mocked(api.listEngineCatalogs).mockResolvedValue([]);
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
    vi.mocked(api.listEngineCatalogs).mockResolvedValue([]);
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
    vi.mocked(api.listEngineCatalogs).mockResolvedValue([]);
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

  test("header shows a metadata row (image · endpoint · delegation)", async () => {
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });

    // image (falls back to imageVersion when starrocksConfig.feImage is absent)
    expect(
      screen.getByText("starrocks/fe-ubuntu:3.3-latest"),
    ).toBeInTheDocument();
    // endpoint host:port derived by kind (StarRocks → {name}-fe-service.{ns}:9030)
    expect(
      screen.getByText("analytics-trino-fe-service.engines:9030"),
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

describe("EngineDetail — StatusCard states", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(api.listEngineCatalogs).mockResolvedValue([]);
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
    vi.mocked(api.listEngineCatalogs).mockResolvedValue([]);
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
  // Two catalogs → the strip's catalog count resolves to 2.
  const CATALOGS = [
    { id: "c1", name: "iceberg", kind: "iceberg", isPrimary: true },
    { id: "c2", name: "pg", kind: "postgresql", isPrimary: false },
  ] as unknown as Awaited<ReturnType<typeof api.listEngineCatalogs>>;

  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(api.listEngineCatalogs).mockResolvedValue(CATALOGS);
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
    // Catalog count resolves from listEngineCatalogs → 2.
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
    vi.mocked(api.listEngineCatalogs).mockResolvedValue([]);
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
// `EngineCatalogDto.configError` — the field `GET /engines/{id}/catalogs` sets
// for a stored row the tightened validator now refuses.

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

/** The same three rows as `GET /engines/{id}/catalogs` sees them. */
function legacyRow(over: Partial<EngineCatalogDto> = {}): EngineCatalogDto {
  return { id: "c-1", name: "sales", kind: "postgresql", isPrimary: false, secretRef: null, ...over };
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

/** Open the Catalogs tab and hand back the user-event session that did it. */
async function openCatalogs(me: Me | null = ADMIN, entry?: string) {
  const user = userEvent.setup({ pointerEventsCheck: 0 });
  renderDetail(entry ?? `/control-plane/engines/${ENGINE.id}`, me);
  await screen.findByRole("heading", { name: ENGINE.name });
  await user.click(screen.getByRole("tab", { name: "Catalogs" }));
  return user;
}

/** The table row whose Catalog cell names `name`. */
function catalogRow(name: string) {
  return screen.getByRole("row", { name: new RegExp(name) });
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
    vi.mocked(api.listEngineCatalogs).mockResolvedValue([
      legacyRow(),
      legacyRow({ id: "c-2", name: "events", kind: "egret-warehouse" }),
    ]);
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
    vi.mocked(api.listEngineCatalogs).mockResolvedValue([]);
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
  });

  test("the broken row is marked, its reason is on screen, and it links to where it is repaired", async () => {
    vi.mocked(api.listEngineCatalogs).mockResolvedValue([
      legacyRow(),
      legacyRow({
        id: "c-2",
        name: "events",
        // Deliberately naming neither the catalog nor any word this screen
        // prints: a fixture that contains the string its own assertion
        // searches for is satisfied by the SERVER half whatever the client
        // renders (SP3c Task 7 review F2).
        configError: "missing field `warehouse` at line 1 column 34",
      }),
    ]);
    await openCatalogs();

    // Marked in the row, so scanning the table finds it…
    expect(within(await screen.findByRole("row", { name: /events/ })).getByText(
      /config invalid/i,
    )).toBeInTheDocument();
    // …and the reason is quoted in full, verbatim, below the table.
    const box = screen.getByTestId("engine-catalog-config-errors");
    expect(box).toHaveTextContent("missing field `warehouse` at line 1 column 34");
    // The repair path: the catalog's own page, named and linked.
    expect(within(box).getByRole("link", { name: /events/ })).toHaveAttribute(
      "href",
      "/control-plane/catalogs/c-2",
    );
  });

  test("Retry is offered but cannot work on a broken row, and says why", async () => {
    vi.mocked(api.listEngineCatalogs).mockResolvedValue([
      legacyRow(),
      legacyRow({ id: "c-2", name: "events", configError: "missing field `warehouse`" }),
    ]);
    await openCatalogs();

    const row = await screen.findByRole("row", { name: /events/ });
    expect(within(row).getByRole("button", { name: /Retry/ })).toBeDisabled();
    expect(within(row).getByText(/cannot render it for this engine/)).toBeInTheDocument();
    // The other row is untouched — the guard is per row, not a dead table.
    expect(within(catalogRow("sales")).getByRole("button", { name: /Detach/ })).toBeEnabled();
  });

  test("a row that parses is never marked broken", async () => {
    vi.mocked(api.listEngineCatalogs).mockResolvedValue([
      legacyRow(),
      legacyRow({ id: "c-2", name: "events" }),
    ]);
    await openCatalogs();

    await screen.findByRole("table");
    expect(screen.queryByText(/config invalid/i)).not.toBeInTheDocument();
    expect(screen.queryByTestId("engine-catalog-config-errors")).not.toBeInTheDocument();
    expect(screen.queryByTestId("engine-catalog-config-unchecked")).not.toBeInTheDocument();
  });

  test("egret says when it could NOT check, rather than letting silence read as fine", async () => {
    vi.mocked(api.listEngineCatalogs).mockRejectedValue(new ApiError(500, "store unavailable"));
    await openCatalogs();

    await screen.findByRole("table");
    const note = await screen.findByTestId("engine-catalog-config-unchecked");
    expect(note).toHaveTextContent(/could not check/i);
    expect(note).toHaveTextContent("store unavailable");
    // Not-checked is not the same as checked-and-clean, and must not be
    // painted as a defect either.
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
    vi.mocked(api.listEngineCatalogs).mockResolvedValue([
      legacyRow(),
      legacyRow({ id: "c-2", name: "events" }),
    ]);
    mockRegistryList([SALES, EVENTS, REMOTE, SPARE]);
    registryAttachFn = vi.fn();
    registryDetachFn = vi.fn();
    registryRetryFn = vi.fn();
    wire(registryApi.useControlPlaneCatalogRegistryRegistryAttachCatalog, registryAttachFn);
    wire(registryApi.useControlPlaneCatalogRegistryDetachCatalog, registryDetachFn);
    wire(registryApi.useControlPlaneCatalogRegistryRetryCatalogApply, registryRetryFn);
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

  test("without catalog_registry:read the rows still list, the Apply column is not invented, and the key is named", async () => {
    mockRegistryList([], {
      data: undefined,
      isError: true,
      error: new ApiError(403, "forbidden"),
    });
    await openCatalogs();

    // The engine's own catalog list is `engine:read`, which this page already
    // needs — so the names do not disappear with the registry.
    expect(within(await screen.findByRole("row", { name: /sales/ })).getByText("sales"))
      .toBeInTheDocument();
    // …but nothing pretends to know the apply state.
    expect(screen.queryByText("applied")).not.toBeInTheDocument();
    const note = screen.getByTestId("engine-catalogs-registry-blocked");
    expect(note).toHaveTextContent("catalog_registry:read");
    // Not the bare word `Error::Forbidden` renders: who can grant it, too.
    expect(note).toHaveTextContent(/administrator/i);
    expect(screen.queryByRole("combobox", { name: /Catalog to attach/ })).not.toBeInTheDocument();
  });

  test("a non-403 registry failure shows the server's own sentence, not a permission story", async () => {
    mockRegistryList([], {
      data: undefined,
      isError: true,
      error: new ApiError(500, "registry store unavailable"),
    });
    await openCatalogs();

    const note = await screen.findByTestId("engine-catalogs-registry-blocked");
    expect(note).toHaveTextContent("registry store unavailable");
    expect(note).not.toHaveTextContent("catalog_registry:read");
  });
});
