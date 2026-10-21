import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

import { FakeWorkflowServer, FLOW_ID, WS } from "@/routes/workflows/__tests__/fakeWorkflowServer";
import { backfill, flowEntry, flowTriggers, otherEntry, scheduleEntry, webhookEntry } from "./triggerFixtures";
import { renderTriggersTab } from "./renderTriggersTab";
import { workflowsKeys } from "@/routes/workflows/queryKeys";
import { TRIGGERS_POLL_MS } from "@/components/workflows/triggers/FlowTriggersTab";

const FLOW_TRIGGERS = `/api/ws/${WS}/workflows/flows/${FLOW_ID}/triggers`;
const BULK = `/api/ws/${WS}/workflows/triggers/disabled`;
const switchPath = (id: string) => `${FLOW_TRIGGERS}/${id}/disabled`;
const OVERDUE = new Date(Date.now() - 10 * 60_000).toISOString();
const FUTURE = new Date(Date.now() + 10 * 60_000).toISOString();

let server: FakeWorkflowServer;

beforeEach(() => {
  for (const f of Object.values(toast)) f.mockReset();
  server = new FakeWorkflowServer();
});
afterEach(() => vi.unstubAllGlobals());

const theSwitch = (id: string) => screen.findByRole("switch", { name: `Trigger ${id} enabled` });
const rowOf = (id: string) => screen.getByRole("link", { name: id }).closest("tr")!;
const statusOf = (id: string) => within(rowOf(id)).getByTestId("trigger-status").textContent;

/** One trigger of each kind; each with a different state so the status column can be told apart. */
function eachKind() {
  server.triggers = flowTriggers({
    triggers: [
      scheduleEntry(
        { last_execution: { id: "aaaaaaaa-0000-4000-8000-000000000009", state: "SUCCESS", created_at: OVERDUE } },
        { next_evaluation_date: FUTURE },
      ),
      webhookEntry(),
      flowEntry({ flow: { states: ["SUCCESS"], listening: false, not_listening_reason: "the namespace does not render", depends_on: null } }),
      otherEntry(),
    ],
  });
}

describe("FlowTriggersTab: the table", () => {
  it("shows the columns, and each trigger's type and status in words", async () => {
    eachKind();
    renderTriggersTab(server);
    await theSwitch("daily");
    expect(screen.getAllByRole("columnheader").map((h) => h.textContent?.trim())).toEqual([
      "",
      "ID",
      "Type",
      "Next run",
      "Last evaluated",
      "Last execution",
      "Status",
      "Enabled",
      "Backfill",
      "Details",
    ]);
    expect(statusOf("daily")).toBe("Active");
    expect(statusOf("hook")).toBe("Active");
    expect(statusOf("upstream")).toBe("Not listening: the namespace does not render");
    expect(statusOf("poll")).toBe("Not run by egret");
    // Type: the kind (for a screen reader) and the short type name.
    const typeCell = (id: string) => within(rowOf(id)).getAllByRole("cell")[2].textContent;
    expect(typeCell("daily")).toBe("Schedule: Schedule");
    expect(typeCell("upstream")).toBe("Flow: Flow");
    expect(typeCell("poll")).toBe("Other: Trigger");
    // The last execution: its state chip and a link to its overview.
    expect(within(rowOf("daily")).getByText("Success")).toBeInTheDocument();
    expect(within(rowOf("daily")).getByRole("link", { name: "aaaaaaaa" })).toHaveAttribute(
      "href",
      `/explore/${WS}/workflows/executions/aaaaaaaa-0000-4000-8000-000000000009/overview`,
    );
    // A Leaf trigger's switch is disabled, with the reason beside it (FE-ACTION-4).
    const poll = await theSwitch("poll");
    expect(poll).toBeDisabled();
    expect(poll).toHaveAccessibleDescription("Not run by egret");
  });

  it("explains Last evaluated in its tooltip", async () => {
    eachKind();
    renderTriggersTab(server);
    await theSwitch("daily");
    await userEvent.hover(screen.getByRole("button", { name: "About Last evaluated" }));
    expect((await screen.findAllByText("The schedule tick the scheduler last processed, not the wall-clock time it fired.")).length).toBeGreaterThan(0);
  });
});

describe("FlowTriggersTab: the switch", () => {
  it("switches a webhook off at once, and moves only when the server has answered", async () => {
    eachKind();
    let release!: () => void;
    server.putGate = new Promise((r) => (release = r));
    const { client } = renderTriggersTab(server);
    // A page of the workspace's Triggers list (Task 7), cached: a switch must mark it stale.
    const globalPage = workflowsKeys.triggers(WS, { kind: "webhook" });
    client.setQueryData(globalPage, { status: 200, data: { rows: [] } });
    const hook = await theSwitch("hook");
    expect(hook).toBeChecked();
    const readsBefore = server.callsTo("GET", FLOW_TRIGGERS).length;
    await userEvent.click(hook);
    await waitFor(() => expect(server.callsTo("PUT", switchPath("hook"))).toHaveLength(1));
    expect(server.callsTo("PUT", switchPath("hook"))[0].body).toEqual({ disabled: true, recover_missed: false });
    // Asked, not answered: the switch still shows the server's state, and keeps the focus,
    // ignoring a second click.
    expect(await theSwitch("hook")).toBeChecked();
    expect(hook).toHaveAttribute("aria-disabled", "true");
    // Not `disabled`: a browser blurs a focused button that becomes disabled (jsdom does not,
    // so `toHaveFocus` alone cannot tell).
    expect(hook).toBeEnabled();
    expect(hook).toHaveFocus();
    await userEvent.click(hook);
    release();
    await waitFor(async () => expect(await theSwitch("hook")).not.toBeChecked());
    expect(server.callsTo("PUT", switchPath("hook"))).toHaveLength(1);
    expect(hook).toHaveFocus();
    expect(hook).not.toHaveAttribute("aria-disabled");
    expect(statusOf("hook")).toMatch(/^Switched off by Ana Lima /);
    expect(toast.success).toHaveBeenCalledWith("Switched hook off.");
    // FE-ACTION-2: the flow's triggers are read again, and the workspace's list is stale.
    await waitFor(() => expect(server.callsTo("GET", FLOW_TRIGGERS).length).toBeGreaterThan(readsBefore));
    expect(client.getQueryState(globalPage)?.isInvalidated).toBe(true);
  });

  it("leaves the switch where it was on a refusal, and shows the server's sentence", async () => {
    server.triggers = flowTriggers({ triggers: [webhookEntry({ runtime_disabled: true, effective_disabled: true })] });
    server.triggerPutReply = {
      status: 409,
      body: { error: "conflict: the trigger is disabled in the flow's YAML; remove disabled: true there first", code: "trigger_disabled_in_source" },
    };
    renderTriggersTab(server);
    const hook = await theSwitch("hook");
    expect(hook).not.toBeChecked();
    // Every later read fails, so a refetch cannot repair a switch the refusal moved:
    // what is asserted below is what the page did with the refusal itself.
    server.triggersReply = { status: 503, body: { error: "the database is unavailable" } };
    await userEvent.click(hook);
    await waitFor(() => expect(toast.error).toHaveBeenCalled());
    expect(toast.error.mock.calls[0][0]).toContain("the trigger is disabled in the flow's YAML; remove disabled: true there first");
    expect(await theSwitch("hook")).not.toBeChecked();
    expect(await theSwitch("hook")).toBeEnabled();
  });

  it("asks about missed runs before switching on an overdue schedule, and sends Recover", async () => {
    server.triggers = flowTriggers({
      triggers: [scheduleEntry({ runtime_disabled: true, effective_disabled: true }, { next_evaluation_date: OVERDUE, missed_ticks: { count: 3, more: false } })],
    });
    renderTriggersTab(server);
    await userEvent.click(await theSwitch("daily"));
    const dialog = await screen.findByRole("dialog", { name: "Enable trigger" });
    expect(server.callsTo("PUT", switchPath("daily"))).toHaveLength(0);
    await userEvent.click(within(dialog).getByRole("radio", { name: "Recover missed runs (3 missed)" }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Enable" }));
    await waitFor(() => expect(server.callsTo("PUT", switchPath("daily"))).toHaveLength(1));
    expect(server.callsTo("PUT", switchPath("daily"))[0].body).toEqual({ disabled: false, recover_missed: true });
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await waitFor(async () => expect(await theSwitch("daily")).toBeChecked());
  });

  it("sends Skip, the dialog's default", async () => {
    server.triggers = flowTriggers({
      triggers: [scheduleEntry({ runtime_disabled: true, effective_disabled: true }, { next_evaluation_date: OVERDUE, missed_ticks: { count: 3, more: false } })],
    });
    renderTriggersTab(server);
    await userEvent.click(await theSwitch("daily"));
    const dialog = await screen.findByRole("dialog", { name: "Enable trigger" });
    expect(within(dialog).getByRole("radio", { name: "Skip missed runs; start at the next tick" })).toBeChecked();
    await userEvent.click(within(dialog).getByRole("button", { name: "Enable" }));
    await waitFor(() => expect(server.callsTo("PUT", switchPath("daily"))).toHaveLength(1));
    expect(server.callsTo("PUT", switchPath("daily"))[0].body).toEqual({ disabled: false, recover_missed: false });
  });

  it("switches a schedule whose next tick is ahead on at once, with no question", async () => {
    server.triggers = flowTriggers({
      triggers: [scheduleEntry({ runtime_disabled: true, effective_disabled: true }, { next_evaluation_date: FUTURE })],
    });
    renderTriggersTab(server);
    await userEvent.click(await theSwitch("daily"));
    await waitFor(() => expect(server.callsTo("PUT", switchPath("daily"))).toHaveLength(1));
    expect(server.callsTo("PUT", switchPath("daily"))[0].body).toEqual({ disabled: false, recover_missed: false });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  // A browser clock behind the server's: the next tick reads as ahead here, but
  // the server, at its own now(), counted missed ticks. Without the question the
  // enable would carry recover_missed:false and drop the backlog.
  it("asks when the server counts missed ticks though the browser's clock reads the tick as ahead", async () => {
    server.triggers = flowTriggers({
      triggers: [scheduleEntry({ runtime_disabled: true, effective_disabled: true }, { next_evaluation_date: FUTURE, missed_ticks: { count: 2, more: false } })],
    });
    renderTriggersTab(server);
    await userEvent.click(await theSwitch("daily"));
    const dialog = await screen.findByRole("dialog", { name: "Enable trigger" });
    expect(within(dialog).getByRole("radio", { name: "Recover missed runs (2 missed)" })).toBeInTheDocument();
    expect(server.callsTo("PUT", switchPath("daily"))).toHaveLength(0);
  });

  it("says, on a switch that is on, that a disabled flow fires nothing (D18)", async () => {
    server.triggers = flowTriggers({ flow_disabled: true, triggers: [webhookEntry({ effective_disabled: true })] });
    renderTriggersTab(server);
    const hook = await theSwitch("hook");
    expect(hook).toBeChecked();
    expect(hook).toBeEnabled();
    expect(hook).toHaveAttribute("title", "On, but the flow is disabled, so it does not fire");
    expect(statusOf("hook")).toBe("On, but the flow is disabled, so it does not fire");
  });
});

describe("FlowTriggersTab: bulk", () => {
  it("disables a selection in one bulk request, and names a refused trigger", async () => {
    eachKind();
    server.bulkRefusals = { hook: { code: "not_found", message: "not found: trigger hook" } };
    renderTriggersTab(server);
    await theSwitch("daily");
    await userEvent.click(screen.getByRole("checkbox", { name: "Select daily" }));
    await userEvent.click(screen.getByRole("checkbox", { name: "Select hook" }));
    await userEvent.click(screen.getByRole("button", { name: "Disable" }));
    await waitFor(() => expect(server.callsTo("PUT", BULK)).toHaveLength(1));
    expect(server.callsTo("PUT", BULK)[0].body).toEqual({
      triggers: [
        { flow_id: FLOW_ID, trigger_id: "daily" },
        { flow_id: FLOW_ID, trigger_id: "hook" },
      ],
      disabled: true,
      recover_missed: false,
    });
    expect(server.calls.filter((c) => c.method === "PUT" && c.path !== BULK)).toHaveLength(0);
    const refused = await screen.findByTestId("bulk-refusals");
    expect(refused).toHaveTextContent("1 trigger was not switched:");
    expect(refused).toHaveTextContent("hook: not found: trigger hook");
    await waitFor(async () => expect(await theSwitch("daily")).not.toBeChecked());
    expect(await theSwitch("hook")).toBeChecked();
    expect(toast.success).toHaveBeenCalledWith("Switched 1 trigger off.");
    // Answered: the selection is cleared; the refused row is named above.
    expect(screen.getByRole("checkbox", { name: "Select daily" })).not.toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Select hook" })).not.toBeChecked();
  });

  it("asks once, in its bulk form, when the selection holds an overdue schedule", async () => {
    server.triggers = flowTriggers({
      triggers: [
        scheduleEntry({ runtime_disabled: true, effective_disabled: true }, { next_evaluation_date: OVERDUE, missed_ticks: { count: 1000, more: true } }),
        webhookEntry({ runtime_disabled: true, effective_disabled: true }),
      ],
    });
    renderTriggersTab(server);
    await theSwitch("daily");
    await userEvent.click(screen.getByRole("checkbox", { name: "Select all triggers" }));
    await userEvent.click(screen.getByRole("button", { name: "Enable" }));
    const dialog = await screen.findByRole("dialog", { name: "Enable 2 triggers" });
    await userEvent.click(within(dialog).getByRole("radio", { name: "Recover missed runs (1000+ missed)" }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Enable" }));
    await waitFor(() => expect(server.callsTo("PUT", BULK)).toHaveLength(1));
    expect(server.callsTo("PUT", BULK)[0].body).toEqual({
      triggers: [
        { flow_id: FLOW_ID, trigger_id: "daily" },
        { flow_id: FLOW_ID, trigger_id: "hook" },
      ],
      disabled: false,
      recover_missed: true,
    });
  });
  it("asks in its bulk form when only the server's clock says a schedule is overdue", async () => {
    server.triggers = flowTriggers({
      triggers: [
        scheduleEntry({ runtime_disabled: true, effective_disabled: true }, { next_evaluation_date: FUTURE, missed_ticks: { count: 2, more: false } }),
        webhookEntry({ runtime_disabled: true, effective_disabled: true }),
      ],
    });
    renderTriggersTab(server);
    await theSwitch("daily");
    await userEvent.click(screen.getByRole("checkbox", { name: "Select all triggers" }));
    await userEvent.click(screen.getByRole("button", { name: "Enable" }));
    const dialog = await screen.findByRole("dialog", { name: "Enable 2 triggers" });
    expect(within(dialog).getByRole("radio", { name: "Recover missed runs (2 missed)" })).toBeInTheDocument();
    expect(server.callsTo("PUT", BULK)).toHaveLength(0);
  });
});

describe("FlowTriggersTab: banners, empty, read-only", () => {
  it("raises the four banners from the server's flags and the flow", async () => {
    eachKind();
    server.triggers = { ...server.triggers, scheduler_enabled: false, worker_enabled: false, webhooks_enabled: false, flow_disabled: true };
    renderTriggersTab(server);
    await theSwitch("daily");
    const banners = within(screen.getByTestId("trigger-banners")).getAllByRole("status").map((b) => b.textContent);
    expect(banners).toEqual([
      "The scheduler is off on this server, so schedules and flow triggers do not fire. An administrator sets workflow.schedulerEnabled.",
      "The workflow worker is off on this server, so executions are created but do not run. An administrator turns it on with workflow.workerEnabled (EGRET_WORKFLOW__WORKER_ENABLED=true).",
      "Webhooks are off on this server; their URLs answer 404.",
      "This flow is disabled, so none of its triggers fire.",
    ]);
  });

  it("raises none when the server runs everything and the flow is on", async () => {
    eachKind();
    renderTriggersTab(server);
    await theSwitch("daily");
    expect(screen.queryByTestId("trigger-banners")).not.toBeInTheDocument();
  });

  it("says a flow without triggers has none, with the way to the editor", async () => {
    renderTriggersTab(server);
    expect(await screen.findByText("This flow has no triggers")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Open the editor" })).toHaveAttribute("href", `/explore/${WS}/workflows/flows/${FLOW_ID}/edit`);
  });

  it("shows the error, not an empty tab, when the triggers cannot be read", async () => {
    server.triggersReply = { status: 503, body: { error: "the database is unavailable" } };
    renderTriggersTab(server);
    expect(await screen.findByText(/The triggers could not be loaded: the database is unavailable/)).toBeInTheDocument();
    expect(screen.queryByText("This flow has no triggers")).not.toBeInTheDocument();
  });

  it("disables every control for a viewer without workflow:manage, and says why", async () => {
    server.triggers = flowTriggers({
      triggers: [
        scheduleEntry({ can_toggle: false, toggle_refusal: "forbidden" }),
        webhookEntry({ can_toggle: false, toggle_refusal: "forbidden" }),
      ],
    });
    renderTriggersTab(server, { manage: false });
    for (const id of ["daily", "hook"]) {
      const s = await theSwitch(id);
      expect(s).toBeDisabled();
      expect(s).toHaveAccessibleDescription("You need workflow:manage");
      const box = screen.getByRole("checkbox", { name: `Select ${id}` });
      expect(box).toBeDisabled();
      expect(box).toHaveAccessibleDescription("You need workflow:manage");
      // From visible text the box names, not a hover-only `title`.
      expect(box).not.toHaveAttribute("title");
      expect(document.getElementById(box.getAttribute("aria-describedby")!)).toHaveTextContent("You need workflow:manage");
    }
    expect(screen.getByRole("checkbox", { name: "Select all triggers" })).toBeDisabled();
    for (const name of ["Enable", "Disable"]) {
      const b = screen.getByRole("button", { name });
      expect(b).toBeDisabled();
      expect(b).toHaveAccessibleDescription("You need workflow:manage");
    }
    // No fieldset or other ancestor disables them for the harness: each control is disabled itself.
    expect(document.querySelector("fieldset")).toBeNull();
  });
});

describe("FlowTriggersTab: refresh and failures", () => {
  it("reads the triggers again every 30 seconds, only while the browser tab is visible", async () => {
    eachKind();
    const { client } = renderTriggersTab(server);
    await theSwitch("daily");
    const query = client.getQueryCache().find({ queryKey: workflowsKeys.flowTriggers(FLOW_ID) })!;
    expect(TRIGGERS_POLL_MS).toBe(30_000);
    expect(query.options).toMatchObject({ refetchIntervalInBackground: false });
    // The interval is a function of the answer: 30 s with no backfill running (asserted in the next test).
    const interval = (query.options as { refetchInterval: (q: unknown) => number | false }).refetchInterval;
    expect(interval({ state: { data: undefined } })).toBe(30_000);
  });

  it("keeps the dialog open with the choice on a coded refusal (recover_missed_unknown)", async () => {
    server.triggers = flowTriggers({
      triggers: [
        scheduleEntry(
          { runtime_disabled: true, effective_disabled: true },
          { next_evaluation_date: OVERDUE, missed_ticks: { count: 3, more: false }, recover_missed_schedules: "SOMETIMES" },
        ),
      ],
    });
    server.triggerPutReply = {
      status: 409,
      body: { error: "conflict: recoverMissedSchedules SOMETIMES is not ALL, NONE or LAST", code: "recover_missed_unknown" },
    };
    renderTriggersTab(server);
    await userEvent.click(await theSwitch("daily"));
    const dialog = await screen.findByRole("dialog", { name: "Enable trigger" });
    await userEvent.click(within(dialog).getByRole("radio", { name: /^Recover missed runs/ }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Enable" }));
    await waitFor(() => expect(toast.error).toHaveBeenCalled());
    expect(toast.error.mock.calls[0][0]).toContain("recoverMissedSchedules SOMETIMES is not ALL, NONE or LAST");
    expect(screen.getByRole("dialog", { name: "Enable trigger" })).toBeInTheDocument();
    expect(within(screen.getByRole("dialog")).getByRole("radio", { name: /^Recover missed runs/ })).toBeChecked();
    // Behind the open (modal) dialog, the switch has not moved.
    expect(screen.getByRole("switch", { name: "Trigger daily enabled", hidden: true })).not.toBeChecked();
  });

  it("says a trigger the flow no longer declares is gone, on a 404", async () => {
    eachKind();
    server.triggerPutReply = { status: 404, body: { error: "not found: trigger" } };
    renderTriggersTab(server);
    await userEvent.click(await theSwitch("hook"));
    await waitFor(() => expect(toast.error).toHaveBeenCalled());
    expect(toast.error).toHaveBeenCalledWith("hook was not switched off: This trigger no longer exists in the flow; reload.");
    expect(await theSwitch("hook")).toBeChecked();
  });

  it("shows a refused bulk request's sentence and keeps the selection", async () => {
    eachKind();
    server.bulkReply = { status: 400, body: { error: "bad request: at most 100 triggers per request" } };
    renderTriggersTab(server);
    await theSwitch("daily");
    await userEvent.click(screen.getByRole("checkbox", { name: "Select daily" }));
    await userEvent.click(screen.getByRole("button", { name: "Disable" }));
    await waitFor(() => expect(toast.error).toHaveBeenCalled());
    expect(toast.error).toHaveBeenCalledWith("The triggers were not switched off: bad request: at most 100 triggers per request");
    expect(screen.getByRole("checkbox", { name: "Select daily" })).toBeChecked();
    expect(await theSwitch("daily")).toBeChecked();
    expect(screen.queryByTestId("bulk-refusals")).not.toBeInTheDocument();
  });

  it("keeps the focus on a bulk button while its request runs", async () => {
    eachKind();
    let release!: () => void;
    server.putGate = new Promise((r) => (release = r));
    renderTriggersTab(server);
    await theSwitch("daily");
    await userEvent.click(screen.getByRole("checkbox", { name: "Select daily" }));
    const disable = screen.getByRole("button", { name: "Disable" });
    await userEvent.click(disable);
    await waitFor(() => expect(disable).toHaveAttribute("aria-disabled", "true"));
    // Not `disabled`, which a browser would blur (jsdom keeps the focus either way).
    expect(disable).toBeEnabled();
    expect(disable).toHaveFocus();
    await userEvent.click(disable);
    release();
    await waitFor(async () => expect(await theSwitch("daily")).not.toBeChecked());
    expect(server.callsTo("PUT", BULK)).toHaveLength(1);
    expect(disable).toHaveFocus();
  });
});

describe("FlowTriggersTab: backfills (W7f §4.1)", () => {
  it("polls every 5 s while a backfill runs, and 30 s once it is paused", async () => {
    server.triggers = flowTriggers({ triggers: [scheduleEntry({ backfill: backfill() })] });
    const { client } = renderTriggersTab(server);
    await screen.findByTestId("backfill-strip");
    const query = client.getQueryCache().find({ queryKey: workflowsKeys.flowTriggers(FLOW_ID) })!;
    const interval = (query.options as { refetchInterval: (q: unknown) => number | false }).refetchInterval;
    const answer = (triggers: unknown[]) => ({ state: { data: { status: 200, data: { triggers } } } });
    expect(interval(answer([scheduleEntry({ backfill: backfill() })]))).toBe(5_000);
    expect(interval(answer([scheduleEntry({ backfill: backfill({ paused: true }) })]))).toBe(TRIGGERS_POLL_MS);
  });

  it("Pause sends the paused flag and the strip says Paused once the server answers", async () => {
    server.triggers = flowTriggers({ triggers: [scheduleEntry({ backfill: backfill() })] });
    renderTriggersTab(server);
    const strip = await screen.findByTestId("backfill-strip");
    await userEvent.click(within(strip).getByRole("button", { name: "Pause" }));
    await waitFor(() => expect(within(screen.getByTestId("backfill-strip")).getByText("Paused")).toBeInTheDocument());
    const puts = server.callsTo("PUT", `/api/ws/${WS}/workflows/flows/${FLOW_ID}/triggers/daily/backfill/paused`);
    expect(puts.map((c) => c.body)).toEqual([{ paused: true }]);
  });

  it("Delete asks first; Confirm sends the delete and the strip goes", async () => {
    server.triggers = flowTriggers({ triggers: [scheduleEntry({ backfill: backfill() })] });
    renderTriggersTab(server);
    const strip = await screen.findByTestId("backfill-strip");
    await userEvent.click(within(strip).getByRole("button", { name: "Delete" }));
    await userEvent.click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Delete backfill" }));
    await waitFor(() => expect(screen.queryByTestId("backfill-strip")).toBeNull());
    expect(server.callsTo("DELETE", `/api/ws/${WS}/workflows/flows/${FLOW_ID}/triggers/daily/backfill`)).toHaveLength(1);
  });
});
