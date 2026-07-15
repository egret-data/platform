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
