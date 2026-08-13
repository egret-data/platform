import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { ScheduleRun, ScheduleSummary } from "@/api/alerts";

const schedulesMock = vi.fn();
const scheduleMock = vi.fn(
  (): { data: unknown; isLoading: boolean } => ({ data: undefined, isLoading: false }),
);
const runsMock = vi.fn(() => ({ data: [] as ScheduleRun[], isLoading: false }));
const createMutate = vi.fn().mockResolvedValue({ id: "new" });
const updateMutate = vi.fn().mockResolvedValue({ id: "s1" });
const removeMutate = vi.fn().mockResolvedValue(undefined);
const runNowMutate = vi.fn().mockResolvedValue({ id: "r1", state: "success" });
const setActiveMutate = vi.fn().mockResolvedValue({ id: "s1", active: false });

vi.mock("@/api/alerts", async (orig) => ({
  ...(await orig<Record<string, unknown>>()),
  useSchedules: (...a: unknown[]) => schedulesMock(...a),
  useScheduleRuns: () => runsMock(),
  useSchedule: () => scheduleMock(),
  useScheduleMutations: () => ({
    create: { mutateAsync: createMutate, isPending: false },
    update: { mutateAsync: updateMutate, isPending: false },
    remove: { mutateAsync: removeMutate, isPending: false },
    runNow: { mutateAsync: runNowMutate, isPending: false },
    setActive: { mutateAsync: setActiveMutate, isPending: false },
  }),
}));

// The editor needs a database picker for alerts; stub the source lists.
vi.mock("@/api/databases", () => ({
  useDatabases: () => ({ data: [{ id: "db1", name: "Trino" }] }),
}));
vi.mock("@/api/dashboards", () => ({
  // Two, so a multi-target report has a second object to pick.
  useDashboards: () => ({
    data: [
      { id: "d1", name: "Ops" },
      { id: "d2", name: "Revenue" },
    ],
  }),
}));
vi.mock("@/api/charts", () => ({
  useCharts: () => ({ data: [{ id: "c1", name: "Revenue" }] }),
}));

import Alerts from "@/routes/explore/Alerts";

function row(over: Partial<ScheduleSummary> = {}): ScheduleSummary {
  return {
    id: "s1",
    type: "alert",
    name: "Orders dropped",
    description: "",
    active: true,
    crontab: "0 9 * * *",
    timezone: "UTC",
    next_run_at: "2026-08-12T09:00:00Z",
    last_state: "success",
    last_eval_at: "2026-08-11T09:00:00Z",
    owners: [{ user_id: "u1", email: "on-call@t.local" }],
    updated_at: "2026-08-11T09:00:00Z",
    can_manage: true,
    ...over,
  } as ScheduleSummary;
}

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MemoryRouter initialEntries={["/explore/eng/alerts"]}>
          <Routes>
            <Route path="/explore/:workspace/alerts" element={<Alerts />} />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  schedulesMock.mockReturnValue({ data: [row()], isLoading: false, isError: false });
  runsMock.mockReturnValue({ data: [], isLoading: false });
  scheduleMock.mockReturnValue({ data: undefined, isLoading: false });
});

describe("Alerts & Reports list", () => {
  it("renders a schedule with its state badge and humanised schedule", () => {
    renderPage();
    expect(screen.getByText("Orders dropped")).toBeInTheDocument();
    // The state badge is the reason the page is worth opening.
    expect(screen.getByText(/success/i)).toBeInTheDocument();
    expect(screen.getByText("on-call@t.local")).toBeInTheDocument();
  });

  it("opens the editor from the row's name, like every sibling list", async () => {
    renderPage();
    const name = screen.getByRole("link", { name: "Orders dropped" });
    expect(name).toHaveAttribute("href", "/explore/eng/alerts/s1");
  });

  it("asks the server for one type per tab, rather than filtering in the browser", async () => {
    const user = userEvent.setup();
    renderPage();
    // Alerts tab is the default.
    expect(schedulesMock).toHaveBeenCalledWith("eng", "alert", false);

    await user.click(screen.getByRole("tab", { name: /reports/i }));
    await waitFor(() => expect(schedulesMock).toHaveBeenCalledWith("eng", "report", false));
  });

  it("shows every run state distinctly, including noop and grace", () => {
    // noop ("ran, condition false") and grace ("suppressed") are the two
    // answers to "why didn't I get an email?"; collapsing either into success
    // loses the answer the page exists to give.
    schedulesMock.mockReturnValue({
      data: [
        row({ id: "a", name: "A", last_state: "noop" }),
        row({ id: "b", name: "B", last_state: "grace" }),
        row({ id: "c", name: "C", last_state: "error" }),
        row({ id: "d", name: "D", last_state: null }),
      ],
      isLoading: false,
      isError: false,
    });
    renderPage();
    expect(screen.getByText(/not triggered/i)).toBeInTheDocument();
    expect(screen.getByText(/on grace/i)).toBeInTheDocument();
    expect(screen.getByText(/^error$/i)).toBeInTheDocument();
    expect(screen.getByText(/never run/i)).toBeInTheDocument();
  });

  it("withholds every write control from a user the server says cannot manage", () => {
    // Withheld, not disabled: a greyed Delete advertises a capability the
    // reader does not have. The flag comes from the server because a session's
    // permissions carry platform grants only.
    schedulesMock.mockReturnValue({
      data: [row({ can_manage: false })],
      isLoading: false,
      isError: false,
    });
    renderPage();

    expect(screen.queryByRole("link", { name: /\+ alert/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /edit/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /delete/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /run now/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("switch")).not.toBeInTheDocument();
    // ...but the row itself is still readable.
    expect(screen.getByText("Orders dropped")).toBeInTheDocument();
  });

  it("offers the write controls to a user who can manage", () => {
    renderPage();
    // A LINK, not a button: creating opens its own route now, the way every
    // sibling list does, rather than a dialog over the list.
    const create = screen.getByRole("link", { name: /\+ alert/i });
    expect(create).toHaveAttribute("href", "/explore/eng/alerts/new");
  });

  it("opens the run log for a schedule", async () => {
    const user = userEvent.setup();
    runsMock.mockReturnValue({
      data: [
        {
          id: "r1",
          state: "error",
          started_at: "2026-08-11T09:00:00Z",
          ended_at: "2026-08-11T09:00:02Z",
          value: null,
          error: "table not found",
          notification_sent_at: null,
        },
      ],
      isLoading: false,
    });
    renderPage();

    await user.click(screen.getByRole("button", { name: /run log/i }));
    const log = await screen.findByRole("dialog");
    expect(within(log).getByText(/table not found/i)).toBeInTheDocument();
  });

  it("shows a humanised crontab, with the raw expression available as a tooltip", () => {
    renderPage();
    expect(screen.getByText("Every day at 09:00")).toBeInTheDocument();
    // The raw expression must still be reachable — a wrong humanisation would
    // be worse than the raw string, so the original is one hover away.
    expect(screen.getByTitle("0 9 * * *")).toBeInTheDocument();
  });

  it("falls back to the raw expression for a crontab it cannot humanise", () => {
    schedulesMock.mockReturnValue({
      data: [row({ crontab: "0 9 * * 1-5" })],
      isLoading: false,
      isError: false,
    });
    renderPage();
    expect(screen.getByText("0 9 * * 1-5")).toBeInTheDocument();
  });

  it("runs a schedule now and reports the result, for a user who can manage", async () => {
    const user = userEvent.setup();
    renderPage();

    // Row actions live in the `…` menu, so the item only exists once it is open.
    await user.click(screen.getByRole("button", { name: /actions for/i }));
    await user.click(await screen.findByRole("menuitem", { name: /run now/i }));
    await waitFor(() => expect(runNowMutate).toHaveBeenCalledWith("s1"));
  });

  it("withholds Run now from a user the server says cannot manage", async () => {
    const user = userEvent.setup();
    schedulesMock.mockReturnValue({
      data: [row({ can_manage: false })],
      isLoading: false,
      isError: false,
    });
    renderPage();

    // The menu still opens — Run log is a read action — but the managing items
    // are absent, not merely disabled.
    await user.click(screen.getByRole("button", { name: /actions for/i }));
    expect(await screen.findByRole("menuitem", { name: /run log/i })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: /run now/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: /delete/i })).not.toBeInTheDocument();
  });

  it("offers an active toggle to a user who can manage", async () => {
    const user = userEvent.setup();
    renderPage();

    const toggle = screen.getByRole("switch");
    expect(toggle).toBeChecked();
    await user.click(toggle);
    await waitFor(() =>
      expect(setActiveMutate).toHaveBeenCalledWith({ id: "s1", active: false }),
    );
  });

  it("withholds the active toggle from a user who cannot manage, showing a Paused badge instead", () => {
    // Not merely disabled: a viewer must not even see a control that will
    // 403 on click. The read-only fallback still answers "is this paused?".
    schedulesMock.mockReturnValue({
      data: [row({ can_manage: false, active: false })],
      isLoading: false,
      isError: false,
    });
    renderPage();
    expect(screen.queryByRole("switch")).not.toBeInTheDocument();
    // Exact case, not /paused/i: the summary strip also carries a lowercase
    // "paused" count, and a case-insensitive match would pass on that alone —
    // i.e. it would still pass with the row's read-only badge deleted.
    expect(screen.getByText("Paused")).toBeInTheDocument();
  });

  it("renders an empty state rather than a bare table when there is nothing", () => {
    schedulesMock.mockReturnValue({ data: [], isLoading: false, isError: false });
    renderPage();
    expect(screen.getByText(/no alerts yet/i)).toBeInTheDocument();
  });
});
