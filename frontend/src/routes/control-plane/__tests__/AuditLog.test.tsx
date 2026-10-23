import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { beforeAll, beforeEach, describe, expect, test, vi } from "vitest";

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
  // The date picker loads its month grid lazily. Transforming that module
  // (and date-fns under it) takes seconds on a cold run; load it once here so
  // no single test pays for it against its own timeout.
  beforeAll(async () => {
    await import("@/components/ui/calendar");
  }, 30_000);

  beforeEach(() => {
    vi.clearAllMocks();
    mockHook({ entries: ENTRIES, nextCursor: null });
  });

  test("renders decision and mutation rows with a destructive deny badge", async () => {
    renderPage();

    // Decision row: action + a destructive "Deny" status badge (the shared
    // `StatusBadge`: sentence case, the destructive tint — KI-222).
    expect(await screen.findByText("secret:read")).toBeInTheDocument();
    const denyBadge = screen.getByText("Deny");
    expect(denyBadge).toHaveAttribute("data-slot", "badge");
    expect(denyBadge).toHaveAttribute("data-status-tone", "destructive");
    expect(denyBadge.className).toContain("bg-destructive/15");

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
      expect.objectContaining({ hideAllowed: true, kind: undefined }),
      expect.anything(),
    );
    expect(screen.getByRole("combobox", { name: "Kind" })).toHaveTextContent("Changes and denials");

    await user.click(screen.getByRole("combobox", { name: "Kind" }));
    await user.click(await screen.findByRole("option", { name: "All" }));

    await waitFor(() =>
      expect(auditApi.useControlPlaneAuditListAudit).toHaveBeenLastCalledWith(
        expect.objectContaining({ hideAllowed: undefined, kind: undefined }),
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

  // KI-218: From and To are the app's date picker, not the browser's
  // datetime-local: the trigger reads YYYY-MM-DD HH:mm in every locale.
  // Each of these renders a month grid of 42 day buttons and walks it with
  // user-event, hence the longer timeout.
  const PICKER_TIMEOUT = 15_000;
  test("picks From from the app's calendar and time field, and filters from that instant", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText("secret:read");

    const from = screen.getByRole("button", { name: "From" });
    expect(from).toHaveTextContent("Any time");
    await user.click(from);
    const grid = await screen.findByRole("grid");
    // The calendar opens on the current month; the 15th exists in every month.
    await user.click(within(grid).getByRole("button", { name: /\b15th\b/ }));

    const now = new Date();
    const day = new Date(now.getFullYear(), now.getMonth(), 15, 0, 0);
    const pad = (n: number) => String(n).padStart(2, "0");
    const ymd = `${day.getFullYear()}-${pad(day.getMonth() + 1)}-15`;
    expect(screen.getByRole("button", { name: "From" })).toHaveTextContent(`${ymd} 00:00`);
    await waitFor(() =>
      expect(auditApi.useControlPlaneAuditListAudit).toHaveBeenLastCalledWith(
        expect.objectContaining({ from: day.toISOString(), to: undefined }),
        expect.anything(),
      ),
    );

    const time = screen.getByLabelText("Time");
    await user.clear(time);
    await user.type(time, "09:30{Enter}");
    const at930 = new Date(now.getFullYear(), now.getMonth(), 15, 9, 30);
    expect(screen.getByRole("button", { name: "From" })).toHaveTextContent(`${ymd} 09:30`);
    await waitFor(() =>
      expect(auditApi.useControlPlaneAuditListAudit).toHaveBeenLastCalledWith(
        expect.objectContaining({ from: at930.toISOString() }),
        expect.anything(),
      ),
    );
  }, PICKER_TIMEOUT);

  test("refuses a time that is not HH:mm, beside the field, and keeps the instant", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText("secret:read");

    await user.click(screen.getByRole("button", { name: "To" }));
    const grid = await screen.findByRole("grid");
    await user.click(within(grid).getByRole("button", { name: /\b15th\b/ }));
    const time = screen.getByLabelText("Time");
    await user.clear(time);
    await user.type(time, "25:00{Enter}");

    expect(time).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByText("Enter a time as HH:mm, from 00:00 to 23:59.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "To" })).toHaveTextContent(/ 23:59$/);
  }, PICKER_TIMEOUT);

  test("Clear removes the bound", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByText("secret:read");

    await user.click(screen.getByRole("button", { name: "From" }));
    await user.click(within(await screen.findByRole("grid")).getByRole("button", { name: /\b15th\b/ }));
    await user.click(screen.getByRole("button", { name: "Clear" }));

    expect(screen.getByRole("button", { name: "From" })).toHaveTextContent("Any time");
    await waitFor(() =>
      expect(auditApi.useControlPlaneAuditListAudit).toHaveBeenLastCalledWith(
        expect.objectContaining({ from: undefined }),
        expect.anything(),
      ),
    );
  }, PICKER_TIMEOUT);
});
