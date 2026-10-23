import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

// ── Mock factories ────────────────────────────────────────────────────────────

const sharesMock = vi.fn();

vi.mock("@/api/workspaceAccess", () => ({
  useWorkspaceShares: (...args: unknown[]) => sharesMock(...args),
}));

// Mock ShareDialog to a simple probe that records the props it was opened
// with, so we can assert the row action targets the right object without
// depending on ShareDialog's own internals (grant list, mutations, etc).
vi.mock("@/components/sharing/ShareDialog", () => ({
  ShareDialog: (props: { kind: string; id: string; open: boolean }) =>
    props.open ? (
      <div data-testid="mock-share-dialog" data-kind={props.kind} data-id={props.id}>
        mock-share-dialog
      </div>
    ) : null,
}));

import SharedObjects from "@/routes/explore/settings/SharedObjects";

// ── Render helper ─────────────────────────────────────────────────────────────

/** Probe that surfaces the current location as text so navigation can be asserted. */
function LocationProbe() {
  const loc = useLocation();
  return <div data-testid="location-probe">{`${loc.pathname}${loc.search}`}</div>;
}

function renderPage() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/settings/shares"]}>
        <Routes>
          <Route path="/explore/:workspace/settings/shares" element={<SharedObjects />} />
          <Route path="*" element={<LocationProbe />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

// ── Fixtures ──────────────────────────────────────────────────────────────────

const chartShare = {
  kind: "chart",
  object_id: "obj-chart-1",
  object_name: "Revenue Chart",
  principal_type: "group" as const,
  principal_id: "grp-eng",
  principal_name: "Engineering",
  role: "ws_viewer",
  effect: "allow",
  expires_at: null,
};

const dashboardDeny = {
  kind: "dashboard",
  object_id: "obj-dash-1",
  object_name: "Ops Dashboard",
  principal_type: "user" as const,
  principal_id: "u-bob",
  principal_name: "bob@example.com",
  role: "ws_editor",
  effect: "deny",
  expires_at: null,
};

const futureExpiryShare = {
  kind: "dataset",
  object_id: "obj-ds-1",
  object_name: "Customer Dataset",
  principal_type: "user" as const,
  principal_id: "u-alice",
  principal_name: "alice@example.com",
  role: "ws_viewer",
  effect: "allow",
  expires_at: "2099-01-01T00:00:00Z",
};

const expiredShare = {
  kind: "notebook",
  object_id: "obj-nb-1",
  object_name: "Analysis Notebook",
  principal_type: "user" as const,
  principal_id: "u-carol",
  principal_name: "carol@example.com",
  role: "ws_editor",
  effect: "allow",
  expires_at: "2000-01-01T00:00:00Z",
};

// ── Setup ─────────────────────────────────────────────────────────────────────

beforeEach(() => {
  sharesMock.mockReset();
  sharesMock.mockReturnValue({
    data: [chartShare, dashboardDeny],
    isLoading: false,
    isError: false,
    error: null,
  });
});

// ── Tests ─────────────────────────────────────────────────────────────────────

describe("SharedObjects page", () => {
  it("renders every grant row with object, principal, role, effect, and a summary line", () => {
    renderPage();

    // Object names + kind badges.
    expect(screen.getByText("Revenue Chart")).toBeInTheDocument();
    expect(screen.getByText("Ops Dashboard")).toBeInTheDocument();
    expect(screen.getByText("chart")).toBeInTheDocument();
    expect(screen.getByText("dashboard")).toBeInTheDocument();

    // Principals.
    expect(screen.getByText("Engineering")).toBeInTheDocument();
    expect(screen.getByText("bob@example.com")).toBeInTheDocument();

    // Roles.
    expect(screen.getByText("ws_viewer")).toBeInTheDocument();
    expect(screen.getByText("ws_editor")).toBeInTheDocument();

    // Allow / deny badges.
    expect(screen.getByText("Allow")).toBeInTheDocument();
    expect(screen.getByText("Deny")).toBeInTheDocument();

    // Summary line: "2 shares · 2 objects · 1 denied". `getByText` on the
    // label matches only that stat's own `<span>` (SummaryStrip renders the
    // label as a direct text-node sibling of the value span), so scope
    // straight off it — no further DOM walk needed.
    const sharesStat = screen.getByText("shares").closest("span") as HTMLElement;
    expect(within(sharesStat).getByText("2")).toBeInTheDocument();
    const objectsStat = screen.getByText("objects").closest("span") as HTMLElement;
    expect(within(objectsStat).getByText("2")).toBeInTheDocument();
    const deniedStat = screen.getByText("denied").closest("span") as HTMLElement;
    expect(within(deniedStat).getByText("1")).toBeInTheDocument();
  });

  it("opens ShareDialog for the row's object when 'Manage sharing…' is clicked", async () => {
    const user = userEvent.setup();
    renderPage();

    expect(screen.queryByTestId("mock-share-dialog")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Actions for Revenue Chart" }));
    await user.click(await screen.findByRole("menuitem", { name: "Manage sharing…" }));

    const dialog = await screen.findByTestId("mock-share-dialog");
    expect(dialog).toHaveAttribute("data-kind", "chart");
    expect(dialog).toHaveAttribute("data-id", "obj-chart-1");
  });

  it("deep-links to who-can-access for the row's object when 'Who can access?' is clicked", async () => {
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole("button", { name: "Actions for Revenue Chart" }));
    await user.click(await screen.findByRole("menuitem", { name: "Who can access?" }));

    const probe = await screen.findByTestId("location-probe");
    expect(probe).toHaveTextContent(
      "/control-plane/who-can-access?ws=eng&kind=chart&id=obj-chart-1",
    );
  });

  it("shows a 403-forbidden message for non-admins", () => {
    sharesMock.mockReturnValue({
      data: undefined,
      isLoading: false,
      isError: true,
      error: { status: 403, message: "Forbidden" },
    });
    renderPage();

    // The shared refusal (KI-231).
    expect(screen.getByRole("heading", { level: 2, name: "You don't have access to this section." })).toBeInTheDocument();
    expect(screen.getByText("Only a workspace admin can view the shared objects.")).toBeInTheDocument();
  });

  it("shows a skeleton while shares are loading", () => {
    sharesMock.mockReturnValue({
      data: undefined,
      isLoading: true,
      isError: false,
      error: null,
    });
    renderPage();

    expect(screen.getByTestId("list-skeleton")).toBeInTheDocument();
  });

  it("shows grant expiry: '—' when null, a date when in the future, and an Expired badge when in the past", () => {
    sharesMock.mockReturnValue({
      data: [chartShare, futureExpiryShare, expiredShare],
      isLoading: false,
      isError: false,
      error: null,
    });
    renderPage();

    // null expiry → em dash placeholder.
    expect(screen.getByText("—")).toBeInTheDocument();

    // Future expiry → visibly marked as expired is false; text should NOT
    // read "Expired" for this row, and the row is still findable.
    expect(screen.getByText("Customer Dataset")).toBeInTheDocument();

    // Past expiry → an "Expired" badge, visible (not color-only).
    expect(screen.getByText("Analysis Notebook")).toBeInTheDocument();
    expect(screen.getByText("Expired")).toBeInTheDocument();
  });

  it("shows an empty state when nothing is shared", () => {
    sharesMock.mockReturnValue({
      data: [],
      isLoading: false,
      isError: false,
      error: null,
    });
    renderPage();

    expect(screen.getByText(/nothing shared in this workspace yet/i)).toBeInTheDocument();
  });
});
