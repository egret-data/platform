import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

import EngineDetail from "@/routes/control-plane/EngineDetail";
import * as api from "@/api/controlPlane";
import * as engineApi from "@/api/generated/control-plane-engines/control-plane-engines";
import * as wsApi from "@/api/generated/admin-workspaces/admin-workspaces";
import type { EngineWorkspaceDto } from "@/api/generated/model/engineWorkspaceDto";
import type { WorkspaceDto } from "@/api/generated/model/workspaceDto";

vi.mock("@/api/controlPlane");
vi.mock("@/api/generated/control-plane-engines/control-plane-engines");
vi.mock("@/api/generated/admin-workspaces/admin-workspaces");

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

function renderDetail() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[`/control-plane/engines/${ENGINE.id}`]}>
        <Routes>
          <Route path="/control-plane/engines/:id" element={<EngineDetail />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
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

  test("shows both Configuration and Security tabs, and enables Restart", async () => {
    const user = userEvent.setup();
    renderDetail();
    await screen.findByRole("heading", { name: "analytics-trino" });

    // Configuration is StarRocks-shaped (Task 5); the Security & Exposure tab is
    // now available for StarRocks too (Task 6 — delegation + TLS).
    expect(screen.getByRole("tab", { name: "Configuration" })).toBeInTheDocument();
    expect(
      screen.getByRole("tab", { name: "Security & Exposure" }),
    ).toBeInTheDocument();
    // Other tabs stay available.
    expect(screen.getByRole("tab", { name: "Catalogs" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Monitor" })).toBeInTheDocument();

    // Restart is now enabled for StarRocks (backend supports it — Task 4).
    expect(screen.getByRole("button", { name: /restart/i })).toBeEnabled();

    // The StarRocks Configuration tab renders its FE/CN image fields.
    await user.click(screen.getByRole("tab", { name: "Configuration" }));
    expect(await screen.findByLabelText("FE image")).toBeInTheDocument();

    // The StarRocks Security tab renders its delegation control.
    await user.click(screen.getByRole("tab", { name: "Security & Exposure" }));
    expect(
      await screen.findByLabelText("Per-user delegation"),
    ).toBeInTheDocument();
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
