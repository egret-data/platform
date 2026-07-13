import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// ── Mock factories ────────────────────────────────────────────────────────────

const joinableMock = vi.fn();
const mineMock = vi.fn();
const createMutate = vi.fn();
const cancelMutate = vi.fn();

vi.mock("@/api/accessRequests", () => ({
  useJoinable: () => joinableMock(),
  useMyRequests: () => mineMock(),
  useCreateRequest: () => ({ mutate: createMutate, isPending: false }),
  useCancelRequest: () => ({ mutate: cancelMutate, isPending: false }),
}));

import RequestAccessDialog from "@/components/access/RequestAccessDialog";

// ── Render helper ─────────────────────────────────────────────────────────────

function renderDialog() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <RequestAccessDialog open onOpenChange={() => {}} />
    </QueryClientProvider>,
  );
}

// ── Fixtures ──────────────────────────────────────────────────────────────────

const salesWs = { id: "ws-1", slug: "sales", name: "Sales" };
const opsWs = { id: "ws-2", slug: "ops", name: "Ops" };

const pendingReq = {
  id: "req-1",
  workspace_id: "ws-3",
  workspace_name: "Eng",
  requester_id: "me-id",
  requester_name: "me@egret.local",
  requested_role: "ws_viewer",
  reason: null,
  status: "pending" as const,
  decided_by: null,
  decided_at: null,
  decision_note: null,
  expires_at: null,
  created_at: "2026-07-01T00:00:00Z",
};

const approvedReq = {
  ...pendingReq,
  id: "req-2",
  workspace_name: "Ops",
  status: "approved" as const,
  decided_by: "admin-id",
  decided_at: "2026-07-02T00:00:00Z",
};

// ── Setup ─────────────────────────────────────────────────────────────────────

beforeEach(() => {
  joinableMock.mockReset();
  mineMock.mockReset();
  createMutate.mockReset();
  cancelMutate.mockReset();

  joinableMock.mockReturnValue({ data: [salesWs, opsWs], isLoading: false });
  mineMock.mockReturnValue({ data: [], isLoading: false });
});

// ── Tests ─────────────────────────────────────────────────────────────────────

describe("RequestAccessDialog", () => {
  it("renders joinable workspaces + role select + reason, and submits the chosen values", async () => {
    const user = userEvent.setup();
    renderDialog();

    // Workspace select
    await user.click(screen.getByLabelText("Workspace"));
    await user.click(await screen.findByRole("option", { name: "Sales" }));

    // Role select — default is ws_viewer, switch to ws_editor
    await user.click(screen.getByLabelText("Role"));
    await user.click(await screen.findByRole("option", { name: "ws_editor" }));

    // Reason textarea
    await user.type(screen.getByLabelText("Reason"), "Need access for reporting");

    await user.click(screen.getByRole("button", { name: /submit|request/i }));

    await waitFor(() => {
      expect(createMutate).toHaveBeenCalledWith(
        expect.objectContaining({
          workspace_id: "ws-1",
          role: "ws_editor",
          reason: "Need access for reporting",
        }),
        expect.anything(),
      );
    });
  });

  it("disables submit until a workspace is chosen", () => {
    renderDialog();
    expect(screen.getByRole("button", { name: /submit|request/i })).toBeDisabled();
  });

  it("shows a pending request with a working Cancel button, and an approved request with no Cancel button", async () => {
    mineMock.mockReturnValue({ data: [pendingReq, approvedReq], isLoading: false });
    const user = userEvent.setup();
    renderDialog();

    expect(screen.getByText("Eng")).toBeInTheDocument();
    expect(screen.getByText("Ops")).toBeInTheDocument();

    const cancelButtons = screen.getAllByRole("button", { name: /cancel/i });
    expect(cancelButtons).toHaveLength(1);

    await user.click(cancelButtons[0]);
    await waitFor(() => {
      expect(cancelMutate).toHaveBeenCalledWith("req-1", expect.anything());
    });
  });

  it("shows the 'member of every workspace' message when there is nothing joinable", () => {
    joinableMock.mockReturnValue({ data: [], isLoading: false });
    renderDialog();
    expect(screen.getByText(/member of every workspace/i)).toBeInTheDocument();
  });
});
