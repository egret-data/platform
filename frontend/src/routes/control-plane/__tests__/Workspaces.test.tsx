import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

import Workspaces from "@/routes/control-plane/Workspaces";
import * as wsApi from "@/api/generated/admin-workspaces/admin-workspaces";
import type { WorkspaceDto } from "@/api/generated/model/workspaceDto";

vi.mock("@/api/generated/admin-workspaces/admin-workspaces");

const WORKSPACES: WorkspaceDto[] = [
  {
    id: "11111111-1111-1111-1111-111111111111",
    slug: "eng",
    name: "Engineering",
    status: "active",
    created_at: "2026-03-01T00:00:00Z",
    created_by: "u1",
    owner_name: "alice",
  },
  {
    id: "22222222-2222-2222-2222-222222222222",
    slug: "old",
    name: "Old WS",
    status: "archived",
    created_at: "2026-01-01T00:00:00Z",
    created_by: null,
    owner_name: null,
  },
];

const restoreMutate = vi.fn();

function stubMutation(overrides: Record<string, unknown> = {}) {
  return { mutate: vi.fn(), mutateAsync: vi.fn(), isPending: false, ...overrides };
}

function mockList(rows: WorkspaceDto[], overrides: Record<string, unknown> = {}) {
  vi.mocked(wsApi.useAdminWorkspacesListWs).mockReturnValue({
    data: { data: rows, status: 200, headers: new Headers() },
    isLoading: false,
    isError: false,
    error: null,
    ...overrides,
  } as unknown as ReturnType<typeof wsApi.useAdminWorkspacesListWs>);
}

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>
        <Workspaces />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("Workspaces list page", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockList(WORKSPACES);
    vi.mocked(wsApi.useAdminWorkspacesArchiveWs).mockReturnValue(
      stubMutation() as unknown as ReturnType<typeof wsApi.useAdminWorkspacesArchiveWs>,
    );
    vi.mocked(wsApi.useAdminWorkspacesRestoreWs).mockReturnValue(
      stubMutation({ mutate: restoreMutate }) as unknown as ReturnType<
        typeof wsApi.useAdminWorkspacesRestoreWs
      >,
    );
    vi.mocked(wsApi.useAdminWorkspacesCreateWs).mockReturnValue(
      stubMutation() as unknown as ReturnType<typeof wsApi.useAdminWorkspacesCreateWs>,
    );
    vi.mocked(wsApi.useAdminWorkspacesUpdateWs).mockReturnValue(
      stubMutation() as unknown as ReturnType<typeof wsApi.useAdminWorkspacesUpdateWs>,
    );
    vi.mocked(wsApi.getAdminWorkspacesListWsQueryKey).mockReturnValue(["/api/admin/workspaces"]);
  });

  test("renders workspaces with a summary strip and owner", async () => {
    renderPage();
    expect(await screen.findByText("Engineering")).toBeInTheDocument();
    expect(screen.getByText("Old WS")).toBeInTheDocument();
    expect(screen.getByText("alice")).toBeInTheDocument();
    // summary strip totals
    expect(screen.getByText("2")).toBeInTheDocument(); // total
  });

  test("status facet filters to archived only", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText("Engineering");

    await user.click(screen.getByRole("combobox", { name: "Status" }));
    await user.click(await screen.findByRole("option", { name: "Archived" }));

    await waitFor(() => expect(screen.queryByText("Engineering")).not.toBeInTheDocument());
    expect(screen.getByText("Old WS")).toBeInTheDocument();
  });

  test("Restore action on an archived row fires the restore mutation", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText("Old WS");

    await user.click(screen.getByRole("button", { name: "Actions for Old WS" }));
    await user.click(await screen.findByText("Restore"));

    expect(restoreMutate).toHaveBeenCalledWith({
      id: "22222222-2222-2222-2222-222222222222",
    });
  });
});
