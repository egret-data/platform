import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

import WorkspaceDetail from "@/routes/control-plane/WorkspaceDetail";
import * as wsApi from "@/api/generated/admin-workspaces/admin-workspaces";
import * as auditApi from "@/api/generated/control-plane-audit/control-plane-audit";
import * as engineApi from "@/api/generated/control-plane-engines/control-plane-engines";
import * as accessReqApi from "@/api/accessRequests";
import * as cpApi from "@/api/controlPlane";
import type { WorkspaceDto } from "@/api/generated/model/workspaceDto";
import type { WorkspaceEngineDto } from "@/api/generated/model/workspaceEngineDto";
import type { EngineDto } from "@/api/generated/model/engineDto";

vi.mock("@/api/generated/admin-workspaces/admin-workspaces");
vi.mock("@/api/generated/control-plane-audit/control-plane-audit");
vi.mock("@/api/generated/control-plane-engines/control-plane-engines");
vi.mock("@/api/accessRequests");
vi.mock("@/api/access", () => ({ listUsers: vi.fn(() => Promise.resolve([])) }));
vi.mock("@/api/controlPlane", () => ({ listWarehouses: vi.fn(), listStorage: vi.fn() }));
// Stub the members panel — it has its own API and its own tests.
vi.mock("@/components/access/MembersPanel", () => ({
  MembersPanel: ({ workspaceId }: { workspaceId: string }) => (
    <div data-testid="members-panel">members of {workspaceId}</div>
  ),
}));

const WS: WorkspaceDto = {
  id: "11111111-1111-1111-1111-111111111111",
  slug: "eng",
  name: "Engineering",
  status: "active",
  created_at: "2026-03-01T00:00:00Z",
  created_by: "u1",
  owner_name: "alice",
};

const ASSIGNED_ENGINE: WorkspaceEngineDto = {
  id: "engine-1",
  name: "analytics-trino",
  type: "trino",
  status: "running",
};

const ALL_ENGINES: EngineDto[] = [
  {
    id: "engine-1",
    name: "analytics-trino",
    type: "trino",
    namespace: "engines",
    status: "running",
    origin: "provisioned",
    spec: {
      coordinatorReplicas: 1,
      workerReplicas: 2,
      cpuMin: "1",
      cpuMax: "2",
      memory: "4Gi",
      imageVersion: "latest",
    },
  },
  {
    id: "engine-2",
    name: "reporting-trino",
    type: "trino",
    namespace: "engines",
    status: "running",
    origin: "provisioned",
    // JWT-enabled → assignable (the picker only offers these).
    spec: {
      coordinatorReplicas: 1,
      workerReplicas: 2,
      cpuMin: "1",
      cpuMax: "2",
      memory: "4Gi",
      imageVersion: "latest",
      trinoConfig: { auth: { jwt: { jwksUri: "http://kc/jwks" } } },
    },
  },
];

function stubMutation() {
  return { mutate: vi.fn(), mutateAsync: vi.fn(), isPending: false };
}

const assignMutate = vi.fn();
const unassignMutate = vi.fn();

function query<T>(data: T | undefined) {
  return {
    data: data ? { data, status: 200, headers: new Headers() } : undefined,
    isLoading: false,
    isError: false,
    error: null,
  };
}

function renderDetail() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[`/control-plane/workspaces/${WS.id}`]}>
        <Routes>
          <Route path="/control-plane/workspaces/:id" element={<WorkspaceDetail />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("WorkspaceDetail cockpit", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(wsApi.useAdminWorkspacesGetWs).mockReturnValue(
      query(WS) as unknown as ReturnType<typeof wsApi.useAdminWorkspacesGetWs>,
    );
    vi.mocked(wsApi.useAdminWorkspacesWorkspaceStats).mockReturnValue(
      query({ member_count: 7, query_count: 42 }) as unknown as ReturnType<
        typeof wsApi.useAdminWorkspacesWorkspaceStats
      >,
    );
    vi.mocked(wsApi.useAdminWorkspacesArchiveWs).mockReturnValue(
      stubMutation() as unknown as ReturnType<typeof wsApi.useAdminWorkspacesArchiveWs>,
    );
    vi.mocked(wsApi.useAdminWorkspacesRestoreWs).mockReturnValue(
      stubMutation() as unknown as ReturnType<typeof wsApi.useAdminWorkspacesRestoreWs>,
    );
    vi.mocked(wsApi.useAdminWorkspacesUpdateWs).mockReturnValue(
      stubMutation() as unknown as ReturnType<typeof wsApi.useAdminWorkspacesUpdateWs>,
    );
    vi.mocked(wsApi.useAdminWorkspacesTransferOwner).mockReturnValue(
      stubMutation() as unknown as ReturnType<typeof wsApi.useAdminWorkspacesTransferOwner>,
    );
    vi.mocked(auditApi.useControlPlaneAuditListAudit).mockReturnValue(
      query({ entries: [], nextCursor: null }) as unknown as ReturnType<
        typeof auditApi.useControlPlaneAuditListAudit
      >,
    );
    vi.mocked(accessReqApi.useWorkspaceRequests).mockReturnValue({
      data: [],
      isLoading: false,
    } as unknown as ReturnType<typeof accessReqApi.useWorkspaceRequests>);
    vi.mocked(accessReqApi.useApproveRequest).mockReturnValue(
      stubMutation() as unknown as ReturnType<typeof accessReqApi.useApproveRequest>,
    );
    vi.mocked(accessReqApi.useDenyRequest).mockReturnValue(
      stubMutation() as unknown as ReturnType<typeof accessReqApi.useDenyRequest>,
    );
    vi.mocked(engineApi.useControlPlaneEnginesListWorkspaceEngines).mockReturnValue(
      query([ASSIGNED_ENGINE]) as unknown as ReturnType<
        typeof engineApi.useControlPlaneEnginesListWorkspaceEngines
      >,
    );
    vi.mocked(engineApi.useControlPlaneEnginesListEngines).mockReturnValue(
      query(ALL_ENGINES) as unknown as ReturnType<
        typeof engineApi.useControlPlaneEnginesListEngines
      >,
    );
    vi.mocked(engineApi.useControlPlaneEnginesAssignWorkspaceEngine).mockReturnValue({
      mutate: assignMutate,
      mutateAsync: vi.fn(),
      isPending: false,
    } as unknown as ReturnType<typeof engineApi.useControlPlaneEnginesAssignWorkspaceEngine>);
    vi.mocked(engineApi.useControlPlaneEnginesUnassignWorkspaceEngine).mockReturnValue({
      mutate: unassignMutate,
      mutateAsync: vi.fn(),
      isPending: false,
    } as unknown as ReturnType<typeof engineApi.useControlPlaneEnginesUnassignWorkspaceEngine>);
    vi.mocked(cpApi.listWarehouses).mockResolvedValue([
      { id: "wh1", name: "wh-eng", workspaceId: WS.id, storageConnId: "s1", lakekeeperWarehouseId: "lk1" },
      { id: "wh2", name: "wh-other", workspaceId: "other-ws", storageConnId: "s1", lakekeeperWarehouseId: null },
    ]);
    vi.mocked(cpApi.listStorage).mockResolvedValue([
      { id: "s1", name: "rustfs-main" },
    ] as unknown as Awaited<ReturnType<typeof cpApi.listStorage>>);
  });

  test("overview shows owner, status, and member/query stats", async () => {
    renderDetail();
    expect(await screen.findByRole("heading", { name: "Engineering" })).toBeInTheDocument();
    expect(screen.getByText("alice")).toBeInTheDocument();
    expect(screen.getByText("7")).toBeInTheDocument(); // members
    expect(screen.getByText("42")).toBeInTheDocument(); // queries run
  });

  test("switching to Members renders the members panel keyed by slug", async () => {
    const user = userEvent.setup();
    renderDetail();
    await screen.findByRole("heading", { name: "Engineering" });

    await user.click(screen.getByRole("tab", { name: "Members" }));
    expect(await screen.findByTestId("members-panel")).toHaveTextContent("members of eng");
  });

  test("Resources tab lists this workspace's warehouses (not other workspaces')", async () => {
    const user = userEvent.setup();
    renderDetail();
    await screen.findByRole("heading", { name: "Engineering" });

    await user.click(screen.getByRole("tab", { name: "Resources" }));
    // This workspace's warehouse links by WAREHOUSE id; another workspace's is
    // excluded. There is no warehouse detail page any more — SP3c-1 moved its
    // tabs onto `CatalogDetail` — and `/control-plane/warehouses/:id` now
    // redirects to the matching catalog. The href is asserted unchanged on
    // purpose: the redirect is what keeps every remaining warehouse-id link
    // working without rewiring this card's query onto the registry list, and
    // `__tests__/warehouseRedirect.test.tsx` is what pins the other end of it.
    expect(await screen.findByRole("link", { name: "wh-eng" })).toHaveAttribute(
      "href",
      "/control-plane/warehouses/wh1",
    );
    expect(screen.queryByText("wh-other")).not.toBeInTheDocument();
    // "Manage" opens the Catalogs page scoped to this workspace — SP3c-2
    // Task 8 deleted the Warehouses list, and Catalogs supports the same
    // `?workspace=` filter server-side.
    expect(screen.getByRole("link", { name: "Manage" })).toHaveAttribute(
      "href",
      `/control-plane/catalogs?workspace=${WS.id}`,
    );
  });

  test("Resources tab lists an assigned engine", async () => {
    const user = userEvent.setup();
    renderDetail();
    await screen.findByRole("heading", { name: "Engineering" });

    await user.click(screen.getByRole("tab", { name: "Resources" }));
    expect(await screen.findByText("analytics-trino")).toBeInTheDocument();
  });

  test("Remove calls Unassign with the engine id", async () => {
    const user = userEvent.setup();
    renderDetail();
    await screen.findByRole("heading", { name: "Engineering" });

    await user.click(screen.getByRole("tab", { name: "Resources" }));
    await screen.findByText("analytics-trino");
    await user.click(screen.getByRole("button", { name: /Remove/i }));

    expect(unassignMutate).toHaveBeenCalledWith({ id: WS.id, engineId: "engine-1" });
  });

  test("Assign dialog assigns an unassigned engine", async () => {
    const user = userEvent.setup();
    renderDetail();
    await screen.findByRole("heading", { name: "Engineering" });

    await user.click(screen.getByRole("tab", { name: "Resources" }));
    await user.click(await screen.findByRole("button", { name: /Assign engine/i }));

    // The picker offers only the not-yet-assigned engine (engine-2).
    fireEvent.click(await screen.findByLabelText("Engine"));
    fireEvent.click(await screen.findByRole("option", { name: /reporting-trino/i }));
    await user.click(screen.getByRole("button", { name: "Assign" }));

    expect(assignMutate).toHaveBeenCalledWith({ id: WS.id, data: { engineId: "engine-2" } });
  });
});
