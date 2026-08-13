import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { ScheduleRun, ScheduleSummary } from "@/api/alerts";

// `vi.hoisted`: the mock factory is lifted above every `const`, so a plain
// top-level spy is not initialised yet when the factory runs.
const { toastSuccess } = vi.hoisted(() => ({ toastSuccess: vi.fn() }));
vi.mock("sonner", () => ({ toast: { success: toastSuccess, error: vi.fn() } }));

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

import ScheduleDetail from "@/routes/explore/ScheduleDetail";
import { ScheduleEditor } from "@/components/alerts/ScheduleEditor";

/** Mounts the editor alone so the dirty signal can be observed directly. */
function ScheduleEditorProbe({ onDirty }: { onDirty: (d: boolean) => void }) {
  return (
    <ScheduleEditor
      ws="eng"
      type="alert"
      scheduleId="s1"
      section="general"
      onDirtyChange={onDirty}
      onDone={() => {}}
    />
  );
}

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

/** Mount the create/edit ROUTE — the editor is no longer a dialog on the list. */
function renderEditor(path = "/explore/eng/alerts/new") {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MemoryRouter initialEntries={[path]}>
          <Routes>
            <Route path="/explore/:workspace/alerts/new" element={<ScheduleDetail />} />
            <Route path="/explore/:workspace/alerts/:id" element={<ScheduleDetail />} />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

/** Click through to a section — the form is five panels now, so a test fills
 *  fields the same way a person does. */
async function goTo(user: ReturnType<typeof userEvent.setup>, label: RegExp) {
  await user.click(screen.getByRole("button", { name: label }));
}

/** Add a delivery method through the menu, the way a person does. */
async function addDestination(user: ReturnType<typeof userEvent.setup>, kind: RegExp) {
  await user.click(screen.getByRole("button", { name: /add delivery method/i }));
  await user.click(await screen.findByRole("menuitem", { name: kind }));
}

/** A saved alert delivering to both an inbox and a signed webhook. */
function withWebhook() {
  scheduleMock.mockReturnValue({
    data: {
      ...row(),
      sql: "SELECT 1",
      database_id: "db1",
      validator_type: "not_null",
      validator_config: {},
      recipients: [
        { id: "r1", kind: "email", config: { addresses: ["on-call@t.local"] } },
        {
          id: "r2",
          kind: "webhook",
          config: { url: "https://hooks.example.com/egret/1", has_secret: true },
        },
      ],
    },
    isLoading: false,
  });
}

async function saveExisting(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByRole("button", { name: /^save$/i }));
  await waitFor(() => expect(updateMutate).toHaveBeenCalled());
  return updateMutate.mock.calls[0][0].body.recipients as {
    kind: string;
    config: Record<string, unknown>;
  }[];
}

beforeEach(() => {
  vi.clearAllMocks();
  schedulesMock.mockReturnValue({ data: [row()], isLoading: false, isError: false });
  runsMock.mockReturnValue({ data: [], isLoading: false });
  scheduleMock.mockReturnValue({ data: undefined, isLoading: false });
});

describe("Alerts & Reports editor page", () => {
  it("creates an alert through the real body shape", async () => {
    const user = userEvent.setup();
    renderEditor();

    await user.type(screen.getByLabelText(/^name$/i), "Revenue dip");
    await goTo(user, /^condition$/i);
    await user.type(screen.getByLabelText(/sql/i), "SELECT count(*) FROM orders");
    await goTo(user, /^schedule$/i);
    // The picker is the default path now; "every 5 minutes" is a dropdown
    // choice rather than an expression the user has to know how to write.
    await user.selectOptions(screen.getByLabelText(/repeat/i), "minutes");
    await user.selectOptions(screen.getByLabelText(/^every$/i), "5");
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

  it("lets an alert carry a chart attachment, which the model used to strip", async () => {
    // Superset allows this behind ALERTS_ATTACH_REPORTS; egret's model used to
    // null out chart_id/dashboard_id on every alert, so a fired alert could
    // only ever send CSV rows and never a picture.
    const user = userEvent.setup();
    renderEditor();
    await user.type(screen.getByLabelText(/^name$/i), "with picture");
    await goTo(user, /^condition$/i);
    await user.type(screen.getByLabelText(/sql/i), "SELECT 1");
    await user.click(screen.getByLabelText(/attach a chart or dashboard/i));
    await user.click(screen.getByRole("button", { name: /^create$/i }));

    await waitFor(() => expect(createMutate).toHaveBeenCalled());
    const body = createMutate.mock.calls[0][0];
    expect(body.type).toBe("alert");
    expect(body.report_format).toBe("png");
    // Exactly one target, as the validator requires on either type.
    expect(Boolean(body.chart_id) !== Boolean(body.dashboard_id)).toBe(true);
  });

  it("sends the advanced fields the editor previously had no way to set", async () => {
    const user = userEvent.setup();
    renderEditor();
    await user.type(screen.getByLabelText(/^name$/i), "tuned");
    await goTo(user, /^condition$/i);
    await user.type(screen.getByLabelText(/sql/i), "SELECT 1");
    await goTo(user, /^notification$/i);
    await user.type(screen.getByLabelText(/email subject/i), "Orders are down");
    await goTo(user, /^advanced$/i);
    await user.clear(screen.getByLabelText(/grace period/i));
    await user.type(screen.getByLabelText(/grace period/i), "600");
    await user.click(screen.getByRole("button", { name: /^create$/i }));

    await waitFor(() => expect(createMutate).toHaveBeenCalled());
    const body = createMutate.mock.calls[0][0];
    expect(body.email_subject).toBe("Orders are down");
    expect(body.grace_period_s).toBe(600);
    // Blank stays blank rather than becoming 0, which the server rejects.
    expect(body.working_timeout_s).toBe(3600);
  });

  it("acknowledges a save, rather than navigating away in silence", async () => {
    // Saving leaves the page, so with no toast the write lands with no
    // acknowledgement at all — every other mutation in this app says what it
    // did, and a silent success reads as a lost click.
    const user = userEvent.setup();
    renderEditor();
    await user.type(screen.getByLabelText(/^name$/i), "acknowledged");
    await goTo(user, /^condition$/i);
    await user.type(screen.getByLabelText(/sql/i), "SELECT 1");
    await user.click(screen.getByRole("button", { name: /^create$/i }));

    await waitFor(() => expect(createMutate).toHaveBeenCalled());
    await waitFor(() => expect(toastSuccess).toHaveBeenCalled());
  });

  it("does not claim unsaved changes on a schedule nobody edited", async () => {
    // A sanity check, NOT a regression guard for the bug that prompted it: the
    // guard used to key on any `change` event reaching the form, and Radix's
    // Select and Checkbox fire their own during hydration in a REAL browser.
    // jsdom does not, so this test passes with the old code too — verified by
    // restoring it and watching this still pass. The bug was found by Playwright
    // hanging on a beforeunload dialog, and only a browser can catch it again.
    const onDirty: boolean[] = [];
    scheduleMock.mockReturnValue({
      data: {
        ...row(),
        sql: "SELECT 1",
        database_id: "db1",
        validator_type: "not_null",
        validator_config: {},
        recipients: [],
      },
      isLoading: false,
    });
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={qc}>
        <TooltipProvider>
          <MemoryRouter initialEntries={["/explore/eng/alerts/s1"]}>
            <Routes>
              <Route
                path="/explore/:workspace/alerts/:id"
                element={
                  <ScheduleEditorProbe onDirty={(d) => onDirty.push(d)} />
                }
              />
            </Routes>
          </MemoryRouter>
        </TooltipProvider>
      </QueryClientProvider>,
    );

    await screen.findByDisplayValue("Orders dropped");
    await waitFor(() => expect(onDirty.length).toBeGreaterThan(0));
    expect(onDirty.some(Boolean)).toBe(false);
  });

  it("keeps typed values when you move between sections", async () => {
    // The panels unmount, so the values have to live in the form's state, not
    // in the DOM. If this breaks, a user loses everything they typed the moment
    // they look at another section.
    const user = userEvent.setup();
    renderEditor();
    await user.type(screen.getByLabelText(/^name$/i), "survives");
    await goTo(user, /^schedule$/i);
    await goTo(user, /^general$/i);
    expect(screen.getByLabelText(/^name$/i)).toHaveValue("survives");
  });

  it("jumps to the section holding the field you must fix", async () => {
    // `required` cannot do this job: a control in an unmounted section is not
    // focusable, so the browser blocks the submit and says nothing at all.
    const user = userEvent.setup();
    renderEditor();
    await goTo(user, /^notification$/i);
    await user.click(screen.getByRole("button", { name: /^create$/i }));

    expect(await screen.findByText(/a name is required/i)).toBeInTheDocument();
    // ...and it took us back to where the name lives.
    expect(screen.getByLabelText(/^name$/i)).toBeInTheDocument();
    expect(createMutate).not.toHaveBeenCalled();
  });

  it("keeps the raw cron field for an expression the picker cannot express", async () => {
    // `0 9 * * 1-5` has a weekday RANGE. Showing it as dropdowns would mean
    // rewriting it, which changes when the mail arrives — so the editor opens
    // on the raw field instead and the round trip is lossless.
    const user = userEvent.setup();
    scheduleMock.mockReturnValue({
      data: {
        ...row(),
        crontab: "0 9 * * 1-5",
        sql: "SELECT 1",
        database_id: "db1",
        validator_type: "not_null",
        validator_config: {},
        recipients: [],
      },
      isLoading: false,
    });
    renderEditor("/explore/eng/alerts/s1?section=schedule");
    const cron = await screen.findByLabelText(/cron expression/i);
    expect(cron).toHaveValue("0 9 * * 1-5");

    await user.click(screen.getByRole("button", { name: /^save$/i }));
    await waitFor(() => expect(updateMutate).toHaveBeenCalled());
    expect(updateMutate.mock.calls[0][0].body.crontab).toBe("0 9 * * 1-5");
  });

  it("sends a report body with a target rather than alert fields", async () => {
    const user = userEvent.setup();
    renderEditor("/explore/eng/alerts/new?type=report");

    await user.type(screen.getByLabelText(/^name$/i), "Weekly ops");
    await goTo(user, /^contents$/i);
    await user.click(screen.getByRole("button", { name: /^create$/i }));

    await waitFor(() => expect(createMutate).toHaveBeenCalled());
    const body = createMutate.mock.calls[0][0];
    expect(body.type).toBe("report");
    expect(body.report_format).toBe("png");
    // Exactly one target, and no alert-only fields riding along.
    expect(Boolean(body.dashboard_id) !== Boolean(body.chart_id)).toBe(true);
    expect(body.sql ?? null).toBeNull();
  });

  it("forces PDF once a report covers more than one object", async () => {
    // A PNG has no second page. Sending one object of several and letting the
    // recipient believe that is the whole report is the failure to avoid.
    const user = userEvent.setup();
    renderEditor("/explore/eng/alerts/new?type=report");
    await user.type(screen.getByLabelText(/^name$/i), "weekly pack");
    await goTo(user, /^contents$/i);
    await user.selectOptions(screen.getByLabelText(/^format$/i), "png");
    await user.click(screen.getByRole("button", { name: /add another/i }));
    await user.click(screen.getByRole("button", { name: /^create$/i }));

    await waitFor(() => expect(createMutate).toHaveBeenCalled());
    const body = createMutate.mock.calls[0][0];
    expect(body.report_format).toBe("pdf");
    expect(body.targets).toHaveLength(2);
    // Page order is the order the editor showed.
    expect(body.targets[0].object_id).not.toBe(body.targets[1].object_id);
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
    renderEditor();
    await user.type(screen.getByLabelText(/^name$/i), "no db");
    await goTo(user, /^condition$/i);
    await user.type(screen.getByLabelText(/sql/i), "SELECT 1");
    await user.click(screen.getByRole("button", { name: /^create$/i }));

    await waitFor(() => expect(createMutate).toHaveBeenCalled());
    const body = createMutate.mock.calls[0][0];
    expect(body.database_id).toBeNull();
    expect(body.database_id).not.toBe("");
  });

  it("keeps a webhook destination through a no-op save", async () => {
    // The update REPLACES the recipient set, so a body carrying only the email
    // field silently deletes the webhook. Opening the editor and pressing Save
    // without touching anything must change nothing.
    const user = userEvent.setup();
    withWebhook();
    renderEditor("/explore/eng/alerts/s1");

    const recipients = await saveExisting(user);
    expect(recipients.filter((r) => r.kind === "webhook")).toHaveLength(1);
    expect(recipients.find((r) => r.kind === "webhook")?.config.url).toBe(
      "https://hooks.example.com/egret/1",
    );
    // The email is still there too — the fix must not trade one for the other.
    expect(recipients.filter((r) => r.kind === "email")).toHaveLength(1);
  });

  it("sends a webhook as a bare url, never a credential", async () => {
    // The browser is never given the signing secret, so it must not try to
    // carry one: an absent `secret` means "keep the stored one" server-side.
    // Echoing `has_secret` back would also be wrong — it is a report, not an
    // input, and the server rebuilds the config from what it is sent.
    const user = userEvent.setup();
    withWebhook();
    renderEditor("/explore/eng/alerts/s1");

    const recipients = await saveExisting(user);
    const hook = recipients.find((r) => r.kind === "webhook");
    expect(hook?.config).toEqual({ url: "https://hooks.example.com/egret/1" });
  });

  it("opens prefilled from a Subscribe deep link", async () => {
    // The entry point both reference products have: you subscribe FROM the
    // dashboard, and land on a page already pointed at it.
    renderEditor("/explore/eng/alerts/new?type=report&kind=dashboard&object=d2");

    expect(await screen.findByRole("heading", { name: /new report/i })).toBeInTheDocument();
    await userEvent.setup().click(screen.getByRole("button", { name: /^contents$/i }));
    expect(screen.getByLabelText(/^dashboard$/i)).toHaveValue("d2");
  });

  it("sends a slack destination as a channel, never a token in the row", async () => {
    const user = userEvent.setup();
    renderEditor();
    await user.type(screen.getByLabelText(/^name$/i), "to slack");
    await goTo(user, /^condition$/i);
    await user.type(screen.getByLabelText(/sql/i), "SELECT 1");
    await goTo(user, /^notification$/i);
    await addDestination(user, /^slack$/i);
    await user.type(screen.getByLabelText(/^channel$/i), "#data-alerts");
    await user.click(screen.getByRole("button", { name: /^create$/i }));

    await waitFor(() => expect(createMutate).toHaveBeenCalled());
    const slack = createMutate.mock.calls[0][0].recipients.find(
      (r: { kind: string }) => r.kind === "slack",
    );
    expect(slack.config.channel).toBe("#data-alerts");
    // No token typed means "keep whatever is stored" — the browser must not
    // invent an empty one, which the server reads as "remove it".
    expect(slack.config.token).toBeUndefined();
  });

  it("delivers to several destinations of the same kind", async () => {
    // The flat form had one Slack field and one email field, so "post to
    // #data AND #exec" was not expressible — even though the server has
    // always stored recipients as a list and delivered to all of them.
    const user = userEvent.setup();
    renderEditor();
    await user.type(screen.getByLabelText(/^name$/i), "two channels");
    await goTo(user, /^condition$/i);
    await user.type(screen.getByLabelText(/sql/i), "SELECT 1");
    await goTo(user, /^notification$/i);

    await addDestination(user, /^slack$/i);
    await addDestination(user, /^slack$/i);
    const channels = screen.getAllByLabelText(/^channel$/i);
    expect(channels).toHaveLength(2);
    await user.type(channels[0], "#data");
    await user.type(channels[1], "#exec");
    await user.click(screen.getByRole("button", { name: /^create$/i }));

    await waitFor(() => expect(createMutate).toHaveBeenCalled());
    const slack = createMutate.mock.calls[0][0].recipients.filter(
      (r: { kind: string }) => r.kind === "slack",
    );
    expect(slack.map((r: { config: { channel: string } }) => r.config.channel)).toEqual([
      "#data",
      "#exec",
    ]);
  });

  it("removing a destination drops it from the saved set", async () => {
    // The update REPLACES the recipient set, so removal is the absence of a
    // row — which makes it worth pinning that the row really does disappear
    // and does not merely stop rendering.
    const user = userEvent.setup();
    withWebhook();
    renderEditor("/explore/eng/alerts/s1");

    await screen.findByRole("heading", { name: /orders dropped/i });
    await goTo(user, /^notification$/i);
    expect(screen.getByDisplayValue("https://hooks.example.com/egret/1")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /remove webhook destination/i }));
    const recipients = await saveExisting(user);

    expect(recipients.filter((r) => r.kind === "webhook")).toHaveLength(0);
    // The email it sat beside is untouched.
    expect(recipients.filter((r) => r.kind === "email")).toHaveLength(1);
  });

  it("carries a subject placeholder to the server verbatim", async () => {
    // The substitution happens at SEND time, in the schedule's own timezone —
    // a browser-rendered date would freeze the day the subject was written
    // into every future edition.
    const user = userEvent.setup();
    renderEditor();
    await user.type(screen.getByLabelText(/^name$/i), "daily orders");
    await goTo(user, /^condition$/i);
    await user.type(screen.getByLabelText(/sql/i), "SELECT 1");
    await goTo(user, /^notification$/i);
    // `{{` is userEvent's escape for a literal brace, so the template is
    // pasted rather than typed — otherwise the field receives `Orders {date}}`
    // and the assertion below passes for the wrong reason.
    await user.click(screen.getByLabelText(/email subject/i));
    await user.paste("Orders {{date}}");
    await user.click(screen.getByRole("button", { name: /^create$/i }));

    await waitFor(() => expect(createMutate).toHaveBeenCalled());
    expect(createMutate.mock.calls[0][0].email_subject).toBe("Orders {{date}}");
  });

  it("inserts a subject placeholder at the caret, not glued to the end", async () => {
    // Appending looked fine in the diff and wrong the moment it was clicked:
    // "Weekly overview" + the button gave `Weekly overview{{date}}`, which
    // SENDS as "Weekly overview2026-08-13" — no space, one run-on word.
    const user = userEvent.setup();
    renderEditor();
    await user.type(screen.getByLabelText(/^name$/i), "placement");
    await goTo(user, /^condition$/i);
    await user.type(screen.getByLabelText(/sql/i), "SELECT 1");
    await goTo(user, /^notification$/i);

    const subject = screen.getByLabelText(/email subject/i) as HTMLInputElement;
    await user.click(subject);
    await user.paste("Orders  - EU");
    // Caret between the two spaces, where a person would put it.
    subject.setSelectionRange(7, 7);
    await user.click(screen.getByRole("button", { name: "{{date}}" }));

    expect(subject.value).toBe("Orders {{date}} - EU");
  });

  it("sends the opening and closing notes as separate fields", async () => {
    // Two columns, not one: the picture is rendered BETWEEN them, so merging
    // them would put the whole note on one side of it.
    const user = userEvent.setup();
    renderEditor();
    await user.type(screen.getByLabelText(/^name$/i), "topped and tailed");
    await goTo(user, /^condition$/i);
    await user.type(screen.getByLabelText(/sql/i), "SELECT 1");
    await goTo(user, /^notification$/i);
    await user.type(screen.getByLabelText(/opening note/i), "Why you get this.");
    await user.type(screen.getByLabelText(/closing note/i), "Ask #data.");
    await user.click(screen.getByRole("button", { name: /^create$/i }));

    await waitFor(() => expect(createMutate).toHaveBeenCalled());
    const body = createMutate.mock.calls[0][0];
    expect(body.message).toBe("Why you get this.");
    expect(body.message_footer).toBe("Ask #data.");
  });

  it("surfaces the server's validation message instead of a generic failure", async () => {
    const user = userEvent.setup();
    createMutate.mockRejectedValueOnce(
      Object.assign(new Error("bad"), { body: { error: "invalid crontab: bad expression" } }),
    );
    renderEditor();
    await user.type(screen.getByLabelText(/^name$/i), "x");
    await goTo(user, /^condition$/i);
    await user.type(screen.getByLabelText(/sql/i), "SELECT 1");
    await user.click(screen.getByRole("button", { name: /^create$/i }));

    await waitFor(() =>
      expect(screen.getByText(/invalid crontab/i)).toBeInTheDocument(),
    );
  });

  it("scrolls a rejected save into view", async () => {
    // The banner is at the top of a form tall enough to scroll and Save is at
    // the bottom, so rendering the message is not the same as showing it —
    // without this, pressing Save looks like it did nothing.
    const scrollIntoView = vi.fn();
    HTMLElement.prototype.scrollIntoView = scrollIntoView;
    const user = userEvent.setup();
    createMutate.mockRejectedValueOnce(
      Object.assign(new Error("bad"), { body: { error: "webhook url must be http(s)" } }),
    );
    renderEditor();
    await user.type(screen.getByLabelText(/^name$/i), "x");
    await user.click(screen.getByRole("button", { name: /^create$/i }));

    await waitFor(() => expect(scrollIntoView).toHaveBeenCalled());
  });

});
