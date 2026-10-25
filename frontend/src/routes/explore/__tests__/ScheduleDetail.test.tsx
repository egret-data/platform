import { useState } from "react";
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { ScheduleRun, ScheduleSummary } from "@/api/alerts";

/** Pick `option` in the shared `ui/select` picker named `label`: open it, then click the item. */
async function pick(user: ReturnType<typeof userEvent.setup>, label: RegExp, option: string | RegExp) {
  await user.click(screen.getByLabelText(label));
  await user.click(await screen.findByRole("option", { name: option }));
}

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
// The dataapi_alert variant's endpoint picker — same envelope shape
// (`{status, data}`) `useDataApiListEndpoints`'s other consumers mock
// (Activity.test.tsx / ActivityCallList), so the `.data.status === 200`
// unwrap in ScheduleEditor gets real data rather than reading through `undefined`.
vi.mock("@/api/generated/data-api/data-api", () => ({
  useDataApiListEndpoints: () => ({
    data: {
      status: 200,
      data: [
        { id: "ep1", name: "Orders API" },
        { id: "ep2", name: "Users API" },
      ],
    },
  }),
}));
// Owners are picked from workspace members, and a chip must show the PERSON
// rather than the user id it is stored as.
vi.mock("@/api/members", () => ({
  useWorkspaceMembers: () => ({
    data: [
      { principal_id: "u1", principal_type: "user", principal_name: "on-call@t.local" },
      { principal_id: "u2", principal_type: "user", principal_name: "analyst@t.local" },
      { principal_id: "g1", principal_type: "group", principal_name: "data-team" },
    ],
  }),
}));

import ScheduleDetail from "@/routes/explore/ScheduleDetail";
import { FORM_ID, ScheduleEditor, type SectionKey } from "@/components/alerts/ScheduleEditor";

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

/**
 * Mounts `ScheduleEditor` directly for the `dataapi_alert` type, with its own
 * plain-button section nav — NOT `ScheduleDetail`'s `<Tabs>`. `ScheduleDetail`
 * still computes its tab label from a bare `isAlert` boolean (`type ===
 * "alert"`), which is out of this task's file scope (`ScheduleEditor.tsx`
 * only) and would show "Contents" rather than "Condition" for this type — a
 * cosmetic gap, not a functional one, recorded in the task report. Routing
 * around it here keeps these tests independent of that gap and of the
 * `SECTIONS` label text entirely.
 */
function DataapiAlertHarness({
  scheduleId = null,
  onDone = () => {},
}: {
  scheduleId?: string | null;
  onDone?: () => void;
}) {
  const [section, setSection] = useState<SectionKey>("general");
  return (
    <>
      {(["general", "what", "schedule", "notification"] as SectionKey[]).map((s) => (
        <button key={s} type="button" onClick={() => setSection(s)}>
          {s}
        </button>
      ))}
      <ScheduleEditor
        ws="eng"
        type="dataapi_alert"
        scheduleId={scheduleId}
        section={section}
        onDone={onDone}
      />
      <button type="submit" form={FORM_ID}>
        Save
      </button>
    </>
  );
}

function renderDataapiEditor(scheduleId: string | null = null) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>
        <MemoryRouter initialEntries={["/explore/eng/alerts/x"]}>
          <DataapiAlertHarness scheduleId={scheduleId} />
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
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
 *  fields the same way a person does.
 *
 *  `role: "tab"`, not "button": the section nav is a real tablist, which is
 *  what makes arrow-key navigation and the `aria-selected` state work. */
async function goTo(user: ReturnType<typeof userEvent.setup>, label: RegExp) {
  await user.click(screen.getByRole("tab", { name: label }));
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
    await pick(user, /repeat/i, "Every N minutes");
    await pick(user, /^every$/i, "5 minutes");
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
    // The control lives in Notification, not Condition: it decides what is IN
    // the message, and at the foot of the query section nobody found it.
    await goTo(user, /^notification$/i);
    await user.click(screen.getByLabelText(/also show a chart or dashboard/i));
    await user.click(screen.getByRole("button", { name: /^create$/i }));

    await waitFor(() => expect(createMutate).toHaveBeenCalled());
    const body = createMutate.mock.calls[0][0];
    expect(body.type).toBe("alert");
    expect(body.report_format).toBe("png");
    // Exactly one target, as the validator requires on either type.
    expect(Boolean(body.chart_id) !== Boolean(body.dashboard_id)).toBe(true);
  });

  it("sends the timers and retention the editor previously had no way to set", async () => {
    const user = userEvent.setup();
    renderEditor();
    await user.type(screen.getByLabelText(/^name$/i), "tuned");
    await goTo(user, /^condition$/i);
    await user.type(screen.getByLabelText(/sql/i), "SELECT 1");
    await goTo(user, /^notification$/i);
    await user.type(screen.getByLabelText(/email subject/i), "Orders are down");
    // The timers moved out of an "Advanced" tab and into Schedule, where the
    // rest of the WHEN lives — Superset's division, and the reason the tab is
    // gone entirely rather than merely renamed.
    await goTo(user, /^schedule$/i);
    await user.clear(screen.getByLabelText(/^grace period/i));
    await user.type(screen.getByLabelText(/^grace period/i), "600");
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
    await pick(user, /^format$/i, "PNG image");
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
    await userEvent.setup().click(screen.getByRole("tab", { name: /^contents$/i }));
    expect(screen.getByLabelText(/^dashboard$/i)).toHaveTextContent("Revenue");
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
    // The editor has no token field at all now: the bot token belongs to the
    // WORKSPACE (Settings -> Notifications), so three channels no longer mean
    // the same credential pasted three times. An absent `token` also still
    // means "keep whatever this recipient has", which is what leaves a
    // pre-workspace-app destination's own token untouched.
    expect(slack.config).toEqual({ channel: "#data-alerts" });
    expect(screen.queryByLabelText(/bot token/i)).not.toBeInTheDocument();
  });

  it("has four sections, and no Advanced", async () => {
    // The split follows Superset's: owners belong with the name, and the
    // timers belong with the schedule, because both are statements about the
    // thing beside them. An "Advanced" tab is where fields go when nobody
    // decided where they belong — it left General with three fields and
    // Schedule with two.
    renderEditor();

    const tabs = await screen.findAllByRole("tab");
    expect(tabs.map((t) => t.textContent)).toEqual([
      "General",
      "Condition",
      "Schedule",
      "Notification",
    ]);
  });

  it("keeps owners with the name and the timers with the schedule", async () => {
    const user = userEvent.setup();
    renderEditor();

    // Owners: General, beside what the thing is called.
    expect(screen.getByLabelText(/^owners$/i)).toBeInTheDocument();
    // Timers: Schedule, beside when it runs.
    await goTo(user, /^schedule$/i);
    expect(screen.getByLabelText(/^grace period/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/^working timeout/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/run log retention/i)).toBeInTheDocument();
  });

  it("shows an owner as a person, and keeps them through a no-op save", async () => {
    // Stored as a user id, read by a human. A chip of UUIDs is not a list of
    // people — and since the update REPLACES the owner set, a field that
    // could not render them would be one Save away from dropping them.
    const user = userEvent.setup();
    scheduleMock.mockReturnValue({
      data: { ...row(), sql: "SELECT 1", recipients: [], targets: [] },
      isLoading: false,
    });
    renderEditor("/explore/eng/alerts/s1");

    expect(await screen.findByText("on-call@t.local")).toBeInTheDocument();
    const recipientsIgnored = await saveExisting(user);
    expect(recipientsIgnored).toBeDefined();
    expect(updateMutate.mock.calls[0][0].body.owners).toEqual(["u1"]);
  });

  it("offers only people as owners, never groups", async () => {
    // A schedule runs its query as ONE identity. A group has no credentials
    // to delegate, so offering it would create a schedule that cannot run.
    renderEditor();

    // Focus, not click: Radix's popover trigger opens on the input's own
    // focus handler, and a jsdom click on the wrapper does not deliver the
    // pointer events the trigger listens for.
    screen.getByLabelText(/^owners$/i).focus();
    // The suggestion list is a Radix popover fed by a react-query lookup, so
    // it lands a tick or two after the click.
    expect(
      await screen.findByRole("button", { name: "analyst@t.local" }, { timeout: 3000 }),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "data-team" })).not.toBeInTheDocument();
  });

  it("shows recent runs under the form, so the badge's question has an answer", async () => {
    // The header says "Error". Without the runs on the same screen the only
    // way to read the error is to go back to the list and open a dialog.
    runsMock.mockReturnValue({
      data: [
        {
          id: "r1",
          state: "error",
          started_at: "2026-08-11T09:00:00Z",
          ended_at: "2026-08-11T09:00:02Z",
          value: null,
          error: "line 1:8: Column 'oops' cannot be resolved",
          notification_sent_at: null,
        },
      ] as ScheduleRun[],
      isLoading: false,
    });
    scheduleMock.mockReturnValue({
      data: { ...row({ last_state: "error" }), sql: "SELECT 1", recipients: [], targets: [] },
      isLoading: false,
    });
    renderEditor("/explore/eng/alerts/s1");

    expect(await screen.findByRole("heading", { name: /recent runs/i })).toBeInTheDocument();
    expect(screen.getByText(/cannot be resolved/i)).toBeInTheDocument();
  });

  it("shows no runs panel on a schedule that does not exist yet", async () => {
    // There is nothing to have run, and an empty "Recent runs" card on the
    // create page is a promise the page cannot keep.
    renderEditor();

    expect(await screen.findByRole("heading", { name: /new alert/i })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: /recent runs/i })).not.toBeInTheDocument();
  });

  it("heads the page with what the schedule IS, before any field", async () => {
    // The three questions a reader has before editing anything: is it on,
    // when does it run, and in which timezone. The crontab is humanised —
    // "0 9 * * *" is not an answer to "when does it run".
    scheduleMock.mockReturnValue({
      data: { ...row(), sql: "SELECT 1", recipients: [], targets: [] },
      isLoading: false,
    });
    renderEditor("/explore/eng/alerts/s1");

    expect(await screen.findByRole("heading", { name: /orders dropped/i })).toBeInTheDocument();
    expect(screen.getByText(/every day at 09:00/i)).toBeInTheDocument();
    expect(screen.getByText("UTC")).toBeInTheDocument();
    // Active, so the badge reports how the last run went.
    expect(screen.getByText(/success/i)).toBeInTheDocument();
  });

  it("says Paused rather than reporting the last run, when it is paused", async () => {
    // "Why has this not fired?" is the question a paused schedule has to
    // answer, and a green Success badge on a schedule that will never run
    // again answers it wrongly.
    scheduleMock.mockReturnValue({
      data: { ...row({ active: false }), sql: "SELECT 1", recipients: [], targets: [] },
      isLoading: false,
    });
    renderEditor("/explore/eng/alerts/s1");

    expect(await screen.findByText(/paused/i)).toBeInTheDocument();
    expect(screen.queryByText(/success/i)).not.toBeInTheDocument();
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

describe("Data API alert condition (Task 6)", () => {
  /** A saved dataapi_alert row, endpoint-scoped. */
  function dataapiRow(over: Record<string, unknown> = {}) {
    return {
      ...row({ type: "dataapi_alert" }),
      dataapi_endpoint_id: "ep1",
      dataapi_metric: "error_rate",
      dataapi_error_code: null,
      dataapi_window_s: 300,
      validator_type: "operator",
      validator_config: { op: ">", threshold: 0.5 },
      recipients: [],
      targets: [],
      ...over,
    };
  }

  it("always sends dataapi_endpoint_id, including an explicit null for the whole-workspace choice", async () => {
    // The load-bearing test from the Task 5 review: `PUT`/`POST` is a full
    // replace, and an OMITTED `dataapi_endpoint_id` is stored as `null` — the
    // same value "whole workspace" sends deliberately. A client that simply
    // stopped sending the key would silently widen an endpoint-scoped alert
    // to watch the entire workspace, 200 and no warning. This must fail if a
    // future edit reintroduces a conditional spread that omits the key.
    const user = userEvent.setup();
    renderDataapiEditor();
    await user.type(screen.getByLabelText(/^name$/i), "Error spike");
    await user.click(screen.getByRole("button", { name: "what" }));
    // Left as the default — "Whole workspace" — deliberately not touched.
    await user.click(screen.getByRole("button", { name: /^save$/i }));

    await waitFor(() => expect(createMutate).toHaveBeenCalled());
    const body = createMutate.mock.calls[0][0];
    expect(body.type).toBe("dataapi_alert");
    expect("dataapi_endpoint_id" in body).toBe(true);
    expect(body.dataapi_endpoint_id).toBeNull();
    // No not_null/operator picker exists for this type — it is pinned.
    expect(body.validator_type).toBe("operator");
  });

  it("sends the chosen endpoint's id when one is picked, not null", async () => {
    const user = userEvent.setup();
    renderDataapiEditor();
    await user.type(screen.getByLabelText(/^name$/i), "Latency up");
    await user.click(screen.getByRole("button", { name: "what" }));
    await pick(user, /^endpoint$/i, "Users API");
    await user.click(screen.getByRole("button", { name: /^save$/i }));

    await waitFor(() => expect(createMutate).toHaveBeenCalled());
    expect(createMutate.mock.calls[0][0].dataapi_endpoint_id).toBe("ep2");
  });

  it("resends dataapi_endpoint_id, unmodified, on a no-op update of an existing row", async () => {
    // The same guarantee as above, but on the UPDATE path — the one Task 5's
    // review actually found broken. Opening an endpoint-scoped alert and
    // saving without touching the endpoint field must keep watching that ONE
    // endpoint, not fall back to the workspace.
    scheduleMock.mockReturnValue({ data: dataapiRow(), isLoading: false });
    const user = userEvent.setup();
    renderDataapiEditor("s1");
    await screen.findByDisplayValue("Orders dropped");
    await user.click(screen.getByRole("button", { name: /^save$/i }));

    await waitFor(() => expect(updateMutate).toHaveBeenCalled());
    const body = updateMutate.mock.calls[0][0].body;
    expect("dataapi_endpoint_id" in body).toBe(true);
    expect(body.dataapi_endpoint_id).toBe("ep1");
  });

  it("keeps a stored endpoint selectable even when it's missing from the picker's own list", async () => {
    // Mirrors the Schedule section's timezone picker: a stored value the
    // browser's list does not contain (here, an endpoint since trashed) must
    // stay selected rather than silently reading as "Whole workspace" — an
    // untouched Save would otherwise widen the watch on a row nobody meant
    // to change.
    scheduleMock.mockReturnValue({
      data: dataapiRow({ dataapi_endpoint_id: "gone" }),
      isLoading: false,
    });
    const user = userEvent.setup();
    renderDataapiEditor("s1");
    await user.click(screen.getByRole("button", { name: "what" }));
    expect(await screen.findByLabelText(/^endpoint$/i)).toHaveTextContent(/^gone \(not visible/);

    await user.click(screen.getByRole("button", { name: /^save$/i }));
    await waitFor(() => expect(updateMutate).toHaveBeenCalled());
    expect(updateMutate.mock.calls[0][0].body.dataapi_endpoint_id).toBe("gone");
  });

  it("shows the error code field only for error_code_count, and sends it only then", async () => {
    const user = userEvent.setup();
    renderDataapiEditor();
    await user.type(screen.getByLabelText(/^name$/i), "Rate limited");
    await user.click(screen.getByRole("button", { name: "what" }));
    expect(screen.queryByLabelText(/^error code$/i)).not.toBeInTheDocument();

    await pick(user, /^metric$/i, "Count of a specific error code");
    await user.type(screen.getByLabelText(/^error code$/i), "rate_limited");
    await user.click(screen.getByRole("button", { name: /^save$/i }));

    await waitFor(() => expect(createMutate).toHaveBeenCalled());
    const body = createMutate.mock.calls[0][0];
    expect(body.dataapi_metric).toBe("error_code_count");
    expect(body.dataapi_error_code).toBe("rate_limited");
  });

  it("sends no error code for a metric other than error_code_count", async () => {
    const user = userEvent.setup();
    renderDataapiEditor();
    await user.type(screen.getByLabelText(/^name$/i), "Too few calls");
    await user.click(screen.getByRole("button", { name: "what" }));
    await pick(user, /^metric$/i, "Call count");
    await user.click(screen.getByRole("button", { name: /^save$/i }));

    await waitFor(() => expect(createMutate).toHaveBeenCalled());
    const body = createMutate.mock.calls[0][0];
    expect(body.dataapi_metric).toBe("call_count");
    expect(body.dataapi_error_code).toBeNull();
  });

  it("defaults the grace period to 900s, shorter than the alert/report default of 14400s", async () => {
    // §Task 6 Step 4: a `dataapi_alert` watches an operational signal, where
    // a four-hour default (migration 0110, inherited from Superset) is too
    // long to go quiet after a first page.
    const user = userEvent.setup();
    renderDataapiEditor();
    await user.click(screen.getByRole("button", { name: "schedule" }));
    expect(screen.getByLabelText(/^grace period/i)).toHaveValue("900");
  });

  it("sends the window and the comparison the user set", async () => {
    const user = userEvent.setup();
    renderDataapiEditor();
    await user.type(screen.getByLabelText(/^name$/i), "Slow endpoint");
    await user.click(screen.getByRole("button", { name: "what" }));
    await pick(user, /^metric$/i, "p95 latency");
    await user.clear(screen.getByLabelText(/^window/i));
    await user.type(screen.getByLabelText(/^window/i), "60");
    await pick(user, /^condition$/i, ">=");
    await user.clear(screen.getByLabelText(/^threshold$/i));
    await user.type(screen.getByLabelText(/^threshold$/i), "500");
    await user.click(screen.getByRole("button", { name: /^save$/i }));

    await waitFor(() => expect(createMutate).toHaveBeenCalled());
    const body = createMutate.mock.calls[0][0];
    expect(body.dataapi_metric).toBe("p95_duration_ms");
    expect(body.dataapi_window_s).toBe(60);
    expect(body.validator_config).toEqual({ op: ">=", threshold: 500 });
  });
});

/**
 * The page-chrome half of reachability (Task 6, fix round 1): `ScheduleDetail`
 * itself, not the direct-mount `ScheduleEditor` harness above. Before this
 * round, `type` had no `dataapi_alert` arm for a NEW schedule, and the header
 * (`Icon`/Type fact) and the `what` tab's own label were driven by a bare
 * `isAlert` boolean, which read a dataapi_alert as a report ("Contents",
 * "Report") rather than what it is.
 */
describe("ScheduleDetail — dataapi_alert reachability (Task 6 fix round 1)", () => {
  function dataapiExisting(over: Record<string, unknown> = {}) {
    return {
      ...row({ type: "dataapi_alert" }),
      dataapi_endpoint_id: null,
      dataapi_metric: "error_rate",
      dataapi_error_code: null,
      dataapi_window_s: 300,
      validator_type: "operator",
      validator_config: { op: ">", threshold: 0.1 },
      recipients: [],
      targets: [],
      ...over,
    };
  }

  it("reads the what-tab Condition, not Contents, for an existing dataapi_alert", async () => {
    scheduleMock.mockReturnValue({ data: dataapiExisting(), isLoading: false });
    renderEditor("/explore/eng/alerts/s1");

    expect(await screen.findByRole("tab", { name: /^condition$/i })).toBeInTheDocument();
    expect(screen.queryByRole("tab", { name: /^contents$/i })).not.toBeInTheDocument();
  });

  it("labels the header Type fact 'Data API alert', not 'Report'", async () => {
    scheduleMock.mockReturnValue({ data: dataapiExisting(), isLoading: false });
    renderEditor("/explore/eng/alerts/s1");

    expect(await screen.findByText("Data API alert")).toBeInTheDocument();
    expect(screen.queryByText(/^report$/i)).not.toBeInTheDocument();
  });

  it("opens a new schedule as a Data API alert from ?type=dataapi_alert, with the Condition tab", async () => {
    renderEditor("/explore/eng/alerts/new?type=dataapi_alert");

    expect(
      await screen.findByRole("heading", { name: /new data api alert/i }),
    ).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: /^condition$/i })).toBeInTheDocument();
  });
});

describe("Alerts & Reports UX (U7)", () => {
  const runs = [
    {
      id: "r1",
      state: "success",
      started_at: "2026-08-11T09:00:00Z",
      ended_at: "2026-08-11T09:00:02Z",
      value: null,
      error: null,
      notification_sent_at: "2026-08-11T09:00:02Z",
    },
  ] as ScheduleRun[];

  function existing(over: Record<string, unknown> = {}) {
    return {
      data: {
        ...row(),
        sql: "SELECT 1",
        database_id: "db1",
        validator_type: "not_null",
        validator_config: {},
        recipients: [],
        targets: [],
        ...over,
      },
      isLoading: false,
    };
  }

  // KI-038 — the header, the list and the editor read a schedule through one formatter.
  it("says an hourly schedule in words in the page header (KI-038)", async () => {
    scheduleMock.mockReturnValue(existing({ crontab: "0 * * * *" }));
    renderEditor("/explore/eng/alerts/s1");

    expect(await screen.findByText("Every hour at minute 0")).toBeInTheDocument();
  });

  it("previews the picked schedule in words, with the expression beside it (KI-038)", async () => {
    const user = userEvent.setup();
    renderEditor("/explore/eng/alerts/new");
    await goTo(user, /^schedule$/i);

    const preview = screen.getByText(/^Runs every day at 09:00/);
    expect(preview).toBeInTheDocument();
    expect(preview.textContent).toContain("0 9 * * *");
  });

  it("reads a raw cron expression back in words under the field (KI-038)", async () => {
    scheduleMock.mockReturnValue(existing({ crontab: "30 8 * * 1-5" }));
    renderEditor("/explore/eng/alerts/s1?section=schedule");

    expect(await screen.findByLabelText(/cron expression/i)).toHaveValue("30 8 * * 1-5");
    expect(screen.getByText(/Runs every Monday to Friday at 08:30/)).toBeInTheDocument();
  });

  // KI-039 — the option appears only where it does something.
  it.each([
    ["alert", true],
    ["report", false],
    ["dataapi_alert", false],
  ])("offers \"Don't send when there are no rows\" on a new %s: %s (KI-039)", async (type, shown) => {
    renderEditor(`/explore/eng/alerts/new?type=${type}&section=notification`);
    await screen.findByRole("tab", { name: /^notification$/i, selected: true });

    expect(screen.queryByLabelText(/don't send when there are no rows/i) !== null).toBe(shown);
  });

  // KI-171 — a report's run has no value, so its log has no Value column.
  it("shows no Value column in a report's run log (KI-171)", async () => {
    runsMock.mockReturnValue({ data: runs, isLoading: false });
    scheduleMock.mockReturnValue(existing({ type: "report" }));
    renderEditor("/explore/eng/alerts/s1");

    const panel = (await screen.findByRole("heading", { name: /recent runs/i })).parentElement!;
    const headers = within(panel).getAllByRole("columnheader").map((h) => h.textContent);
    expect(headers).toContain("Started");
    expect(headers).not.toContain("Value");
  });

  it("keeps the Value column in an alert's run log (KI-171)", async () => {
    runsMock.mockReturnValue({ data: [{ ...runs[0], value: 42 }], isLoading: false });
    scheduleMock.mockReturnValue(existing());
    renderEditor("/explore/eng/alerts/s1");

    const panel = (await screen.findByRole("heading", { name: /recent runs/i })).parentElement!;
    expect(within(panel).getAllByRole("columnheader").map((h) => h.textContent)).toContain("Value");
    expect(screen.getByText("42")).toBeInTheDocument();
  });

  // KI-219 — Details is one short line; the rest is one click away.
  it("keeps every Details cell of the Data API condition to one short line (KI-219)", async () => {
    renderEditor("/explore/eng/alerts/new?type=dataapi_alert&section=condition");
    await screen.findByRole("tab", { name: /^condition$/i, selected: true });

    for (const r of screen.getAllByRole("row").slice(1)) {
      const cells = r.querySelectorAll("td");
      if (cells.length < 3) continue;
      expect((cells[1].textContent ?? "").length, cells[0].textContent ?? "").toBeLessThanOrEqual(90);
    }
  });

  it("moves the long explanation behind a More button (KI-219)", async () => {
    const user = userEvent.setup();
    renderEditor("/explore/eng/alerts/new?type=dataapi_alert&section=condition");

    // Rendered twice: in the Details column and, below lg, under the label.
    const [more] = await screen.findAllByRole("button", { name: /more about window/i });
    await user.click(more);
    expect(await screen.findByText(/only the traffic still kept/i)).toBeInTheDocument();
  });

  // KI-220 — the Notification tab is one Setting | Details | Value table.
  it("lists destinations as rows of the notification table (KI-220)", async () => {
    const user = userEvent.setup();
    renderEditor("/explore/eng/alerts/new?section=notification");
    await addDestination(user, /email/i);

    const tables = screen.getAllByRole("table");
    expect(tables).toHaveLength(1);
    const table = tables[0];
    expect(within(table).getByRole("rowheader", { name: /deliver to/i })).toBeInTheDocument();
    expect(within(table).getByLabelText(/addresses/i)).toBeInTheDocument();
    expect(within(table).getByRole("button", { name: /add delivery method/i })).toBeInTheDocument();
  });

  // KI-221 — section keys are the tab labels; old keys still work; unknown ones say so.
  it.each([
    ["alert", "condition", /^condition$/i],
    ["alert", "what", /^condition$/i],
    ["dataapi_alert", "condition", /^condition$/i],
    ["dataapi_alert", "what", /^condition$/i],
    ["report", "contents", /^contents$/i],
    ["report", "what", /^contents$/i],
    ["alert", "schedule", /^schedule$/i],
    ["report", "notification", /^notification$/i],
  ])("opens %s ?section=%s on its tab (KI-221)", async (type, key, tab) => {
    renderEditor(`/explore/eng/alerts/new?type=${type}&section=${key}`);

    expect(await screen.findByRole("tab", { name: tab, selected: true })).toBeInTheDocument();
    expect(screen.queryByText(/no section called/i)).not.toBeInTheDocument();
  });

  it("falls back to General with a message for an unknown section (KI-221)", async () => {
    const user = userEvent.setup();
    renderEditor("/explore/eng/alerts/new?type=dataapi_alert&section=bogus");

    expect(await screen.findByRole("tab", { name: /^general$/i, selected: true })).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent(/no section called “bogus”/i);

    // Choosing a tab replaces the bad key, and the message goes with it.
    await goTo(user, /^schedule$/i);
    expect(screen.queryByText(/no section called/i)).not.toBeInTheDocument();
  });

  it("writes the tab label as the key when you change tab (KI-221)", async () => {
    const user = userEvent.setup();
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    function Spy() {
      return <output data-testid="search">{useLocation().search}</output>;
    }
    render(
      <QueryClientProvider client={qc}>
        <TooltipProvider>
          <MemoryRouter initialEntries={["/explore/eng/alerts/new?type=report"]}>
            <Routes>
              <Route path="/explore/:workspace/alerts/new" element={<><ScheduleDetail /><Spy /></>} />
            </Routes>
          </MemoryRouter>
        </TooltipProvider>
      </QueryClientProvider>,
    );
    await goTo(user, /^contents$/i);
    const search = screen.getByTestId("search").textContent ?? "";
    expect(new URLSearchParams(search).get("section")).toBe("contents");
  });
});

// S2 — opened straight on the Schedule section (a deep link or a reload), the
// picker must show the STORED schedule, and Save must send it back unchanged.
// Going General → Schedule mounted the picker after the record had landed, so
// only the deep link reached the controls before their values did.
describe("Schedule section opened by a deep link (S2)", () => {
  it.each([
    ["alert", "0 * * * *", "Hourly", "Asia/Ho_Chi_Minh"],
    ["report", "0 8 * * *", "Daily", "Europe/Berlin"],
    ["dataapi_alert", "0 8 * * *", "Daily", "America/New_York"],
  ])("a %s keeps %s in %s when saved untouched", async (type, crontab, period, timezone) => {
    const user = userEvent.setup();
    scheduleMock.mockReturnValue({
      data: {
        ...row({ type: type as ScheduleSummary["type"], crontab, timezone }),
        sql: "SELECT 1",
        database_id: "db1",
        validator_type: "not_null",
        validator_config: {},
        dashboard_id: type === "report" ? "d1" : null,
        dataapi_endpoint_id: type === "dataapi_alert" ? "ep1" : null,
        dataapi_metric: type === "dataapi_alert" ? "error_rate" : null,
        recipients: [],
        targets: [],
      },
      isLoading: false,
    });
    renderEditor("/explore/eng/alerts/s1?section=schedule");

    expect(await screen.findByLabelText(/^repeat$/i)).toHaveTextContent(period);
    expect(screen.getByLabelText(/^timezone$/i)).toHaveTextContent(timezone);
    const preview = screen.getByText(/^Runs every/).textContent;
    expect(preview).toContain(crontab);
    expect(preview).toContain(`in ${timezone}`);

    await user.click(screen.getByRole("button", { name: /^save$/i }));
    await waitFor(() => expect(updateMutate).toHaveBeenCalled());
    const body = updateMutate.mock.calls[0][0].body;
    expect(body.crontab).toBe(crontab);
    expect(body.timezone).toBe(timezone);
  });
});
