import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";

import AuditLog from "@/routes/control-plane/AuditLog";
import * as auditApi from "@/api/generated/control-plane-audit/control-plane-audit";
import type { AuditEntryDto } from "@/api/generated/model/auditEntryDto";

vi.mock("@/api/generated/control-plane-audit/control-plane-audit");

const ENTRIES: AuditEntryDto[] = [
  {
    id: 1,
    ts: "2026-07-10T10:00:00Z",
    kind: "decision",
    actorUserId: "user-abc-12345",
    actorEmail: "alice@egret.local",
    workspaceId: null,
    action: "secret:read",
    resourceType: "secret",
    resourceId: "db-token",
    decision: "deny",
    reasons: ["no matching grant"],
    detail: {},
  },
  {
    id: 2,
    ts: "2026-07-10T09:00:00Z",
    kind: "mutation",
    actorUserId: null,
    actorEmail: null,
    workspaceId: null,
    action: "grant.create",
    resourceType: "access_grant",
    resourceId: null,
    decision: null,
    reasons: [],
    detail: { role: "ws_viewer" },
  },
];

type Page = { entries: AuditEntryDto[]; nextCursor?: string | null };

/**
 * Stub the generated hook with a STABLE return reference so the page's
 * append effect (keyed on `query.data`) doesn't re-run every render — mirrors
 * react-query's structural sharing in production.
 */
function mockHook(page: Page | undefined, overrides: Record<string, unknown> = {}) {
  const result = {
    data: page ? { data: page, status: 200, headers: new Headers() } : undefined,
    isLoading: false,
    isError: false,
    error: null,
    isFetching: false,
    ...overrides,
  };
  vi.mocked(auditApi.useControlPlaneAuditListAudit).mockReturnValue(
    result as unknown as ReturnType<typeof auditApi.useControlPlaneAuditListAudit>,
  );
}

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>
        <AuditLog />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("AuditLog", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockHook({ entries: ENTRIES, nextCursor: null });
  });

  test("renders decision and mutation rows with a destructive deny badge", async () => {
    renderPage();

    // Decision row: action + a destructive "deny" badge.
    expect(await screen.findByText("secret:read")).toBeInTheDocument();
    const denyBadge = screen.getByText("deny");
    expect(denyBadge).toHaveAttribute("data-slot", "badge");
    expect(denyBadge.className).toContain("bg-destructive");

    // Mutation row: action renders; a null actor shows as "system".
    expect(screen.getByText("grant.create")).toBeInTheDocument();
    expect(screen.getByText("system")).toBeInTheDocument();
  });

  test("names the actor by the email the API returns", async () => {
    renderPage();
    // KI-018: the decision row's actor (user-abc-12345) surfaces as its email.
    expect(await screen.findByText("alice@egret.local")).toBeInTheDocument();
  });

  // KI-017: page views write an allowed decision each; the log opens on what
  // changed and what was refused.
  test("opens on changes and denials, and All shows everything", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText("secret:read");
    expect(auditApi.useControlPlaneAuditListAudit).toHaveBeenLastCalledWith(
      expect.objectContaining({ hide_allowed: true, kind: undefined }),
      expect.anything(),
    );
    expect(screen.getByRole("combobox", { name: "Kind" })).toHaveTextContent("Changes and denials");

    await user.click(screen.getByRole("combobox", { name: "Kind" }));
    await user.click(await screen.findByRole("option", { name: "All" }));

    await waitFor(() =>
      expect(auditApi.useControlPlaneAuditListAudit).toHaveBeenLastCalledWith(
        expect.objectContaining({ hide_allowed: undefined, kind: undefined }),
        expect.anything(),
      ),
    );
  });

  test("shows the empty state when there are no entries", () => {
    mockHook({ entries: [], nextCursor: null });
    renderPage();
    expect(
      screen.getByText("No audit entries match the filters."),
    ).toBeInTheDocument();
  });

  test("changing the Kind filter re-queries with kind: decision", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText("secret:read");

    await user.click(screen.getByRole("combobox", { name: "Kind" }));
    await user.click(await screen.findByRole("option", { name: "Decision" }));

    await waitFor(() =>
      expect(auditApi.useControlPlaneAuditListAudit).toHaveBeenCalledWith(
        expect.objectContaining({ kind: "decision" }),
        expect.anything(),
      ),
    );
  });
});
