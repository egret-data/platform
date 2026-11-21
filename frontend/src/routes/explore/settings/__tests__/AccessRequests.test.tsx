import { describe, it, expect, vi, beforeAll, beforeEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";

// ── Mock factories ────────────────────────────────────────────────────────────

const requestsMock = vi.fn();
const approveMutate = vi.fn();
const denyMutate = vi.fn();

vi.mock("@/api/accessRequests", () => ({
  useWorkspaceRequests: (...args: unknown[]) => requestsMock(...args),
  useApproveRequest: () => ({ mutate: approveMutate, isPending: false }),
  useDenyRequest: () => ({ mutate: denyMutate, isPending: false }),
}));

// Roles are fetched from the workspace-role catalog (shared with Members).
vi.mock("@/api/members", () => ({
  useWorkspaceRoles: () => ({
    data: [
      { id: "r-viewer", name: "ws_viewer", scope: "workspace" },
      { id: "r-editor", name: "ws_editor", scope: "workspace" },
      { id: "r-admin", name: "ws_admin", scope: "workspace" },
    ],
    isLoading: false,
    isError: false,
    error: null,
  }),
}));

import AccessRequests from "@/routes/explore/settings/AccessRequests";

// The expiry is the app's calendar (KI-276); its month grid loads lazily, so load it once here.
beforeAll(async () => {
  await import("@/components/ui/calendar");
}, 30_000);

/** "1st", "22nd", "30th": how the calendar names a day. */
const ordinal = (n: number) => {
  const suffixes = ["th", "st", "nd", "rd"];
  const v = n % 100;
  return `${n}${suffixes[(v - 20) % 10] ?? suffixes[v] ?? suffixes[0]}`;
};

// ── Render helper ─────────────────────────────────────────────────────────────

function renderPage() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MemoryRouter initialEntries={["/explore/eng/settings/requests"]}>
          <Routes>
            <Route path="/explore/:workspace/settings/requests" element={<AccessRequests />} />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

// ── Fixtures ──────────────────────────────────────────────────────────────────

const pending = {
  id: "req-1",
  workspace_id: "ws-1",
  workspace_name: "eng",
  requester_id: "alice-id",
  requester_name: "alice@egret.local",
  requested_role: "ws_editor",
  reason: "Need to build the Q3 dashboard",
  status: "pending" as const,
  decided_by: null,
  decided_at: null,
  decision_note: null,
  expires_at: null,
  created_at: "2026-07-10T10:00:00Z",
};

// ── Setup ─────────────────────────────────────────────────────────────────────

beforeEach(() => {
  requestsMock.mockReset();
  approveMutate.mockReset();
  denyMutate.mockReset();

  requestsMock.mockReturnValue({ data: [pending], isLoading: false, isError: false, error: null });
});

// ── Tests ─────────────────────────────────────────────────────────────────────

describe("AccessRequests page", () => {
  it("renders a pending request (requester, role, reason)", () => {
    renderPage();
    expect(screen.getByText("alice@egret.local")).toBeInTheDocument();
    expect(screen.getByText("ws_editor")).toBeInTheDocument();
    expect(screen.getByText(/Need to build the Q3 dashboard/)).toBeInTheDocument();
  });

  it("shows an empty state when there are no pending requests", () => {
    requestsMock.mockReturnValue({ data: [], isLoading: false, isError: false, error: null });
    renderPage();
    expect(screen.getByText(/No pending requests/i)).toBeInTheDocument();
  });

  it("shows a skeleton while loading", () => {
    requestsMock.mockReturnValue({ data: undefined, isLoading: true, isError: false, error: null });
    renderPage();
    expect(screen.getByTestId("list-skeleton")).toBeInTheDocument();
  });

  it("403 shows the admin-only alert", () => {
    requestsMock.mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: { status: 403 },
    });
    renderPage();
    // The shared refusal (KI-231).
    expect(screen.getByRole("heading", { level: 2, name: "You don't have access to this section." })).toBeInTheDocument();
    expect(screen.getByText("Only a workspace admin can review access requests.")).toBeInTheDocument();
  });

  it("approve flow: picking a role + date and confirming calls approve with { id, role, expires_at }", async () => {
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole("button", { name: /approve/i }));

    // Role select defaults to the requested role; pick ws_admin instead.
    await user.click(screen.getByLabelText(/role/i));
    await user.click(await screen.findByRole("option", { name: "ws_admin" }));

    // Optional expiry date.
    // The picker floors at today, so pick the month's last day: never in the past.
    await user.click(screen.getByRole("button", { name: /access expires/i }));
    const now = new Date();
    const last = new Date(now.getFullYear(), now.getMonth() + 1, 0).getDate();
    const month = now.toLocaleString("en-US", { month: "long" });
    await user.click(
      within(await screen.findByRole("grid")).getByRole("button", { name: new RegExp(`${month} ${ordinal(last)}`) }),
    );

    await user.click(screen.getByRole("button", { name: /confirm/i }));

    await waitFor(() => {
      expect(approveMutate).toHaveBeenCalledWith(
        expect.objectContaining({
          id: "req-1",
          role: "ws_admin",
          expires_at: expect.stringMatching(/^\d{4}-\d{2}-\d{2}T/),
        }),
        expect.anything(),
      );
    });
  });

  it("deny flow: entering a note and confirming calls deny with { id, note }", async () => {
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole("button", { name: /^deny$/i }));

    const note = screen.getByLabelText(/note/i);
    await user.type(note, "Not needed right now");

    await user.click(screen.getByRole("button", { name: /confirm/i }));

    await waitFor(() => {
      expect(denyMutate).toHaveBeenCalledWith(
        expect.objectContaining({ id: "req-1", note: "Not needed right now" }),
        expect.anything(),
      );
    });
  });
});
