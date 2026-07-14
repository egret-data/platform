import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

import WorkspaceDetail from "@/routes/control-plane/WorkspaceDetail";
import * as wsApi from "@/api/generated/admin-workspaces/admin-workspaces";
import * as auditApi from "@/api/generated/control-plane-audit/control-plane-audit";
import * as accessReqApi from "@/api/accessRequests";
import type { WorkspaceDto } from "@/api/generated/model/workspaceDto";

vi.mock("@/api/generated/admin-workspaces/admin-workspaces");
vi.mock("@/api/generated/control-plane-audit/control-plane-audit");
vi.mock("@/api/accessRequests");
vi.mock("@/api/access", () => ({ listUsers: vi.fn(() => Promise.resolve([])) }));
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

function stubMutation() {
  return { mutate: vi.fn(), mutateAsync: vi.fn(), isPending: false };
}

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

  test("Resources tab shows deep-link cards to CP resource pages", async () => {
    const user = userEvent.setup();
    renderDetail();
    await screen.findByRole("heading", { name: "Engineering" });

    await user.click(screen.getByRole("tab", { name: "Resources" }));
    // Warehouses is per-workspace → deep-links with a ?workspace= filter.
    expect(await screen.findByRole("link", { name: "Warehouses" })).toHaveAttribute(
      "href",
      `/control-plane/warehouses?workspace=${WS.id}`,
    );
    // Engines is platform-global → plain link.
    expect(screen.getByRole("link", { name: "Engines" })).toHaveAttribute(
      "href",
      "/control-plane/engines",
    );
  });
});
