import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { ScheduleRun, ScheduleSummary } from "@/api/alerts";
import { folderCollapseKey, UNCATEGORIZED_KEY } from "@/components/explore/folders";

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
const trashMutate = vi.fn().mockResolvedValue(undefined);
const folderRemoveMock = vi.fn().mockResolvedValue(undefined);
// Read inside the folders mock factory's closure, never at module scope, so a
// test can swap the folder list between renders.
let folders: { id: string; name: string }[] = [];

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
    trash: { mutateAsync: trashMutate, isPending: false },
    restore: { mutateAsync: vi.fn().mockResolvedValue(undefined), isPending: false },
    addBookmark: { mutateAsync: vi.fn().mockResolvedValue(undefined), isPending: false },
    removeBookmark: { mutateAsync: vi.fn().mockResolvedValue(undefined), isPending: false },
  }),
}));

// Unmocked, `useFolders` would fire a real query and the page would only ever
// render the single Uncategorized bucket — no folder to collapse.
vi.mock("@/api/folders", () => ({
  useFolders: () => ({ data: folders }),
  useSetFolder: () => ({ mutateAsync: vi.fn().mockResolvedValue({}) }),
  useFolderMutations: () => ({
    create: { mutateAsync: vi.fn().mockResolvedValue({ id: "f1", name: "F1" }) },
    rename: { mutateAsync: vi.fn().mockResolvedValue({}) },
    remove: { mutateAsync: folderRemoveMock },
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
  folders = [];
  // `useCollapsed` persists collapse state in localStorage under
  // `folders:eng:alerts`; without this a test that collapses a folder leaks
  // that collapse into every later test in the file.
  localStorage.clear();
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
    // A minute list: no phrase here says it honestly.
    schedulesMock.mockReturnValue({
      data: [row({ crontab: "0,30 9 * * *" })],
      isLoading: false,
      isError: false,
    });
    renderPage();
    expect(screen.getByText("0,30 9 * * *")).toBeInTheDocument();
  });

  it.each([
    ["0 * * * *", "Every hour at minute 0"],
    ["0 9 * * *", "Every day at 09:00"],
    ["30 8 * * 1-5", "Every Monday to Friday at 08:30"],
    ["0 9 1 * *", "Every month on day 1 at 09:00"],
  ])("shows %s in words in the Schedule column (KI-038)", (crontab, words) => {
    schedulesMock.mockReturnValue({
      data: [row({ crontab })],
      isLoading: false,
      isError: false,
    });
    renderPage();
    expect(screen.getByText(words)).toHaveAttribute("title", crontab);
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

/**
 * Bulk select must only ever reach rows that are ON SCREEN. `useBulkSelect` was
 * fed the pre-grouping `rows`, so "Select all" swept in the rows of a collapsed
 * folder and "Move n to Trash" trashed schedules the user had never seen.
 *
 * The fixture puts one schedule in each of TWO folders and collapses only one.
 * A fixture with everything inside the collapsed folder cannot tell a correct
 * fix from one that selects nothing at all — both end at an empty selection.
 */
describe("Alerts — bulk select only reaches rows on screen", () => {
  const TWO_FOLDERS = [{ id: "f-rep", name: "Reporting" }, { id: "f-fin", name: "Finance" }];
  const inTwoFolders = () => [
    row({ id: "s-hidden", name: "Hidden alert", folder_id: "f-rep" }),
    row({ id: "s-shown", name: "Shown alert", folder_id: "f-fin" }),
  ];

  it("leaves a collapsed folder's rows out of Select all and out of the trash call", async () => {
    schedulesMock.mockReturnValue({ data: inTwoFolders(), isLoading: false, isError: false });
    folders = TWO_FOLDERS;
    const user = userEvent.setup();
    renderPage();
    // Both folders start expanded, so both rows are genuinely on screen first.
    expect(screen.getByText("Hidden alert")).toBeInTheDocument();
    expect(screen.getByText("Shown alert")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Toggle Reporting" }));
    expect(screen.queryByText("Hidden alert")).not.toBeInTheDocument();
    // Finance is still open — the fix has something left to select.
    expect(screen.getByText("Shown alert")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    await user.click(screen.getByLabelText("Select all"));

    expect(screen.getByRole("button", { name: "Move 1 to Trash" })).toBeInTheDocument();
    expect(screen.queryByLabelText("Select Hidden alert")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Move 1 to Trash" }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Move to Trash" }));

    // The ids, not merely that it was called: `toHaveBeenCalled()` is satisfied
    // by trashing exactly the wrong schedule, which is the whole defect.
    await waitFor(() => expect(trashMutate).toHaveBeenCalledWith("s-shown"));
    expect(trashMutate).not.toHaveBeenCalledWith("s-hidden");
    expect(trashMutate).toHaveBeenCalledTimes(1);
  });

  it("drops a row from the selection when its folder collapses after it was picked", async () => {
    schedulesMock.mockReturnValue({ data: inTwoFolders(), isLoading: false, isError: false });
    folders = TWO_FOLDERS;
    const user = userEvent.setup();
    renderPage();

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    // SELECT first, while it is legitimately on screen…
    await user.click(screen.getByLabelText("Select Hidden alert"));
    await user.click(screen.getByLabelText("Select Shown alert"));
    expect(screen.getByRole("button", { name: "Move 2 to Trash" })).toBeInTheDocument();

    // …then HIDE it. `toggleAll` is not involved: the id is already in
    // `selected`, and only `selectedIds`' re-filter can take it back out.
    await user.click(screen.getByRole("button", { name: "Toggle Reporting" }));
    expect(screen.getByRole("button", { name: "Move 1 to Trash" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Move 1 to Trash" }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Move to Trash" }));

    await waitFor(() => expect(trashMutate).toHaveBeenCalledWith("s-shown"));
    expect(trashMutate).not.toHaveBeenCalledWith("s-hidden");
    expect(trashMutate).toHaveBeenCalledTimes(1);
  });
});

describe("Alerts on the Explore primitives", () => {
  it("reports the Bookmarks filter's engaged state with aria-pressed", async () => {
    renderPage();
    const btn = screen.getByRole("button", { name: "Bookmarks" });
    expect(btn).toHaveAttribute("aria-pressed", "false");
    await userEvent.click(btn);
    expect(btn).toHaveAttribute("aria-pressed", "true");
  });

  it("renders exactly one h1, and it is the page title", () => {
    renderPage();
    const h1s = screen.getAllByRole("heading", { level: 1 });
    expect(h1s).toHaveLength(1);
    expect(h1s[0]).toHaveTextContent("Alerts & Reports");
  });

  // Switching tabs must also clear the state facet. Without the reset, a facet
  // engaged on Alerts stays engaged over Reports while its control shows a
  // value the new list was never filtered by. This guards `setStateFilter(FACET_ALL)`
  // in Alerts.tsx's Tabs `onValueChange` — it must actually engage the facet
  // and read its control back, not merely click the tab and check the tab's
  // own `aria-selected`, which is Radix's behaviour and proves nothing about
  // the page.
  it("clears the state facet when the tab changes", async () => {
    const user = userEvent.setup();
    renderPage();

    // Engage the facet on the Alerts tab first, so the later assertion is a
    // genuine reset rather than a facet that was never set.
    await user.click(screen.getByLabelText("Last run"));
    await user.click(await screen.findByRole("option", { name: "Success" }));
    await user.click(screen.getByLabelText("Last run"));
    expect(await screen.findByRole("option", { name: "Success" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    await user.keyboard("{Escape}");

    await user.click(screen.getByRole("tab", { name: "Reports" }));

    // The facet control must have returned to FACET_ALL ("All"), not merely
    // whatever Radix's own tab state reports.
    await user.click(screen.getByLabelText("Last run"));
    expect(await screen.findByRole("option", { name: "All" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
  });

  // The tab bar is a page-level view switch and must survive the move into the
  // shell's body. It is NOT a shell slot.
  it("keeps the Alerts / Reports switcher on the page", () => {
    renderPage();
    expect(screen.getByRole("tab", { name: "Alerts" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Reports" })).toBeInTheDocument();
  });

  it("falls back to the error text when the error carries no message", () => {
    schedulesMock.mockReturnValue({ data: [], isLoading: false, isError: true, error: new Error("") });
    renderPage();
    expect(screen.getByText("Failed to load schedules.")).toBeInTheDocument();
  });
});

/**
 * spec §5.8: dataapi_alert rides the SAME Alerts list, badge-distinguished —
 * not a third tab. The backend's `type` filter is exact-match, so the page
 * fetches it as a second, narrower call and merges client-side, gated to the
 * Alerts tab (Task 6, fix round 1 — this whole surface was unreachable
 * without it: no tab, no entry point, no badge).
 */
describe("Alerts — dataapi_alert rides the Alerts tab (Task 6 fix round 1)", () => {
  /** Only `type` distinguishes the two calls `Alerts.tsx` now makes — every
   *  OTHER test in this file relies on `schedulesMock.mockReturnValue` (args-
   *  blind) instead, which is exactly why the merge is defensively re-filtered
   *  to rows actually typed `dataapi_alert` in the component itself: those
   *  other tests' single fixture leaks into both calls, and the filter is
   *  what keeps it from being double-counted without anyone having touched
   *  their setup. */
  function mockByType(dataapiRows: ScheduleSummary[], alertRows: ScheduleSummary[] = [row()]) {
    schedulesMock.mockImplementation((_ws: string, type?: string) => ({
      data: type === "dataapi_alert" ? dataapiRows : type === "report" ? [] : alertRows,
      isLoading: false,
      isError: false,
    }));
  }

  it("merges a dataapi_alert row into the Alerts tab, with a Data API badge", () => {
    mockByType([row({ id: "d1", name: "Latency spike", type: "dataapi_alert" })]);
    renderPage();
    expect(screen.getByText("Orders dropped")).toBeInTheDocument();
    expect(screen.getByText("Latency spike")).toBeInTheDocument();
    expect(screen.getByText("Data API")).toBeInTheDocument();
  });

  it("keeps dataapi_alert rows off the Reports tab", async () => {
    mockByType([row({ id: "d1", name: "Latency spike", type: "dataapi_alert" })]);
    const user = userEvent.setup();
    renderPage();
    expect(screen.getByText("Latency spike")).toBeInTheDocument();

    await user.click(screen.getByRole("tab", { name: /reports/i }));
    await waitFor(() =>
      expect(screen.queryByText("Latency spike")).not.toBeInTheDocument(),
    );
  });

  it("offers a Data API alert entry point beside + Alert, only on the Alerts tab", async () => {
    const user = userEvent.setup();
    renderPage();
    const dataapiLink = screen.getByRole("link", { name: /\+ data api alert/i });
    expect(dataapiLink).toHaveAttribute("href", "/explore/eng/alerts/new?type=dataapi_alert");
    // "+ Alert" itself is unchanged — still the plain link the tests above pin.
    expect(screen.getByRole("link", { name: /^\+ alert$/i })).toHaveAttribute(
      "href",
      "/explore/eng/alerts/new",
    );

    await user.click(screen.getByRole("tab", { name: /reports/i }));
    expect(screen.queryByRole("link", { name: /\+ data api alert/i })).not.toBeInTheDocument();
  });

  it("withholds the Data API alert entry point from a user who cannot manage", () => {
    schedulesMock.mockReturnValue({
      data: [row({ can_manage: false })],
      isLoading: false,
      isError: false,
    });
    renderPage();
    expect(screen.queryByRole("link", { name: /\+ data api alert/i })).not.toBeInTheDocument();
  });
});

/*
 * Item 1: the strip owns the singular. This page sets `singular` beside
 * `label`; the RULE (only the number one takes it) is tested once, on the
 * component, in `src/components/explore/__tests__/SummaryStrip.test.tsx`.
 * What is checked here is the WIRING — that the page passes the pair at all,
 * which is the half a component test cannot see and the half five pages had
 * missing.
 */
describe("Alerts — the summary strip says `1 alert`, not `1 alerts`", () => {
  it("uses the singular for one row and one folder", () => {
    schedulesMock.mockReturnValue({ data: [row()], isLoading: false, isError: false });
    folders = [{ id: "f1", name: "Revenue" }];
    renderPage();
    expect(screen.getByText("alert")).toBeInTheDocument();
    expect(screen.getByText("folder")).toBeInTheDocument();
    expect(screen.queryByText("alerts")).not.toBeInTheDocument();
    expect(screen.queryByText("folders")).not.toBeInTheDocument();
  });

  it("goes back to the plural at two", () => {
    schedulesMock.mockReturnValue({
      data: [row(), row({ id: "s2", name: "Signups flat" })],
      isLoading: false,
      isError: false,
    });
    folders = [{ id: "f1", name: "Revenue" }, { id: "f2", name: "Ops" }];
    renderPage();
    expect(screen.getByText("alerts")).toBeInTheDocument();
    expect(screen.getByText("folders")).toBeInTheDocument();
  });

  it("inflects the tab's own noun, not a hard-coded one", async () => {
    // `label` is `${typeLabel}s` and `singular` is `typeLabel`, so the Reports
    // tab must read "1 report" — a page that hard-coded "alert" would pass the
    // two tests above and fail this one.
    const user = userEvent.setup();
    schedulesMock.mockReturnValue({ data: [row({ type: "report" })], isLoading: false, isError: false });
    renderPage();
    await user.click(screen.getByRole("tab", { name: /reports/i }));
    expect(screen.getByText("report")).toBeInTheDocument();
    expect(screen.queryByText("reports")).not.toBeInTheDocument();
  });
});

/*
 * Item 2: the folderless-header gate, lifted from `dataapi/Endpoints.tsx` and
 * `sqllab/SavedTab.tsx` (the original precedent) into the five siblings that
 * headed the folderless bucket unconditionally.
 *
 * Both fixtures matter and neither is optional. Without folders is the case
 * being fixed; WITH a folder is what tells a working gate apart from one
 * wired permanently off, which the first test alone cannot see.
 */
describe("Alerts — no folders, no folderless header", () => {
  it("draws a flat table when the workspace has no folders", () => {
    schedulesMock.mockReturnValue({ data: [row(), row({ id: "s2", name: "Signups flat" })], isLoading: false, isError: false });
    folders = [];
    renderPage();
    expect(screen.getByText("Orders dropped")).toBeInTheDocument();
    expect(screen.getByText("Signups flat")).toBeInTheDocument();
    // "Uncategorized" beside nothing labels the whole list after the one thing
    // it is not, and its collapse control's only effect is to hide everything.
    expect(screen.queryByRole("button", { name: "Toggle Uncategorized" })).not.toBeInTheDocument();
  });

  it("still heads the folderless bucket beside a real folder", () => {
    schedulesMock.mockReturnValue({ data: [row({ folder_id: "f1" }), row({ id: "s2", name: "Signups flat" })], isLoading: false, isError: false });
    folders = [{ id: "f1", name: "Revenue" }];
    renderPage();
    expect(screen.getByRole("button", { name: "Toggle Revenue" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Toggle Uncategorized" })).toBeInTheDocument();
  });

  it("does not strand rows behind a collapse state whose header is no longer rendered", async () => {
    // What a previous session left behind: Uncategorized collapsed, from back
    // when this workspace still had folders. `useCollapsed` reads that out of
    // localStorage at mount and it survives the folders being deleted.
    //
    // Both halves are DERIVED, never spelled out: `folderCollapseKey` is the
    // same function the page hands `useCollapsed`, `UNCATEGORIZED_KEY` the same
    // constant it collapses under. A hardcoded key would land the seed where
    // nothing reads it and this test would still pass, because a seed that
    // misses is indistinguishable from a page that correctly ignored it.
    //
    // Rendering the rows is only half of it. `visibleRows` — what bulk select
    // acts on — is a SEPARATE derivation over the same collapse state, so
    // gating the render loop alone leaves "Select all" sweeping zero rows while
    // two sit in front of the user with no header left anywhere to expand.
    localStorage.setItem(folderCollapseKey("eng", "alerts"), JSON.stringify([UNCATEGORIZED_KEY]));
    schedulesMock.mockReturnValue({ data: [row(), row({ id: "s2", name: "Signups flat" })], isLoading: false, isError: false });
    folders = [];
    const user = userEvent.setup();
    renderPage();
    expect(screen.getByText("Orders dropped")).toBeInTheDocument();
    expect(screen.getByText("Signups flat")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    await user.click(screen.getByLabelText("Select all"));
    expect(screen.getByRole("button", { name: "Move 2 to Trash" })).toBeInTheDocument();
  });
});


// KI-281: the Alerts list trashed rows and deleted folders at once, while the Dashboards list asks
// first (KI-075).
describe("Alerts asks before trashing or deleting a folder (KI-281)", () => {
  it("a row's Move to Trash asks first, names the schedule, and trashes only on confirm", async () => {
    const user = userEvent.setup();
    renderPage();
    await user.click(screen.getByRole("button", { name: "Actions for Orders dropped" }));
    await user.click(await screen.findByRole("menuitem", { name: "Move to Trash" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(trashMutate).not.toHaveBeenCalled();
    expect(dialog.textContent).toContain('"Orders dropped" moves to the Trash');
    await user.click(within(dialog).getByRole("button", { name: "Move to Trash" }));
    await waitFor(() => expect(trashMutate).toHaveBeenCalledWith("s1"));
  });

  it("the bulk Move n to Trash asks first and counts the alerts", async () => {
    schedulesMock.mockReturnValue({ data: [row(), row({ id: "s2", name: "Signups flat" })], isLoading: false, isError: false });
    const user = userEvent.setup();
    renderPage();
    await user.click(screen.getByRole("button", { name: "Bulk select" }));
    await user.click(screen.getByLabelText("Select all"));
    await user.click(screen.getByRole("button", { name: "Move 2 to Trash" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(trashMutate).not.toHaveBeenCalled();
    expect(dialog.textContent).toContain("2 alerts move to the Trash");
    await user.click(within(dialog).getByRole("button", { name: "Move to Trash" }));
    await waitFor(() => expect(trashMutate).toHaveBeenCalledTimes(2));
  });

  it("a folder's Delete asks first and deletes only on confirm", async () => {
    schedulesMock.mockReturnValue({ data: [row({ folder_id: "f1" })], isLoading: false, isError: false });
    folders = [{ id: "f1", name: "F1" }];
    const user = userEvent.setup();
    renderPage();
    await user.click(screen.getByRole("button", { name: "Folder F1 actions" }));
    await user.click(await screen.findByRole("menuitem", { name: "Delete" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(folderRemoveMock).not.toHaveBeenCalled();
    expect(dialog.textContent).toContain("Everything in it moves to Uncategorized; nothing is deleted.");
    await user.click(within(dialog).getByRole("button", { name: "Delete folder" }));
    await waitFor(() => expect(folderRemoveMock).toHaveBeenCalledWith("f1"));
  });
});
