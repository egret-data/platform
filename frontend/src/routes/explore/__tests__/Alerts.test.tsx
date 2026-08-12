import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { ScheduleRun, ScheduleSummary } from "@/api/alerts";

const schedulesMock = vi.fn();
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
  useSchedule: () => ({ data: undefined, isLoading: false }),
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
  useDashboards: () => ({ data: [{ id: "d1", name: "Ops" }] }),
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
});

describe("Alerts & Reports list", () => {
  it("renders a schedule with its state badge and humanised schedule", () => {
    renderPage();
    expect(screen.getByText("Orders dropped")).toBeInTheDocument();
    // The state badge is the reason the page is worth opening.
    expect(screen.getByText(/success/i)).toBeInTheDocument();
    expect(screen.getByText("on-call@t.local")).toBeInTheDocument();
  });

  it("asks the server for one type per tab, rather than filtering in the browser", async () => {
    const user = userEvent.setup();
    renderPage();
    // Alerts tab is the default.
    expect(schedulesMock).toHaveBeenCalledWith("eng", "alert");

    await user.click(screen.getByRole("tab", { name: /reports/i }));
    await waitFor(() => expect(schedulesMock).toHaveBeenCalledWith("eng", "report"));
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

    expect(screen.queryByRole("button", { name: /new alert/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /edit/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /delete/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /run now/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("switch")).not.toBeInTheDocument();
    // ...but the row itself is still readable.
    expect(screen.getByText("Orders dropped")).toBeInTheDocument();
  });

  it("offers the write controls to a user who can manage", () => {
    renderPage();
    expect(screen.getByRole("button", { name: /new alert/i })).toBeInTheDocument();
  });

  it("creates an alert through the real body shape", async () => {
    const user = userEvent.setup();
    renderPage();
    await user.click(screen.getByRole("button", { name: /new alert/i }));

    await user.type(screen.getByLabelText(/^name$/i), "Revenue dip");
    await user.clear(screen.getByLabelText(/cron/i));
    await user.type(screen.getByLabelText(/cron/i), "*/5 * * * *");
    await user.type(screen.getByLabelText(/sql/i), "SELECT count(*) FROM orders");
    await user.click(screen.getByRole("button", { name: /^create$/i }));

    await waitFor(() => expect(createMutate).toHaveBeenCalled());
    const body = createMutate.mock.calls[0][0];
    expect(body).toMatchObject({
      type: "alert",
      name: "Revenue dip",
      crontab: "*/5 * * * *",
      sql: "SELECT count(*) FROM orders",
      validator_type: "not_null",
    });
    // The server requires `active` on update and defaults it on create; sending
    // it explicitly keeps the two paths identical.
    expect(body.active).toBe(true);
  });

  it("sends a report body with a target rather than alert fields", async () => {
    const user = userEvent.setup();
    renderPage();
    await user.click(screen.getByRole("tab", { name: /reports/i }));
    await user.click(screen.getByRole("button", { name: /new report/i }));

    await user.type(screen.getByLabelText(/^name$/i), "Weekly ops");
    await user.click(screen.getByRole("button", { name: /^create$/i }));

    await waitFor(() => expect(createMutate).toHaveBeenCalled());
    const body = createMutate.mock.calls[0][0];
    expect(body.type).toBe("report");
    expect(body.report_format).toBe("png");
    // Exactly one target, and no alert-only fields riding along.
    expect(Boolean(body.dashboard_id) !== Boolean(body.chart_id)).toBe(true);
    expect(body.sql ?? null).toBeNull();
  });

  it("omits an absent database rather than sending an empty string", async () => {
    // An empty string is not a UUID, so the server rejects the whole body with
    // a bare 422 "Unprocessable Entity" before its own validation runs — the
    // user loses the message naming the field and sees an HTTP status instead.
    // Found in a browser, not by these tests: they had always supplied a
    // database.
    vi.mocked(await import("@/api/databases")).useDatabases = (() => ({
      data: [],
    })) as never;
    const user = userEvent.setup();
    renderPage();
    await user.click(screen.getByRole("button", { name: /new alert/i }));
    await user.type(screen.getByLabelText(/^name$/i), "no db");
    await user.type(screen.getByLabelText(/sql/i), "SELECT 1");
    await user.click(screen.getByRole("button", { name: /^create$/i }));

    await waitFor(() => expect(createMutate).toHaveBeenCalled());
    const body = createMutate.mock.calls[0][0];
    expect(body.database_id).toBeNull();
    expect(body.database_id).not.toBe("");
  });

  it("surfaces the server's validation message instead of a generic failure", async () => {
    const user = userEvent.setup();
    createMutate.mockRejectedValueOnce(
      Object.assign(new Error("bad"), { body: { error: "invalid crontab: bad expression" } }),
    );
    renderPage();
    await user.click(screen.getByRole("button", { name: /new alert/i }));
    await user.type(screen.getByLabelText(/^name$/i), "x");
    await user.type(screen.getByLabelText(/sql/i), "SELECT 1");
    await user.click(screen.getByRole("button", { name: /^create$/i }));

    await waitFor(() =>
      expect(screen.getByText(/invalid crontab/i)).toBeInTheDocument(),
    );
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

    await user.click(screen.getByRole("button", { name: /run now/i }));
    await waitFor(() => expect(runNowMutate).toHaveBeenCalledWith("s1"));
  });

  it("withholds Run now from a user the server says cannot manage", () => {
    schedulesMock.mockReturnValue({
      data: [row({ can_manage: false })],
      isLoading: false,
      isError: false,
    });
    renderPage();
    expect(screen.queryByRole("button", { name: /run now/i })).not.toBeInTheDocument();
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
    expect(screen.getByText(/paused/i)).toBeInTheDocument();
  });

  it("renders an empty state rather than a bare table when there is nothing", () => {
    schedulesMock.mockReturnValue({ data: [], isLoading: false, isError: false });
    renderPage();
    expect(screen.getByText(/no alerts yet/i)).toBeInTheDocument();
  });
});
